//! HTML document analysis: turns raw markup into [`HtmlData`].

use crate::model::{Form, HtmlData, Iframe, Image, Link, Script};
use regex::Regex;
use scraper::{ElementRef, Html};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use url::Url;

const TEXT_CAP: usize = 20_000;

/// Decode a response body using the charset from the Content-Type header,
/// falling back to a `<meta charset>` sniff and finally UTF-8.
pub fn decode_body(bytes: &[u8], content_type: &str) -> String {
    let label = charset_from_content_type(content_type).or_else(|| sniff_meta_charset(bytes));
    let enc = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = enc.decode(bytes);
    text.into_owned()
}

fn charset_from_content_type(ct: &str) -> Option<String> {
    ct.split(';')
        .map(|p| p.trim())
        .find_map(|p| {
            p.strip_prefix("charset=")
                .or_else(|| p.strip_prefix("Charset="))
        })
        .map(|c| c.trim_matches(['"', '\'']).to_string())
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+charset\s*=\s*["']?([a-zA-Z0-9_\-]+)"#).unwrap()
    });
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    re.captures(&head).map(|c| c[1].to_string())
}

/// Decode %XX escapes (URL fragments are stored encoded, element ids are not).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (
            b[i],
            b.get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok()),
        ) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_of(el: ElementRef) -> String {
    norm(&el.text().collect::<Vec<_>>().join(" "))
}

fn rel_tokens(el: &ElementRef) -> Vec<String> {
    el.value()
        .attr("rel")
        .unwrap_or("")
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn attr_lower(el: &ElementRef, name: &str) -> String {
    el.value()
        .attr(name)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn has_ancestor(el: &ElementRef, name: &str) -> bool {
    el.ancestors()
        .any(|a| a.value().as_element().is_some_and(|e| e.name() == name))
}

pub(crate) const BLOCKS: &[&str] = &[
    "p",
    "div",
    "li",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "td",
    "th",
    "tr",
    "table",
    "section",
    "article",
    "aside",
    "header",
    "footer",
    "nav",
    "main",
    "blockquote",
    "figcaption",
    "figure",
    "pre",
    "form",
    "fieldset",
    "details",
    "summary",
    "address",
    "caption",
    "option",
    "button",
    "label",
    "br",
    "hr",
];

/// Visible text below `el`, skipping non-rendered subtrees. Block boundaries become newlines so
/// menus and grids without punctuation still split into separate sentences for readability.
/// Iterative: hostile pages nest deeply enough to overflow a worker thread's stack with recursion.
fn visible_text(el: ElementRef, out: &mut String) {
    let mut stack = vec![(el.children(), false)];
    while let Some((children, _)) = stack.last_mut() {
        let Some(child) = children.next() else {
            if stack.pop().is_some_and(|(_, block)| block) {
                out.push('\n');
            }
            continue;
        };
        if let Some(t) = child.value().as_text() {
            out.push_str(t);
            out.push(' ');
        } else if let Some(ch) = ElementRef::wrap(child) {
            match ch.value().name() {
                "script" | "style" | "noscript" | "template" | "svg" | "head" | "iframe"
                | "object" => {}
                n => {
                    let block = BLOCKS.contains(&n);
                    if block {
                        out.push('\n');
                    }
                    stack.push((ch.children(), block));
                }
            }
        }
    }
}

/// Sentences: `.!?`-separated runs of 2+ words, and every text block counts as at least one.
fn count_sentences(text: &str) -> usize {
    text.split('\n')
        .filter(|b| {
            b.split_whitespace()
                .any(|w| w.chars().any(|c| c.is_alphanumeric()))
        })
        .map(|b| {
            b.split(['.', '!', '?'])
                .filter(|s| s.split_whitespace().count() >= 2)
                .count()
                .max(1)
        })
        .sum()
}

/// Deepest element nesting handed to the HTML parser. html5ever's scope checks walk the whole
/// open-element stack, so 100k nested tags take minutes; browsers cap tree depth too (Blink: 512).
const MAX_DEPTH: usize = 512;

/// Remove start tags that would nest deeper than [`MAX_DEPTH`], keeping their content. Uses a
/// rough open-element stack: void and self-closing tags, and tags the parser closes implicitly
/// (p, li, td, a…), are not tracked; an end tag pops back to its matching start tag.
fn cap_nesting(raw: &str) -> Cow<'_, str> {
    const UNTRACKED: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr", "keygen", "basefont", "bgsound", "frame", "image", "html",
        "head", "body", "p", "li", "dt", "dd", "tr", "td", "th", "tbody", "thead", "tfoot",
        "colgroup", "caption", "option", "optgroup", "rb", "rt", "rp", "a", "nobr", "button",
        "form",
    ];
    const RAW_TEXT: &[&str] = &[
        "script",
        "style",
        "textarea",
        "title",
        "xmp",
        "iframe",
        "noembed",
        "noframes",
        "plaintext",
    ];
    let has = |list: &[&str], name: &str| list.iter().any(|t| t.eq_ignore_ascii_case(name));
    let b = raw.as_bytes();
    let find = |from: usize, needle: &[u8]| -> Option<usize> {
        b.get(from..)?
            .windows(needle.len())
            .position(|w| w.eq_ignore_ascii_case(needle))
            .map(|p| from + p)
    };
    let mut stack: Vec<&str> = Vec::new();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut i = 0;
    while let Some(start) = b
        .get(i..)
        .and_then(|rest| rest.iter().position(|&c| c == b'<'))
        .map(|p| i + p)
    {
        i = start + 1;
        if b[i..].starts_with(b"!--") {
            i = find(i, b"-->").map_or(b.len(), |p| p + 3);
            continue;
        }
        let closing = b.get(i) == Some(&b'/');
        let ns = i + closing as usize;
        let ne = ns
            + b[ns.min(b.len())..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-')
                .count();
        if ne == ns || !b[ns].is_ascii_alphabetic() {
            continue;
        }
        let name = &raw[ns..ne]; // ASCII, so on char boundaries
        let Some(end) = find(ne, b">") else { break };
        i = end + 1;
        if closing {
            if !has(UNTRACKED, name) {
                if let Some(pos) = stack.iter().rposition(|n| n.eq_ignore_ascii_case(name)) {
                    stack.truncate(pos);
                }
            }
        } else if has(RAW_TEXT, name) {
            let close = format!("</{name}");
            i = find(i, close.as_bytes()).unwrap_or(b.len());
        } else if b[end - 1] != b'/' && !has(UNTRACKED, name) {
            if stack.len() < MAX_DEPTH {
                stack.push(name);
            } else {
                let o = out.get_or_insert_with(|| String::with_capacity(raw.len()));
                o.push_str(&raw[copied..start]);
                copied = end + 1;
            }
        }
    }
    match out {
        Some(mut o) => {
            o.push_str(&raw[copied..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(raw),
    }
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;

pub fn fnv64(data: &[u8]) -> u64 {
    fnv64_continue(FNV_OFFSET, data)
}

fn fnv64_continue(mut h: u64, data: &[u8]) -> u64 {
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 64-bit simhash over 3-word shingles. Similar documents share most bits.
pub fn simhash(words: &[String]) -> u64 {
    if words.len() < 3 {
        return fnv64(words.join(" ").as_bytes());
    }
    let mut votes = [0i32; 64];
    for w in words.windows(3) {
        // fnv64 of "w0 w1 w2" without building the string
        let h = fnv64_continue(
            fnv64_continue(
                fnv64_continue(
                    fnv64_continue(fnv64_continue(FNV_OFFSET, w[0].as_bytes()), b" "),
                    w[1].as_bytes(),
                ),
                b" ",
            ),
            w[2].as_bytes(),
        );
        for (bit, v) in votes.iter_mut().enumerate() {
            if h >> bit & 1 == 1 {
                *v += 1
            } else {
                *v -= 1
            }
        }
    }
    votes.iter().enumerate().fold(
        0u64,
        |acc, (bit, v)| if *v > 0 { acc | 1 << bit } else { acc },
    )
}

/// Vowel groups over the ASCII letters of `word` (allocation-free: runs for every word).
fn syllables(word: &str) -> usize {
    let (mut count, mut prev, mut len, mut last, mut before_last) = (0, false, 0, 0u8, 0u8);
    for c in word
        .bytes()
        .filter(|b| b.is_ascii_alphabetic())
        .map(|b| b.to_ascii_lowercase())
    {
        let v = b"aeiouy".contains(&c);
        if v && !prev {
            count += 1;
        }
        prev = v;
        len += 1;
        before_last = last;
        last = c;
    }
    if len == 0 {
        return 0;
    }
    // silent final "e", but not "-le"
    if len > 2 && last == b'e' && before_last != b'l' && count > 1 {
        count -= 1;
    }
    count.max(1)
}

/// Flesch reading ease (English). Higher is easier; < 30 is very difficult.
pub fn flesch(words: &[String], sentences: usize) -> f32 {
    if words.is_empty() {
        return 0.0;
    }
    let s = sentences.max(1) as f32;
    let w = words.len() as f32;
    let syl: usize = words.iter().map(|w| syllables(w)).sum();
    206.835 - 1.015 * (w / s) - 84.6 * (syl as f32 / w)
}

const GENERIC_ANCHORS: &[&str] = &[
    "click here",
    "here",
    "read more",
    "more",
    "learn more",
    "this link",
    "link",
    "click",
    "go",
    "this page",
    "more info",
    "details",
    "continue",
];

const DEPRECATED: &[&str] = &[
    "center", "font", "marquee", "blink", "big", "strike", "tt", "acronym", "applet", "basefont",
    "dir", "isindex", "frame", "frameset", "noframes",
];

const HEAD_ALLOWED: &[&str] = &[
    "title", "meta", "link", "script", "style", "base", "noscript", "template", "head",
];

pub fn parse_html(raw: &str, page_url: &Url) -> HtmlData {
    parse_html_probed(raw, page_url, &[])
}

/// [`parse_html`] that also counts matches for rule-pack tests that need the DOM or raw HTML.
pub fn parse_html_probed(raw: &str, page_url: &Url, probes: &[crate::rules::Probe]) -> HtmlData {
    static RE_STRIP: OnceLock<Regex> = OnceLock::new();
    static RE_TAG: OnceLock<Regex> = OnceLock::new();
    static RE_JS_REDIRECT: OnceLock<Regex> = OnceLock::new();
    static RE_FONT_FACE: OnceLock<Regex> = OnceLock::new();
    let re_strip = RE_STRIP.get_or_init(|| {
        Regex::new(r"(?is)<!--.*?-->|<script\b.*?</script>|<style\b.*?</style>").unwrap()
    });
    let re_tag = RE_TAG.get_or_init(|| Regex::new(r"(?i)<([a-z][a-z0-9-]*)").unwrap());
    let re_js_redirect = RE_JS_REDIRECT.get_or_init(|| {
        // The object prefix is optional: a bare `location.href = …` redirects too.
        Regex::new(r"(?:^|[^\w$.]|(?:window|document|top|self)\.)location(?:\.href)?\s*=[^=]|location\.(?:replace|assign)\s*\(").unwrap()
    });
    let re_font_face =
        RE_FONT_FACE.get_or_init(|| Regex::new(r"(?is)@font-face\s*\{[^}]*\}").unwrap());

    let mut d = HtmlData::default();
    let doc = Html::parse_document(&cap_nesting(raw));

    // ---- raw-markup structure checks
    let stripped = re_strip.replace_all(raw, "");
    let lower = stripped.to_ascii_lowercase();
    d.has_doctype = raw
        .trim_start_matches('\u{feff}')
        .trim_start()
        .get(..9)
        .is_some_and(|s| s.eq_ignore_ascii_case("<!doctype"));
    d.head_tags = re_tag
        .captures_iter(&lower)
        .filter(|c| &c[1] == "head")
        .count();
    d.body_tags = re_tag
        .captures_iter(&lower)
        .filter(|c| &c[1] == "body")
        .count();
    if let (Some(start), Some(end)) = (lower.find("<head"), lower.find("</head>")) {
        if start < end {
            let mut bad: Vec<String> = re_tag
                .captures_iter(&lower[start..end])
                .map(|c| c[1].to_string())
                .filter(|t| !HEAD_ALLOWED.contains(&t.as_str()))
                .collect();
            bad.sort();
            bad.dedup();
            d.invalid_in_head = bad;
        }
    }

    // ---- base href first so every URL resolves correctly
    let mut base = page_url.clone();
    for el in doc
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        if el.value().name() == "base" {
            if let Some(h) = el.value().attr("href") {
                d.base_href = Some(h.to_string());
                if let Ok(b) = page_url.join(h.trim()) {
                    base = b;
                }
            }
            break;
        }
    }
    let resolve = |href: &str| -> Option<Url> {
        let mut u = base.join(href.trim()).ok()?;
        if u.scheme() != "http" && u.scheme() != "https" {
            return None;
        }
        u.set_fragment(None);
        Some(u)
    };

    // label[for] targets, for form accessibility
    let mut label_for: HashSet<String> = HashSet::new();
    for el in doc
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        if el.value().name() == "label" {
            if let Some(f) = el.value().attr("for") {
                label_for.insert(f.to_string());
            }
        }
    }

    // Elements inside <head>, found once instead of walking every element's ancestors.
    let mut in_head_ids = HashSet::new();
    for head in doc
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "head")
    {
        in_head_ids.extend(head.descendants().skip(1).map(|n| n.id()));
    }

    let mut id_counts: HashMap<String, usize> = HashMap::new();
    let mut anchor_names: HashSet<String> = HashSet::new();
    let mut deprecated: HashSet<String> = HashSet::new();

    for el in doc
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        d.dom_elements += 1;
        let name = el.value().name();
        let in_head = in_head_ids.contains(&el.id());

        if let Some(id) = el.value().attr("id") {
            *id_counts.entry(id.to_string()).or_insert(0) += 1;
        }
        // <a name="x"> is also a valid #x target.
        if name == "a" {
            if let Some(n) = el.value().attr("name") {
                anchor_names.insert(n.to_string());
            }
        }
        if let Some(t) = el.value().attr("tabindex") {
            if t.trim().parse::<i32>().is_ok_and(|v| v > 0) {
                d.positive_tabindex += 1;
            }
        }
        if el.value().attr("itemscope").is_some() {
            d.microdata = true;
        }
        if el.value().attr("typeof").is_some() || el.value().attr("vocab").is_some() {
            d.rdfa = true;
        }
        if DEPRECATED.contains(&name) {
            deprecated.insert(name.to_string());
        }

        match name {
            "html" => {
                d.lang = el.value().attr("lang").map(|s| s.trim().to_string());
                d.is_amp = el.value().attr("amp").is_some() || el.value().attr("⚡").is_some();
            }
            "body" => {
                d.aria_hidden_body = attr_lower(&el, "aria-hidden") == "true";
            }
            "title" => {
                if has_ancestor(&el, "svg") {
                    continue;
                }
                d.titles.push(text_of(el));
                if !in_head {
                    d.title_outside_head = true;
                }
            }
            "meta" => {
                let mname = attr_lower(&el, "name");
                let prop = attr_lower(&el, "property");
                let content = el.value().attr("content").map(norm).unwrap_or_default();
                if let Some(cs) = el.value().attr("charset") {
                    d.charset = Some(cs.trim().to_string());
                }
                match attr_lower(&el, "http-equiv").as_str() {
                    "refresh" => d.meta_refresh = Some(content.clone()),
                    "content-type" if d.charset.is_none() => {
                        d.charset = charset_from_content_type(&content);
                    }
                    _ => {}
                }
                match mname.as_str() {
                    "description" => {
                        d.meta_descriptions.push(content.clone());
                        if !in_head {
                            d.description_outside_head = true;
                        }
                    }
                    "keywords" => d.meta_keywords.push(content.clone()),
                    "robots" => {
                        d.meta_robots.push(content.clone());
                        if !in_head {
                            d.robots_outside_head = true;
                        }
                    }
                    "googlebot" => d.meta_googlebot.push(content.clone()),
                    n if crate::ai_search::bot(n).is_some() => {
                        d.ai.bot_meta.push((n.to_string(), content.clone()))
                    }
                    "viewport" => d.viewport = Some(content.clone()),
                    n if n.starts_with("twitter:") => {
                        d.twitter.push((n.to_string(), content.clone()))
                    }
                    _ => {}
                }
                if prop.starts_with("og:") {
                    d.og.push((prop.clone(), content.clone()));
                } else if prop.starts_with("twitter:") && mname.is_empty() {
                    d.twitter.push((prop.clone(), content.clone()));
                }
            }
            "link" => {
                let rels = rel_tokens(&el);
                let href = el.value().attr("href").unwrap_or("");
                let has = |r: &str| rels.iter().any(|x| x == r);
                if has("canonical") {
                    d.canonicals_raw.push(href.to_string());
                    if let Some(u) = resolve(href) {
                        d.canonicals.push(u.to_string());
                    }
                    if !in_head {
                        d.canonical_outside_head = true;
                    }
                }
                if has("next") {
                    if let Some(u) = resolve(href) {
                        d.rel_next.push(u.to_string());
                    }
                }
                if has("prev") || has("previous") {
                    if let Some(u) = resolve(href) {
                        d.rel_prev.push(u.to_string());
                    }
                }
                if has("alternate") {
                    if let Some(lang) = el.value().attr("hreflang") {
                        let u = resolve(href)
                            .map(|u| u.to_string())
                            .unwrap_or_else(|| href.to_string());
                        d.hreflang.push((lang.trim().to_string(), u));
                        if !in_head {
                            d.hreflang_outside_head = true;
                        }
                    }
                }
                if has("amphtml") {
                    d.amphtml = resolve(href).map(|u| u.to_string());
                }
                if has("stylesheet") {
                    if let Some(u) = resolve(href) {
                        d.stylesheets.push(u.to_string());
                    }
                }
                if has("icon") {
                    d.favicon = true;
                }
                if has("apple-touch-icon") || has("apple-touch-icon-precomposed") {
                    d.apple_touch_icon = true;
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name.as_bytes()[1] - b'0';
                let text = text_of(el);
                d.heading_levels.push(level);
                if text.is_empty() {
                    let img_alt = el
                        .descendants()
                        .filter_map(ElementRef::wrap)
                        .find(|e| e.value().name() == "img")
                        .and_then(|i| i.value().attr("alt").map(norm))
                        .filter(|a| !a.is_empty());
                    match (level, img_alt) {
                        (1, Some(alt)) => {
                            d.h1_image_only = true;
                            d.h1.push(alt);
                        }
                        (2, Some(alt)) => d.h2.push(alt),
                        _ => {
                            d.empty_headings += 1;
                            if level == 1 {
                                d.h1.push(String::new());
                            } else if level == 2 {
                                d.h2.push(String::new());
                            }
                        }
                    }
                } else if level == 1 {
                    d.h1.push(text);
                } else if level == 2 {
                    d.h2.push(text);
                }
            }
            "a" | "area" => {
                let Some(raw_href) = el.value().attr("href") else {
                    continue;
                };
                let href_trim = raw_href.trim();
                let lower_href = href_trim.to_ascii_lowercase();
                if lower_href.starts_with("javascript:") {
                    d.javascript_links += 1;
                    continue;
                }
                if href_trim.is_empty()
                    || href_trim.starts_with('#')
                    || lower_href.starts_with("mailto:")
                    || lower_href.starts_with("tel:")
                    || lower_href.starts_with("data:")
                    || lower_href.starts_with("sms:")
                {
                    continue;
                }
                if raw_href != href_trim || href_trim.contains(char::is_whitespace) {
                    d.links_with_whitespace += 1;
                }
                let Ok(mut full) = base.join(href_trim) else {
                    continue;
                };
                if full.scheme() != "http" && full.scheme() != "https" {
                    continue;
                }
                let fragment = full
                    .fragment()
                    .filter(|f| !f.is_empty())
                    .map(percent_decode);
                full.set_fragment(None);

                let mut anchor = text_of(el);
                let mut image_link = false;
                if anchor.is_empty() {
                    if let Some(alt) = el
                        .descendants()
                        .filter_map(ElementRef::wrap)
                        .find(|e| e.value().name() == "img")
                        .map(|i| i.value().attr("alt").map(norm).unwrap_or_default())
                    {
                        image_link = true;
                        anchor = alt;
                    }
                }
                if anchor.is_empty() {
                    anchor = el
                        .value()
                        .attr("aria-label")
                        .or(el.value().attr("title"))
                        .map(norm)
                        .unwrap_or_default();
                }
                if anchor.is_empty() && el.value().attr("aria-labelledby").is_none() {
                    d.empty_links += 1;
                }
                if GENERIC_ANCHORS.contains(&anchor.to_ascii_lowercase().as_str()) {
                    d.generic_anchor_links += 1;
                }
                let rels = rel_tokens(&el);
                d.links.push(Link {
                    url: full.to_string(),
                    raw_href: raw_href.to_string(),
                    anchor,
                    image_link,
                    nofollow: rels.iter().any(|r| r == "nofollow"),
                    sponsored: rels.iter().any(|r| r == "sponsored"),
                    ugc: rels.iter().any(|r| r == "ugc"),
                    target_blank: attr_lower(&el, "target") == "_blank",
                    noopener: rels.iter().any(|r| r == "noopener" || r == "noreferrer"),
                    fragment,
                });
            }
            "img" => {
                let src = el
                    .value()
                    .attr("src")
                    .filter(|s| !s.trim().is_empty() && !s.trim().starts_with("data:"));
                let lazy_src = el
                    .value()
                    .attr("data-src")
                    .or(el.value().attr("data-lazy-src"));
                let Some(u) = src.or(lazy_src).and_then(resolve) else {
                    continue;
                };
                d.images.push(Image {
                    url: u.to_string(),
                    alt: el.value().attr("alt").map(norm),
                    has_width: el.value().attr("width").is_some(),
                    has_height: el.value().attr("height").is_some(),
                    lazy: attr_lower(&el, "loading") == "lazy"
                        || (src.is_none() && lazy_src.is_some()),
                    srcset: el.value().attr("srcset").is_some()
                        || el.value().attr("data-srcset").is_some(),
                });
            }
            "script" => {
                let ty = attr_lower(&el, "type");
                if let Some(src) = el.value().attr("src") {
                    if let Some(u) = resolve(src) {
                        d.scripts.push(Script {
                            url: u.to_string(),
                            in_head,
                            is_async: el.value().attr("async").is_some(),
                            defer: el.value().attr("defer").is_some(),
                            module: ty == "module",
                        });
                    }
                } else if ty == "application/ld+json" {
                    let body: String = el.text().collect();
                    match serde_json::from_str::<serde_json::Value>(body.trim()) {
                        Ok(v) => d.json_ld.push(v),
                        Err(e) => d.json_ld_errors.push(e.to_string()),
                    }
                } else if ty.is_empty() || ty.contains("javascript") || ty == "module" {
                    let body: String = el.text().collect();
                    d.inline_scripts += 1;
                    d.inline_script_bytes += body.len();
                    if re_js_redirect.is_match(&body) {
                        d.js_redirect = true;
                    }
                    if body.contains("document.write(") {
                        d.document_write = true;
                    }
                }
            }
            "style" => {
                let body: String = el.text().collect();
                d.inline_styles_bytes += body.len();
                // Only fonts fetched over the network need font-display; local() and
                // inline data: fonts are available without a download.
                let web_font = |f: &str| {
                    f.split("url(").skip(1).any(|u| {
                        !u.trim_start()
                            .trim_start_matches(['"', '\''])
                            .starts_with("data:")
                    })
                };
                if re_font_face
                    .find_iter(&body)
                    .any(|m| web_font(m.as_str()) && !m.as_str().contains("font-display"))
                {
                    d.font_face_without_display = true;
                }
            }
            "iframe" => {
                if let Some(u) = el.value().attr("src").and_then(resolve) {
                    d.iframes.push(Iframe {
                        url: u.to_string(),
                        has_title: el
                            .value()
                            .attr("title")
                            .is_some_and(|t| !t.trim().is_empty()),
                        has_size: el.value().attr("width").is_some()
                            && el.value().attr("height").is_some(),
                    });
                }
            }
            "form" => {
                let action = el
                    .value()
                    .attr("action")
                    .filter(|a| !a.trim().is_empty())
                    .and_then(resolve)
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| page_url.to_string());
                let has_password = el
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .any(|e| e.value().name() == "input" && attr_lower(&e, "type") == "password");
                d.forms.push(Form {
                    action,
                    has_password,
                });
            }
            "input" | "select" | "textarea" => {
                let ty = attr_lower(&el, "type");
                if name == "input"
                    && ["hidden", "submit", "button", "image", "reset"].contains(&ty.as_str())
                {
                    continue;
                }
                let labelled = el
                    .value()
                    .attr("id")
                    .is_some_and(|id| label_for.contains(id))
                    || has_ancestor(&el, "label")
                    || el
                        .value()
                        .attr("aria-label")
                        .is_some_and(|s| !s.trim().is_empty())
                    || el.value().attr("aria-labelledby").is_some()
                    || el
                        .value()
                        .attr("title")
                        .is_some_and(|s| !s.trim().is_empty());
                if !labelled {
                    d.inputs_without_label += 1;
                }
            }
            "button" => {
                let named = !text_of(el).is_empty()
                    || el
                        .value()
                        .attr("aria-label")
                        .is_some_and(|s| !s.trim().is_empty())
                    || el.value().attr("aria-labelledby").is_some()
                    || el
                        .value()
                        .attr("title")
                        .is_some_and(|s| !s.trim().is_empty())
                    || el.descendants().filter_map(ElementRef::wrap).any(|e| {
                        e.value().name() == "img"
                            && e.value().attr("alt").is_some_and(|a| !a.trim().is_empty())
                    });
                if !named {
                    d.buttons_without_name += 1;
                }
            }
            "main" => d.has_main = true,
            "p" if d.lead.is_empty() => {
                let t = text_of(el);
                let chrome = ["nav", "header", "footer", "aside", "form"]
                    .iter()
                    .any(|n| has_ancestor(&el, n));
                if !chrome && t.split_whitespace().count() >= 12 {
                    d.lead = t;
                }
            }
            "video" | "audio" => {
                if el.value().attr("autoplay").is_some()
                    && (name == "audio" || el.value().attr("muted").is_none())
                {
                    d.autoplay_media = true;
                }
                if name == "video" {
                    let captions = el.descendants().filter_map(ElementRef::wrap).any(|e| {
                        e.value().name() == "track"
                            && matches!(attr_lower(&e, "kind").as_str(), "captions" | "subtitles")
                    });
                    if !captions {
                        d.videos_without_captions += 1;
                    }
                }
            }
            "table" => {
                let role = attr_lower(&el, "role");
                let has_th = el
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .any(|e| e.value().name() == "th");
                if !has_th && role != "presentation" && role != "none" {
                    d.tables_without_headers += 1;
                }
            }
            "frameset" | "frame" => d.uses_frames = true,
            "object" | "embed" | "applet" => d.uses_plugins = true,
            _ => {}
        }
        if el
            .value()
            .attr("role")
            .is_some_and(|r| r.eq_ignore_ascii_case("main"))
        {
            d.has_main = true;
        }
    }

    d.duplicate_ids = id_counts.values().filter(|&&c| c > 1).map(|c| c - 1).sum();
    let mut ids: Vec<String> = id_counts.into_keys().chain(anchor_names).collect();
    ids.sort();
    ids.dedup();
    ids.truncate(5000);
    d.ids = ids;
    let mut dep: Vec<String> = deprecated.into_iter().collect();
    dep.sort();
    d.deprecated_tags = dep;

    // ---- visible text, readability, fingerprints
    let mut text = String::new();
    let body = doc
        .root_element()
        .children()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == "body");
    if let Some(body) = body {
        visible_text(body, &mut text);
        crate::ai_search::quotability(body, &mut d.ai);
    }
    let words: Vec<String> = text
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .map(|w| w.to_lowercase())
        .collect();
    d.word_count = words.len();
    d.sentence_count = count_sentences(&text);
    d.flesch = flesch(&words, d.sentence_count);
    let joined = words.join(" ");
    d.content_hash = fnv64(joined.as_bytes());
    d.simhash = simhash(&words);
    // Placeholder copy counts only in prose; code samples (docs, tutorials) use it on purpose.
    d.lorem_ipsum = joined.contains("lorem ipsum")
        && body.is_some_and(|b| {
            b.descendants()
                .filter_map(|n| n.value().as_text().map(|t| (n, t)))
                .any(|(n, t)| {
                    t.to_lowercase().contains("lorem ipsum")
                        && !n
                            .ancestors()
                            .filter_map(|a| a.value().as_element())
                            .any(|e| {
                                matches!(e.name(), "pre" | "code" | "samp" | "kbd" | "textarea")
                            })
                })
        });
    d.text_ratio = if raw.is_empty() {
        0.0
    } else {
        norm(&text).len() as f32 / raw.len() as f32 * 100.0
    };
    let mut capped = norm(&text);
    if capped.len() > TEXT_CAP {
        let mut cut = TEXT_CAP;
        while !capped.is_char_boundary(cut) {
            cut -= 1;
        }
        capped.truncate(cut);
    }
    // Kept for the whole crawl: drop the spare capacity (the uncapped text can be 2x the cap).
    capped.shrink_to_fit();
    d.text = capped;
    d.links.shrink_to_fit();
    d.images.shrink_to_fit();
    d.rule_probes = crate::rules::eval_probes(probes, &doc, raw);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(html: &str) -> HtmlData {
        parse_html(html, &Url::parse("https://example.com/dir/page").unwrap())
    }

    #[test]
    fn extracts_core_fields() {
        let d = parse(
            r#"<!DOCTYPE html><html lang="en"><head><title> Hello  World </title>
            <meta name="Description" content="A page">
            <link rel="canonical" href="/dir/page">
            <meta name="robots" content="noindex, follow">
            <link rel="alternate" hreflang="de" href="https://example.com/de/">
            </head><body><h1>Main</h1><h3>Skip</h3>
            <a href="other#frag" rel="nofollow">Other page</a>
            <a href="https://ext.com/" target="_blank">ext</a>
            <a href="javascript:void(0)">js</a>
            <img src="/a.png"><img src="/b.png" alt="">
            <p>Lorem ipsum dolor sit amet. Second sentence here!</p></body></html>"#,
        );
        assert_eq!(d.titles, vec!["Hello World"]);
        assert_eq!(d.meta_descriptions, vec!["A page"]);
        assert_eq!(d.canonicals, vec!["https://example.com/dir/page"]);
        assert_eq!(d.hreflang[0].0, "de");
        assert_eq!(d.h1, vec!["Main"]);
        assert_eq!(d.heading_levels, vec![1, 3]);
        assert_eq!(d.links.len(), 2);
        assert_eq!(d.links[0].url, "https://example.com/dir/other");
        assert_eq!(d.links[0].fragment.as_deref(), Some("frag"));
        assert!(d.links[0].nofollow);
        assert!(d.links[1].target_blank && !d.links[1].noopener);
        assert_eq!(d.javascript_links, 1);
        assert_eq!(d.images[0].alt, None);
        assert_eq!(d.images[1].alt.as_deref(), Some(""));
        assert!(d.lorem_ipsum);
        assert!(d.has_doctype);
        assert_eq!(d.lang.as_deref(), Some("en"));
    }

    #[test]
    fn detects_head_problems() {
        let d = parse("<html><head><div>x</div><title>T</title></head><body><title>Again</title></body></html>");
        assert!(d.invalid_in_head.contains(&"div".to_string()));
        assert!(d.title_outside_head);
    }

    #[test]
    fn blocks_without_punctuation_count_as_sentences() {
        let items: String = (0..40)
            .map(|i| format!("<li><a href=\"/p{i}\">Camper van model {i}</a></li>"))
            .collect();
        let d = parse(&format!(
            "<html><body><ul>{items}</ul><p>One sentence here. And another one!</p></body></html>"
        ));
        assert_eq!(d.sentence_count, 42);
        assert!(d.flesch > 30.0, "flesch {}", d.flesch);
    }

    #[test]
    fn lorem_in_code_samples_is_not_placeholder_copy() {
        assert!(!parse("<html><body><pre><code>&lt;p&gt;Lorem ipsum dolor&lt;/p&gt;</code></pre></body></html>").lorem_ipsum);
        assert!(parse("<html><body><p>Lorem ipsum dolor sit amet</p></body></html>").lorem_ipsum);
    }

    #[test]
    fn bookmark_targets_include_named_anchors_and_decoded_fragments() {
        let d = parse(
            r#"<html><body><a name="legacy"></a><h2 id="café">x</h2><a href="/p#caf%C3%A9">y</a></body></html>"#,
        );
        assert!(d.ids.binary_search(&"legacy".to_string()).is_ok());
        assert_eq!(d.links[0].fragment.as_deref(), Some("café"));
    }

    #[test]
    fn font_display_only_matters_for_web_fonts() {
        let local = parse("<html><head><style>@font-face{font-family:F;src:local(Arial);size-adjust:106%}</style></head></html>");
        assert!(!local.font_face_without_display);
        let inline = parse("<html><head><style>@font-face{font-family:S;src:url(\"data:font/woff;base64,AAAA\") format('woff')}</style></head></html>");
        assert!(!inline.font_face_without_display);
        let web = parse(
            "<html><head><style>@font-face{font-family:F;src:url(/f.woff2)}</style></head></html>",
        );
        assert!(web.font_face_without_display);
    }

    #[test]
    fn caps_hostile_nesting() {
        let deep = format!(
            "<html><body>{}<a href=\"/x\">deep</a>{}</body></html>",
            "<div>".repeat(100_000),
            "</div>".repeat(100_000)
        );
        assert_eq!(cap_nesting(&deep).matches("<div>").count(), MAX_DEPTH);
        let d = parse(&deep);
        assert_eq!(d.links.len(), 1);
        assert_eq!(d.word_count, 1);
        let normal = "<html><body><div><p>a<div>b</div></p><!-- <div> --><script>if (a<b) {}</script></div></body></html>";
        assert!(matches!(cap_nesting(normal), Cow::Borrowed(_)));
    }

    #[test]
    fn simhash_and_syllables_match_reference() {
        let words: Vec<String> = "naïve readable little table the cat sat on a mat"
            .split(' ')
            .map(String::from)
            .collect();
        let mut votes = [0i32; 64];
        for w in words.windows(3) {
            let h = fnv64(w.join(" ").as_bytes());
            for (bit, v) in votes.iter_mut().enumerate() {
                *v += if h >> bit & 1 == 1 { 1 } else { -1 };
            }
        }
        let reference =
            votes.iter().enumerate().fold(
                0u64,
                |acc, (bit, v)| if *v > 0 { acc | 1 << bit } else { acc },
            );
        assert_eq!(simhash(&words), reference);
        let counts: Vec<usize> = [
            "table", "little", "the", "make", "naïve", "rhythm", "", "42", "Queue",
        ]
        .iter()
        .map(|w| syllables(w))
        .collect();
        assert_eq!(counts, vec![2, 2, 1, 1, 1, 1, 0, 0, 1]);
        // previous char-vector implementation
        let old = |word: &str| {
            let w: Vec<char> = word
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if w.is_empty() {
                return 0;
            }
            let (mut count, mut prev) = (0, false);
            for &c in &w {
                let v = "aeiouy".contains(c);
                if v && !prev {
                    count += 1;
                }
                prev = v;
            }
            if w.len() > 2 && w.ends_with(&['e']) && !w.ends_with(&['l', 'e']) && count > 1 {
                count -= 1;
            }
            count.max(1)
        };
        for w in
            "Le le ale idle Apple BEE eye ée crème brûlée x-ray re-use oe aye Yule whole café 3e e"
                .split(' ')
        {
            assert_eq!(syllables(w), old(w), "{w}");
        }
    }

    #[test]
    fn simhash_is_similar_for_similar_text() {
        let a: Vec<String> = "the quick brown fox jumps over the lazy dog again and again today"
            .split(' ')
            .map(String::from)
            .collect();
        let mut b = a.clone();
        b.push("tomorrow".into());
        assert!((simhash(&a) ^ simhash(&b)).count_ones() < 16);
    }
}
