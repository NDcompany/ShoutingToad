//! The catalogue of SEO and UX checks. Every entry carries its own test,
//! so the number of checks listed is exactly the number the engine runs.

use crate::audit::{hd, ih, ihd, indexable, pixel_width, self_canonical, Ctx, Kind, Priority};
use crate::model::{HtmlData, Page, Source};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use url::Url;

pub type Test = Box<dyn Fn(&Page, &Ctx<'_>) -> bool + Send + Sync>;

pub struct Check {
    pub id: String,
    pub category: &'static str,
    pub name: String,
    pub kind: Kind,
    pub priority: Priority,
    pub description: String,
    pub fix: String,
    pub test: Test,
}

pub const CATEGORIES: &[&str] = &[
    "Response Codes",
    "Redirects",
    "Security",
    "URL",
    "Page Titles",
    "Meta Description",
    "Meta Keywords",
    "H1",
    "H2",
    "Headings",
    "Content",
    "Images",
    "Canonicals",
    "Pagination",
    "Directives",
    "Robots.txt",
    "Sitemaps",
    "Hreflang",
    "Structured Data",
    "Social",
    "Links",
    "Resources",
    "Performance",
    "Mobile",
    "Accessibility",
    "Validation",
    "AMP",
    "AI Search",
];

fn t<F: Fn(&Page, &Ctx<'_>) -> bool + Send + Sync + 'static>(f: F) -> Test {
    Box::new(f)
}

pub fn catalogue() -> &'static [Check] {
    static C: OnceLock<Vec<Check>> = OnceLock::new();
    C.get_or_init(build)
}

pub fn find(id: &str) -> Option<usize> {
    catalogue().iter().position(|c| c.id == id)
}

macro_rules! add {
    ($v:ident, $id:expr, $cat:expr, $k:ident, $p:ident, $name:expr, $desc:expr, $fix:expr, $f:expr) => {
        $v.push(Check {
            id: $id.into(),
            category: $cat,
            name: $name.into(),
            kind: Kind::$k,
            priority: Priority::$p,
            description: $desc.into(),
            fix: $fix.into(),
            test: t($f),
        })
    };
}

// ---------------------------------------------------------------- helpers

/// Real pages (not robots.txt / sitemap fetches). Sitemap files are recorded at depth 0;
/// pages discovered through a sitemap (depth 1) are real pages.
fn real(p: &Page) -> bool {
    match p.source {
        Some(Source::Robots) => false,
        Some(Source::Sitemap) => p.depth > 0,
        _ => true,
    }
}

fn err_kind(p: &Page) -> &'static str {
    let e = p.error.as_deref().unwrap_or("").to_ascii_lowercase();
    if e.starts_with("timeout") {
        "timeout"
    } else if e.contains("certificate") || e.contains("tls") || e.contains("ssl") {
        "tls"
    } else if e.starts_with("connection failed") || e.contains("dns") {
        "connect"
    } else {
        "other"
    }
}

fn no_response(p: &Page) -> bool {
    p.status == 0 && !p.blocked_by_robots && p.error.is_some()
}

/// Error response or no response. 429 is rate limiting of this crawl, not a broken URL
/// (it is reported on its own under Response Codes).
/// A Cloudflare bot challenge (cf-mitigated: challenge) is shown to crawlers, not visitors.
fn broken(x: &Ctx, url: &str) -> bool {
    x.page(url).is_some_and(|t| {
        !t.blocked_by_robots
            && ((t.status >= 400 && t.status != 429 && t.header("cf-mitigated").is_none())
                || no_response(t))
    })
}

fn redirecting(x: &Ctx, url: &str) -> bool {
    x.page(url).is_some_and(|t| t.is_redirect())
}

fn host(u: &str) -> String {
    Url::parse(u)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default()
}

fn bare_host(u: &str) -> String {
    host(u).trim_start_matches("www.").to_string()
}

fn path_of(u: &str) -> String {
    Url::parse(u)
        .map(|u| u.path().to_string())
        .unwrap_or_default()
}

fn query_keys(u: &str) -> Vec<String> {
    Url::parse(u)
        .map(|u| {
            u.query_pairs()
                .map(|(k, _)| k.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn first(v: &[String]) -> &str {
    v.first().map(|s| s.as_str()).unwrap_or("")
}

fn all_caps(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 8 && letters.iter().all(|c| !c.is_lowercase())
}

fn stuffed(s: &str) -> bool {
    let mut counts: HashMap<String, usize> = HashMap::new();
    const STOP: &[&str] = &[
        "what", "that", "this", "with", "from", "your", "have", "will", "they", "them", "their",
        "there", "when", "which", "were", "been", "than", "then", "into", "more", "most", "also",
        "just", "only", "over", "some", "such", "about", "each", "other", "these", "those", "does",
        "here", "where", "while", "would", "could", "should",
    ];
    for w in s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
    {
        let w = w.to_lowercase();
        if !STOP.contains(&w.as_str()) {
            *counts.entry(w).or_insert(0) += 1;
        }
    }
    counts.values().any(|&c| c >= 3)
}

fn placeholder(s: &str) -> bool {
    let l = s.to_lowercase();
    [
        "lorem ipsum",
        "title goes here",
        "description goes here",
        "your title here",
        "enter description",
        "{{",
        "}}",
        "todo:",
        "untitled document",
    ]
    .iter()
    .any(|p| l.contains(p))
}

fn double_encoded(s: &str) -> bool {
    ["&amp;", "&quot;", "&#", "&nbsp;", "&lt;", "&gt;", "&apos;"]
        .iter()
        .any(|e| s.contains(e))
}

/// File name of an image without extension, size/density suffixes and theme
/// variants, so logo-light-96.webp, logo-dark-96.webp and logo.png?width=300 all
/// count as the same picture.
fn image_stem(u: &str) -> String {
    const VARIANT: &[&str] = &[
        "light", "dark", "white", "black", "mobile", "desktop", "retina", "small", "large",
        "thumb", "sm", "md", "lg", "xl",
    ];
    let path = path_of(u);
    let name = path.rsplit('/').next().unwrap_or("");
    let stem = name
        .rsplit_once('.')
        .map(|(n, _)| n)
        .unwrap_or(name)
        .to_ascii_lowercase();
    let s = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty() && !VARIANT.contains(t))
        .filter(|t| {
            !t.trim_end_matches('x')
                .chars()
                .all(|c| c.is_ascii_digit() || c == 'x')
        })
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        path
    } else {
        s
    }
}

fn header_has(p: &Page, name: &str) -> bool {
    p.header(name).is_some()
}

fn cookies(p: &Page) -> Vec<String> {
    p.headers_all("set-cookie")
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Cookie (lower-cased Set-Cookie value) carries attribute `name`; the name=value pair itself is skipped.
fn cookie_attr(c: &str, name: &str) -> bool {
    c.split(';')
        .skip(1)
        .any(|a| a.split('=').next().unwrap_or("").trim() == name)
}

fn csp(p: &Page) -> String {
    p.header("content-security-policy")
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn max_age(v: &str) -> Option<u64> {
    let l = v.to_ascii_lowercase();
    let i = l.find("max-age=")?;
    l[i + 8..]
        .trim_start_matches('"')
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn is_static(p: &Page) -> bool {
    p.is_internal && p.status == 200 && matches!(p.kind(), "Image" | "CSS" | "JavaScript")
}

fn english(h: &HtmlData) -> bool {
    h.lang
        .as_deref()
        .is_none_or(|l| l.to_ascii_lowercase().starts_with("en"))
}

fn charset_norm(s: &str) -> String {
    s.trim()
        .trim_matches(['"', '\''])
        .to_ascii_lowercase()
        .replace('_', "-")
}

fn header_charset(p: &Page) -> Option<String> {
    p.content_type.split(';').map(|s| s.trim()).find_map(|s| {
        s.to_ascii_lowercase()
            .strip_prefix("charset=")
            .map(charset_norm)
    })
}

fn private_host(h: &str) -> bool {
    if h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".test")
        || h == "0.0.0.0"
    {
        return true;
    }
    match h.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback(),
        Err(_) => false,
    }
}

fn link_canonical_header(p: &Page) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"<([^>]+)>\s*;[^,]*rel\s*=\s*"?canonical"?"#).unwrap());
    let base = Url::parse(&p.url).ok()?;
    p.headers_all("link")
        .find_map(|v| re.captures(v).map(|c| c[1].to_string()))
        .and_then(|u| base.join(&u).ok())
        .map(|u| u.to_string())
}

/// Value of a `name:value` robots directive such as max-snippet:50.
fn directive_value(p: &Page, name: &str) -> Option<String> {
    p.robots_directives()
        .split([',', ';'])
        .filter_map(|t| t.trim().split_once(':'))
        .find_map(|(k, v)| {
            let k = k.trim();
            // also handle "googlebot: max-snippet:20" style tokens
            if k == name {
                Some(v.trim().to_string())
            } else {
                v.trim()
                    .strip_prefix(&format!("{name}:"))
                    .map(|x| x.trim().to_string())
            }
        })
}

fn og<'a>(h: &'a HtmlData, key: &str) -> Option<&'a str> {
    h.og.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn tw<'a>(h: &'a HtmlData, key: &str) -> Option<&'a str> {
    h.twitter
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

// ---- language codes (ISO 639-1 and ISO 3166-1 alpha-2)
const LANGS: &str = "aa ab ae af ak am an ar as av ay az ba be bg bh bi bm bn bo br bs ca ce ch co cr cs cu cv cy da de dv dz ee el en eo es et eu fa ff fi fj fo fr fy ga gd gl gn gu gv ha he hi ho hr ht hu hy hz ia id ie ig ii ik io is it iu ja jv ka kg ki kj kk kl km kn ko kr ks ku kv kw ky la lb lg li ln lo lt lu lv mg mh mi mk ml mn mr ms mt my na nb nd ne ng nl nn no nr nv ny oc oj om or os pa pi pl ps pt qu rm rn ro ru rw sa sc sd se sg si sk sl sm sn so sq sr ss st su sv sw ta te tg th ti tk tl tn to tr ts tt tw ty ug uk ur uz ve vi vo wa wo xh yi yo za zh zu";
const REGIONS: &str = "ad ae af ag ai al am ao aq ar as at au aw ax az ba bb bd be bf bg bh bi bj bl bm bn bo bq br bs bt bv bw by bz ca cc cd cf cg ch ci ck cl cm cn co cr cu cv cw cx cy cz de dj dk dm do dz ec ee eg eh er es et fi fj fk fm fo fr ga gb gd ge gf gg gh gi gl gm gn gp gq gr gs gt gu gw gy hk hm hn hr ht hu id ie il im in io iq ir is it je jm jo jp ke kg kh ki km kn kp kr kw ky kz la lb lc li lk lr ls lt lu lv ly ma mc md me mf mg mh mk ml mm mn mo mp mq mr ms mt mu mv mw mx my mz na nc ne nf ng ni nl no np nr nu nz om pa pe pf pg ph pk pl pm pn pr ps pt pw py qa re ro rs ru rw sa sb sc sd se sg sh si sj sk sl sm sn so sr ss st sv sx sy sz tc td tf tg th tj tk tl tm tn to tr tt tv tw tz ua ug um us uy uz va vc ve vg vi vn vu wf ws ye yt za zm zw";

fn lang_set() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| LANGS.split(' ').collect())
}

fn region_set() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| REGIONS.split(' ').collect())
}

/// Google hreflang: ISO 639-1 language, optional script, optional ISO 3166-1 region, or x-default.
pub fn valid_hreflang(code: &str) -> bool {
    let c = code.trim().to_ascii_lowercase();
    if c == "x-default" {
        return true;
    }
    let parts: Vec<&str> = c.split('-').collect();
    if parts.is_empty() || parts.len() > 3 || !lang_set().contains(parts[0]) {
        return false;
    }
    match parts.len() {
        1 => true,
        2 => {
            region_set().contains(parts[1])
                || (parts[1].len() == 4 && parts[1].chars().all(|c| c.is_ascii_alphabetic()))
        }
        _ => {
            parts[1].len() == 4
                && parts[1].chars().all(|c| c.is_ascii_alphabetic())
                && region_set().contains(parts[2])
        }
    }
}

/// BCP 47 sanity check for `<html lang>`.
pub fn valid_html_lang(code: &str) -> bool {
    let c = code.trim().to_ascii_lowercase().replace('_', "-");
    let parts: Vec<&str> = c.split('-').collect();
    let Some(primary) = parts.first() else {
        return false;
    };
    let primary_ok = match primary.len() {
        2 => lang_set().contains(primary),
        3 => primary.chars().all(|c| c.is_ascii_alphabetic()),
        _ => false,
    };
    primary_ok
        && parts[1..]
            .iter()
            .all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn lang_primary(code: &str) -> String {
    code.trim()
        .to_ascii_lowercase()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_string()
}

// ---- structured data helpers
fn sd_items(h: &HtmlData) -> Vec<&Value> {
    fn walk<'v>(v: &'v Value, out: &mut Vec<&'v Value>) {
        match v {
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => {
                if o.contains_key("@type") {
                    out.push(v);
                }
                o.values().for_each(|x| walk(x, out));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    h.json_ld.iter().for_each(|v| walk(v, &mut out));
    out
}

/// Top-level JSON-LD entities (array items and @graph members).
fn sd_roots(h: &HtmlData) -> Vec<&Value> {
    let mut out = Vec::new();
    for v in &h.json_ld {
        let items: Vec<&Value> = match v {
            Value::Array(a) => a.iter().collect(),
            other => vec![other],
        };
        for i in items {
            match i.get("@graph").and_then(|g| g.as_array()) {
                Some(g) => out.extend(g.iter()),
                None => out.push(i),
            }
        }
    }
    out
}

fn type_names(v: &Value) -> Vec<String> {
    let norm = |s: &str| s.rsplit(['/', ':']).next().unwrap_or(s).to_string();
    match v.get("@type") {
        Some(Value::String(s)) => vec![norm(s)],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(norm).collect(),
        _ => vec![],
    }
}

fn sd_present(v: &Value, prop: &str) -> bool {
    match v.get(prop) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(_) => true,
    }
}

/// A bare node reference such as {"@type": "Organization", "@id": "…/#org"}: the
/// entity is described elsewhere, so missing properties here are not errors.
fn sd_ref(v: &Value) -> bool {
    v.get("@id").is_some()
        && v.as_object()
            .is_some_and(|o| o.keys().all(|k| k.starts_with('@')))
}

/// `prop` is set on `v` or on another node of the page with the same @id.
fn sd_has(items: &[&Value], v: &Value, prop: &str) -> bool {
    sd_present(v, prop)
        || v.get("@id").and_then(|i| i.as_str()).is_some_and(|id| {
            items
                .iter()
                .any(|o| o.get("@id").and_then(|x| x.as_str()) == Some(id) && sd_present(o, prop))
        })
}

/// Offers that belong to a Product (its `offers` property).
fn product_offers<'v>(items: &[&'v Value]) -> Vec<&'v Value> {
    items
        .iter()
        .filter(|i| {
            type_names(i)
                .iter()
                .any(|t| t == "Product" || t == "ProductGroup")
        })
        .filter_map(|i| i.get("offers"))
        .flat_map(|o| match o {
            Value::Array(a) => a.iter().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

fn valid_iso_date(s: &str) -> bool {
    let s = s.trim();
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
        || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
        || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
        || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").is_ok()
        || chrono::DateTime::parse_from_str(&s.replace('Z', "+00:00"), "%Y-%m-%dT%H:%M%:z").is_ok()
}

// ---------------------------------------------------------------- catalogue

mod ai_search;

/// Almost no text in the HTML but heavy scripts: the content probably appears only after
/// JavaScript runs.
fn js_rendered(h: &HtmlData) -> bool {
    h.word_count < 50 && (h.scripts.len() >= 3 || h.inline_script_bytes > 30_000)
}

fn build() -> Vec<Check> {
    let mut v: Vec<Check> = Vec::with_capacity(460);
    response_codes(&mut v);
    redirects(&mut v);
    security(&mut v);
    urls(&mut v);
    titles(&mut v);
    descriptions(&mut v);
    headings(&mut v);
    content(&mut v);
    images(&mut v);
    canonicals(&mut v);
    pagination(&mut v);
    directives(&mut v);
    robots_and_sitemaps(&mut v);
    hreflang(&mut v);
    structured_data(&mut v);
    social(&mut v);
    links(&mut v);
    resources(&mut v);
    performance(&mut v);
    mobile_accessibility(&mut v);
    validation(&mut v);
    ai_search::ai_search(&mut v);
    v
}

fn response_codes(v: &mut Vec<Check>) {
    const CAT: &str = "Response Codes";
    add!(v, "response.internal_blocked_robots", CAT, Warning, High, "Internal Blocked by Robots.txt",
        "Internal URLs that robots.txt disallows. Search engines cannot crawl them, so their content and links are invisible.",
        "Check the URLs should really be blocked. Use noindex instead of robots.txt when the goal is to keep pages out of the index.",
        |p, _| p.is_internal && p.blocked_by_robots && matches!(p.source, Some(Source::Link) | Some(Source::Seed) | Some(Source::Redirect) | Some(Source::Sitemap) | Some(Source::Canonical) | Some(Source::Hreflang) | Some(Source::Pagination) | Some(Source::Amp)));
    add!(v, "response.internal_blocked_resource", CAT, Warning, Medium, "Internal Blocked Resource",
        "Images, CSS or JavaScript files disallowed by robots.txt. Search engines cannot render the page the way visitors see it.",
        "Allow crawling of CSS, JavaScript and image paths in robots.txt.",
        |p, _| p.is_internal && p.blocked_by_robots && matches!(p.source, Some(Source::Image) | Some(Source::Script) | Some(Source::Stylesheet)));

    let errors: [(&str, &str, &str); 4] = [
        ("timeout", "Timeout", "The server did not respond within the timeout."),
        ("connect", "Connection Failed", "The host could not be reached (DNS failure, refused connection or network error)."),
        ("tls", "SSL/TLS Error", "The secure connection failed, often because of an invalid, expired or mismatched certificate."),
        ("other", "Other Error", "The request failed for another reason before a response arrived."),
    ];
    let codes: [(u16, &str, Kind, Priority, &str, &str); 17] = [
        (301, "Moved Permanently", Kind::Warning, Priority::Low, "Permanent redirects. Links pointing at them waste crawl budget and add latency.", "Update links to point straight at the final URL."),
        (302, "Found (Temporary Redirect)", Kind::Warning, Priority::Medium, "Temporary redirects. Search engines may keep the old URL indexed.", "Use a 301 or 308 if the move is permanent, and update links to the final URL."),
        (303, "See Other", Kind::Warning, Priority::Medium, "303 redirects are meant for form responses, not for page moves.", "Use a 301 for permanent moves and link straight to the destination."),
        (307, "Temporary Redirect", Kind::Warning, Priority::Medium, "Temporary redirects that keep the request method. Search engines may keep the old URL indexed.", "Use a 308 or 301 if the move is permanent, and update links."),
        (308, "Permanent Redirect", Kind::Warning, Priority::Low, "Permanent redirects. Links pointing at them waste crawl budget and add latency.", "Update links to point straight at the final URL."),
        (400, "Bad Request", Kind::Issue, Priority::High, "The server rejected the request as malformed, often a broken URL or bad encoding.", "Fix the URL that links here or the server routing that rejects it."),
        (401, "Unauthorized", Kind::Warning, Priority::Medium, "The URL needs authentication. Search engines cannot see it.", "Remove links to protected URLs from public pages, or make the content public if it should rank."),
        (403, "Forbidden", Kind::Issue, Priority::High, "The server refuses to serve the URL. Firewalls and bot protection often cause this.", "Allow the URL, or remove links to it. Check firewall rules are not blocking crawlers."),
        (404, "Not Found", Kind::Issue, Priority::High, "Broken URLs. Visitors hit a dead end and link equity is lost.", "Restore the page, 301-redirect it to the closest relevant page, or remove links to it."),
        (405, "Method Not Allowed", Kind::Issue, Priority::Medium, "The server does not accept GET for this URL.", "Configure the server to answer GET requests, or remove the link."),
        (406, "Not Acceptable", Kind::Issue, Priority::Medium, "The server refused based on request headers, often an aggressive firewall.", "Relax content negotiation or firewall rules so normal browsers and bots receive the page."),
        (408, "Request Timeout", Kind::Issue, Priority::Medium, "The server timed out waiting for the request.", "Investigate server capacity and timeouts."),
        (410, "Gone", Kind::Issue, Priority::Medium, "The URL was deliberately removed, but links still point at it.", "Remove or update the links pointing to removed URLs."),
        (429, "Too Many Requests", Kind::Warning, Priority::High, "The server is rate limiting. Search engine crawlers get throttled the same way.", "Raise rate limits for verified crawlers, or slow this crawl with a crawl delay."),
        (451, "Unavailable For Legal Reasons", Kind::Warning, Priority::Medium, "The URL is blocked for legal reasons.", "Make sure this is intended and remove internal links if so."),
        (500, "Internal Server Error", Kind::Issue, Priority::High, "The server crashed producing the page.", "Check server logs and fix the application error."),
        (503, "Service Unavailable", Kind::Issue, Priority::High, "The server is overloaded or in maintenance. Long-running 503s get pages dropped.", "Restore capacity; only use 503 briefly during maintenance."),
    ];
    let more5xx: [(u16, &str, &str); 3] = [
        (
            501,
            "Not Implemented",
            "The server does not support the request method.",
        ),
        (
            502,
            "Bad Gateway",
            "A proxy or CDN got an invalid response from the origin server.",
        ),
        (
            504,
            "Gateway Timeout",
            "A proxy or CDN timed out waiting for the origin server.",
        ),
    ];
    for (scope, internal) in [("Internal", true), ("External", false)] {
        let sid = scope.to_lowercase();
        let soften = |k: Kind| {
            if internal {
                k
            } else if k == Kind::Issue {
                Kind::Warning
            } else {
                k
            }
        };
        let lower = |p: Priority| {
            if internal {
                p
            } else {
                match p {
                    Priority::High => Priority::Medium,
                    _ => Priority::Low,
                }
            }
        };
        for (key, label, desc) in errors {
            let fix = if internal {
                "Check the server is up and reachable, and that DNS and certificates are valid."
            } else {
                "Check the external site; update or remove the link if it stays unreachable."
            };
            v.push(Check {
                id: format!("response.{sid}_no_response_{key}"),
                category: CAT,
                name: format!("{scope} No Response: {label}"),
                kind: soften(Kind::Issue),
                priority: lower(Priority::High),
                description: desc.to_string(),
                fix: fix.to_string(),
                test: t(move |p, _| {
                    real(p) && p.is_internal == internal && no_response(p) && err_kind(p) == key
                }),
            });
        }
        for (code, label, kind, pri, desc, fix) in codes {
            v.push(Check {
                id: format!("response.{sid}_{code}"),
                category: CAT,
                name: format!("{scope} {code} {label}"),
                kind: soften(kind),
                priority: lower(pri),
                description: desc.to_string(),
                fix: fix.to_string(),
                test: t(move |p, _| real(p) && p.is_internal == internal && p.status == code),
            });
        }
        for (code, label, desc) in more5xx {
            v.push(Check {
                id: format!("response.{sid}_{code}"),
                category: CAT,
                name: format!("{scope} {code} {label}"),
                kind: soften(Kind::Issue),
                priority: lower(Priority::High),
                description: desc.to_string(),
                fix: "Check the server, proxy and CDN configuration and logs.".to_string(),
                test: t(move |p, _| real(p) && p.is_internal == internal && p.status == code),
            });
        }
        v.push(Check {
            id: format!("response.{sid}_other_3xx"),
            category: CAT,
            name: format!("{scope} Other Redirection (3xx)"),
            kind: Kind::Warning,
            priority: lower(Priority::Medium),
            description: "Uncommon 3xx responses (300, 304, 305, 306) that browsers and crawlers handle inconsistently.".into(),
            fix: "Return 200 for the content or a 301 to the final URL.".into(),
            test: t(move |p, _| real(p) && p.is_internal == internal && p.is_redirect() && ![301, 302, 303, 307, 308].contains(&p.status)),
        });
        v.push(Check {
            id: format!("response.{sid}_other_4xx"),
            category: CAT,
            name: format!("{scope} Other Client Error (4xx)"),
            kind: soften(Kind::Issue),
            priority: lower(Priority::Medium),
            description: "Less common 4xx responses that stop the page being served.".into(),
            fix: "Fix the URL or remove links to it.".into(),
            test: t(move |p, _| {
                real(p)
                    && p.is_internal == internal
                    && (400..500).contains(&p.status)
                    && ![400, 401, 403, 404, 405, 406, 408, 410, 429, 451].contains(&p.status)
            }),
        });
        v.push(Check {
            id: format!("response.{sid}_other_5xx"),
            category: CAT,
            name: format!("{scope} Other Server Error (5xx)"),
            kind: soften(Kind::Issue),
            priority: lower(Priority::High),
            description: "Less common 5xx server errors.".into(),
            fix: "Check server logs and fix the error.".into(),
            test: t(move |p, _| {
                real(p)
                    && p.is_internal == internal
                    && p.status >= 500
                    && ![500, 501, 502, 503, 504].contains(&p.status)
            }),
        });
    }
}

fn redirects(v: &mut Vec<Check>) {
    const CAT: &str = "Redirects";
    let ir = |p: &Page| p.is_internal && p.is_redirect() && real(p);
    add!(v, "redirects.chain", CAT, Issue, Medium, "Redirect Chain",
        "The URL redirects more than once before reaching its destination. Every hop adds latency and loses crawl efficiency.",
        "Point the first redirect straight at the final URL and update links.",
        move |p, x| ir(p) && { let (_, hops, looped) = x.follow(&p.url); hops >= 2 && !looped });
    add!(v, "redirects.chain_too_long", CAT, Issue, High, "Redirect Chain Over 5 Hops",
        "Googlebot stops following after about 10 hops and may give up sooner. Long chains risk the destination never being indexed.",
        "Collapse the chain into a single redirect.",
        move |p, x| ir(p) && { let (_, hops, looped) = x.follow(&p.url); hops > 5 && !looped });
    add!(
        v,
        "redirects.loop",
        CAT,
        Issue,
        High,
        "Redirect Loop",
        "The redirects lead back to a URL already in the chain, so the page never loads.",
        "Fix the redirect rules so they end at a URL that returns 200.",
        move |p, x| ir(p) && x.follow(&p.url).2
    );
    add!(
        v,
        "redirects.to_4xx",
        CAT,
        Issue,
        High,
        "Redirect to Client Error (4xx)",
        "The redirect ends at a broken URL.",
        "Redirect to a live, relevant page instead.",
        move |p, x| ir(p)
            && x.final_status(&p.url)
                .is_some_and(|s| (400..500).contains(&s))
    );
    add!(
        v,
        "redirects.to_5xx",
        CAT,
        Issue,
        High,
        "Redirect to Server Error (5xx)",
        "The redirect ends at a URL that returns a server error.",
        "Fix the destination or redirect elsewhere.",
        move |p, x| ir(p) && x.final_status(&p.url).is_some_and(|s| s >= 500)
    );
    add!(
        v,
        "redirects.to_no_response",
        CAT,
        Issue,
        High,
        "Redirect to No Response",
        "The redirect ends at a URL that did not respond.",
        "Point the redirect at a working URL.",
        move |p, x| ir(p) && matches!(x.follow(&p.url), (Some(f), _, false) if no_response(f))
    );
    add!(v, "redirects.to_non_indexable", CAT, Warning, Medium, "Redirect to Non-Indexable Page",
        "The redirect ends at a noindex or canonicalised page, so link value flows to a page that will not rank.",
        "Redirect to the indexable version of the content.",
        move |p, x| ir(p) && matches!(x.follow(&p.url), (Some(f), _, false) if ih(f) && !indexable(f)));
    add!(v, "redirects.external", CAT, Warning, Low, "Redirect to External Domain",
        "Internal URLs that redirect to another domain. Sometimes intended, but often a leftover from a migration or a hijacked link.",
        "Confirm each off-site redirect is intended.",
        move |p, _| ir(p) && p.redirect_to.as_deref().is_some_and(|t| bare_host(t) != bare_host(&p.url)));
    add!(v, "redirects.https_to_http", CAT, Issue, High, "HTTPS to HTTP Redirect",
        "A secure URL redirects to an insecure one. Browsers warn, and the signal undermines HTTPS.",
        "Redirect to the HTTPS version of the destination.",
        move |p, _| ir(p) && p.is_https() && p.redirect_to.as_deref().is_some_and(|t| t.starts_with("http://")));
    add!(
        v,
        "redirects.missing_location",
        CAT,
        Issue,
        High,
        "Redirect Without Location Header",
        "A 3xx response has no Location header, so there is nowhere to go.",
        "Send a Location header with the destination URL.",
        move |p, _| ir(p) && p.redirect_to.is_none()
    );
    add!(v, "redirects.meta_refresh", CAT, Warning, Medium, "Meta Refresh Redirect",
        "The page redirects with <meta http-equiv=\"refresh\">. It is slower, worse for accessibility, and a weaker signal than a server redirect.",
        "Replace it with a 301 server-side redirect.",
        |p, _| hd(p).and_then(|h| h.meta_refresh.as_deref()).is_some_and(|r| r.to_ascii_lowercase().contains("url=")));
    add!(v, "redirects.meta_refresh_reload", CAT, Warning, Low, "Meta Refresh Page Reload",
        "The page reloads itself on a timer, which disorients screen-reader users and wastes resources.",
        "Remove the auto-refresh, or use JavaScript updates for live content.",
        |p, _| hd(p).and_then(|h| h.meta_refresh.as_deref()).is_some_and(|r| !r.to_ascii_lowercase().contains("url=")));
    add!(v, "redirects.javascript", CAT, Warning, Medium, "JavaScript Redirect",
        "Inline script changes window.location. Search engines only see it after rendering, and may not pass signals reliably.",
        "Use a server-side 301 redirect instead.",
        |p, _| hd(p).is_some_and(|h| h.js_redirect));
}

fn security(v: &mut Vec<Check>) {
    const CAT: &str = "Security";
    let html_https = |p: &Page| ih(p) && p.is_https();
    add!(v, "security.http_url", CAT, Issue, High, "HTTP URLs",
        "Internal pages served without encryption. Browsers flag them as not secure, and HTTPS is a ranking signal.",
        "Serve the site over HTTPS and 301-redirect every HTTP URL.",
        |p, _| ih(p) && p.url.starts_with("http://"));
    add!(v, "security.mixed_content", CAT, Issue, High, "Mixed Content",
        "HTTPS pages that load images, scripts, stylesheets or iframes over HTTP. Browsers block or warn about them.",
        "Load every resource over HTTPS.",
        move |p, _| html_https(p) && hd(p).is_some_and(|h| {
            h.images.iter().map(|i| &i.url).chain(h.scripts.iter().map(|s| &s.url)).chain(h.stylesheets.iter()).chain(h.iframes.iter().map(|f| &f.url)).any(|u| u.starts_with("http://"))
        }));
    add!(
        v,
        "security.form_http_action",
        CAT,
        Issue,
        High,
        "Form Submits Over HTTP",
        "A form sends its data to an http:// URL, exposing what visitors type.",
        "Point form actions at HTTPS URLs.",
        |p, _| hd(p).is_some_and(|h| h.forms.iter().any(|f| f.action.starts_with("http://")))
    );
    add!(
        v,
        "security.password_on_http",
        CAT,
        Issue,
        High,
        "Password Field on HTTP Page",
        "A password input on an unencrypted page. Browsers show a not-secure warning.",
        "Serve login pages over HTTPS only.",
        |p, _| {
            hd(p).is_some_and(|h| {
                p.url.starts_with("http://") && h.forms.iter().any(|f| f.has_password)
            })
        }
    );
    add!(v, "security.missing_hsts", CAT, Warning, Medium, "Missing HSTS Header",
        "No Strict-Transport-Security header, so browsers may still try HTTP first and allow downgrade attacks.",
        "Send Strict-Transport-Security: max-age=31536000; includeSubDomains.",
        move |p, _| html_https(p) && !header_has(p, "strict-transport-security"));
    add!(
        v,
        "security.hsts_short",
        CAT,
        Warning,
        Low,
        "HSTS Max-Age Under One Year",
        "The HSTS max-age is shorter than the 31536000 seconds needed for preload lists.",
        "Raise max-age to at least 31536000.",
        move |p, _| html_https(p)
            && p.header("strict-transport-security")
                .and_then(max_age)
                .is_some_and(|a| a < 31_536_000)
    );
    add!(
        v,
        "security.hsts_no_subdomains",
        CAT,
        Opportunity,
        Low,
        "HSTS Without includeSubDomains",
        "HSTS does not cover subdomains, so they can still be reached over HTTP.",
        "Add includeSubDomains once every subdomain supports HTTPS.",
        move |p, _| html_https(p)
            && p.header("strict-transport-security")
                .is_some_and(|h| !h.to_ascii_lowercase().contains("includesubdomains"))
    );
    add!(v, "security.missing_csp", CAT, Warning, Low, "Missing Content-Security-Policy",
        "No Content-Security-Policy header to limit which scripts and resources may run. It is a key defence against XSS.",
        "Add a Content-Security-Policy header, starting in report-only mode.",
        |p, _| ih(p) && !header_has(p, "content-security-policy"));
    add!(
        v,
        "security.csp_unsafe_inline",
        CAT,
        Warning,
        Low,
        "CSP Allows unsafe-inline",
        "The policy permits inline scripts, which removes most of its XSS protection.",
        "Use nonces or hashes instead of 'unsafe-inline'.",
        |p, _| ih(p) && csp(p).contains("'unsafe-inline'")
    );
    add!(
        v,
        "security.csp_unsafe_eval",
        CAT,
        Warning,
        Low,
        "CSP Allows unsafe-eval",
        "The policy permits eval(), which lets injected strings run as code.",
        "Remove 'unsafe-eval' and the code that depends on it.",
        |p, _| ih(p) && csp(p).contains("'unsafe-eval'")
    );
    add!(v, "security.missing_nosniff", CAT, Warning, Low, "Missing X-Content-Type-Options",
        "Without X-Content-Type-Options: nosniff, browsers may guess content types and run files as scripts.",
        "Send X-Content-Type-Options: nosniff.",
        |p, _| ih(p) && !header_has(p, "x-content-type-options"));
    add!(v, "security.missing_frame_protection", CAT, Warning, Low, "Missing Clickjacking Protection",
        "Neither X-Frame-Options nor a CSP frame-ancestors rule stops other sites framing this page.",
        "Send X-Frame-Options: SAMEORIGIN or a frame-ancestors directive.",
        |p, _| ih(p) && !header_has(p, "x-frame-options") && !csp(p).contains("frame-ancestors"));
    add!(
        v,
        "security.missing_referrer_policy",
        CAT,
        Warning,
        Low,
        "Missing Referrer-Policy",
        "No Referrer-Policy header to control how much of the URL leaks to other sites.",
        "Send Referrer-Policy: strict-origin-when-cross-origin.",
        |p, _| ih(p) && !header_has(p, "referrer-policy")
    );
    add!(v, "security.unsafe_referrer_policy", CAT, Warning, Low, "Unsafe Referrer-Policy",
        "Referrer-Policy: unsafe-url leaks full URLs, including query strings, to every site linked from the page.",
        "Use strict-origin-when-cross-origin instead.",
        |p, _| ih(p) && p.header("referrer-policy").is_some_and(|r| r.to_ascii_lowercase().contains("unsafe-url")));
    add!(v, "security.missing_permissions_policy", CAT, Opportunity, Low, "Missing Permissions-Policy",
        "No Permissions-Policy header to disable browser features (camera, geolocation and so on) the site does not use.",
        "Send a Permissions-Policy that turns off unused features.",
        |p, _| ih(p) && !header_has(p, "permissions-policy"));
    add!(
        v,
        "security.missing_coop",
        CAT,
        Opportunity,
        Low,
        "Missing Cross-Origin-Opener-Policy",
        "Without COOP, pages opened from other origins can keep a handle on this window.",
        "Send Cross-Origin-Opener-Policy: same-origin where possible.",
        |p, _| ih(p) && !header_has(p, "cross-origin-opener-policy")
    );
    add!(v, "security.unsafe_cross_origin_links", CAT, Warning, Low, "Unsafe Cross-Origin Links",
        "External links open in a new tab without rel=\"noopener\". Older browsers let the opened page control this one.",
        "Add rel=\"noopener\" (or noreferrer) to target=\"_blank\" links.",
        |p, _| hd(p).is_some_and(|h| { let me = host(&p.url); h.links.iter().any(|l| l.target_blank && !l.noopener && host(&l.url) != me) }));
    add!(v, "security.protocol_relative_links", CAT, Warning, Low, "Protocol-Relative Links",
        "Links that start with //. On HTTP pages they load over HTTP, and they are an outdated pattern.",
        "Use explicit https:// URLs.",
        |p, _| hd(p).is_some_and(|h| h.links.iter().any(|l| l.raw_href.trim_start().starts_with("//"))));
    add!(
        v,
        "security.server_version",
        CAT,
        Warning,
        Low,
        "Server Header Reveals Version",
        "The Server header names software and version, which helps attackers find known exploits.",
        "Remove version numbers from the Server header.",
        |p, _| ih(p)
            && p.header("server")
                .is_some_and(|s| s.contains('/') && s.chars().any(|c| c.is_ascii_digit()))
    );
    add!(
        v,
        "security.powered_by",
        CAT,
        Warning,
        Low,
        "X-Powered-By Header Present",
        "The X-Powered-By header reveals the technology stack.",
        "Remove the X-Powered-By header.",
        |p, _| ih(p) && header_has(p, "x-powered-by")
    );
    add!(
        v,
        "security.cookie_not_secure",
        CAT,
        Warning,
        Medium,
        "Cookie Without Secure Flag",
        "Cookies set on HTTPS pages without the Secure flag can be sent over plain HTTP.",
        "Add the Secure attribute to every cookie.",
        |p, _| ih(p) && p.is_https() && cookies(p).iter().any(|c| !cookie_attr(c, "secure"))
    );
    add!(
        v,
        "security.cookie_not_httponly",
        CAT,
        Warning,
        Low,
        "Cookie Without HttpOnly Flag",
        "Cookies readable from JavaScript can be stolen through XSS.",
        "Add HttpOnly to cookies that scripts do not need.",
        |p, _| ih(p) && cookies(p).iter().any(|c| !cookie_attr(c, "httponly"))
    );
    add!(
        v,
        "security.cookie_no_samesite",
        CAT,
        Warning,
        Low,
        "Cookie Without SameSite",
        "Cookies without a SameSite attribute are more exposed to cross-site request forgery.",
        "Set SameSite=Lax (or Strict) on cookies.",
        |p, _| ih(p) && cookies(p).iter().any(|c| !cookie_attr(c, "samesite"))
    );
    add!(v, "security.http_not_redirected", CAT, Issue, High, "HTTP Version Not Redirected to HTTPS",
        "The http:// version of the site answers without redirecting to https://, creating duplicate, insecure copies.",
        "301-redirect all HTTP requests to HTTPS.",
        |p, x| x.is_start(p) && x.crawl.site.http_redirects_to_https == Some(false));
    add!(v, "security.cors_wildcard", CAT, Warning, Low, "Wildcard CORS on HTML Pages",
        "Access-Control-Allow-Origin: * lets any website read these pages from a visitor's browser.",
        "Restrict CORS to the origins that need it.",
        |p, _| ih(p) && p.header("access-control-allow-origin").is_some_and(|v| v.trim() == "*"));
}

fn urls(v: &mut Vec<Check>) {
    const CAT: &str = "URL";
    static NON_ASCII: OnceLock<Regex> = OnceLock::new();
    add!(
        v,
        "url.uppercase",
        CAT,
        Warning,
        Low,
        "Uppercase Characters",
        "URLs are case-sensitive. Mixed case invites duplicate URLs and broken links.",
        "Use lower-case URLs and redirect upper-case variants.",
        |p, _| ih(p) && path_of(&p.url).chars().any(|c| c.is_ascii_uppercase())
    );
    add!(
        v,
        "url.underscores",
        CAT,
        Opportunity,
        Low,
        "Underscores",
        "Search engines treat hyphens as word separators but not underscores.",
        "Use hyphens to separate words in URLs.",
        |p, _| ih(p) && path_of(&p.url).contains('_')
    );
    add!(v, "url.non_ascii", CAT, Warning, Low, "Non-ASCII Characters",
        "URLs with non-ASCII characters are percent-encoded, which makes them long and error-prone to share.",
        "Use ASCII slugs, transliterating characters where needed.",
        |p, _| ih(p) && NON_ASCII.get_or_init(|| Regex::new(r"%[89A-Fa-f][0-9A-Fa-f]").unwrap()).is_match(&p.url));
    add!(
        v,
        "url.spaces",
        CAT,
        Warning,
        Low,
        "Spaces",
        "Encoded spaces (%20) make URLs fragile when copied and shared.",
        "Replace spaces with hyphens.",
        |p, _| ih(p) && p.url.contains("%20")
    );
    add!(
        v,
        "url.multiple_slashes",
        CAT,
        Warning,
        Low,
        "Multiple Slashes",
        "Double slashes in the path usually come from a template bug and create duplicate URLs.",
        "Fix link generation and redirect // paths.",
        |p, _| ih(p) && path_of(&p.url).contains("//")
    );
    add!(v, "url.repetitive_path", CAT, Warning, Low, "Repetitive Path",
        "The same folder repeats in the path, a sign of relative-link loops creating infinite URLs.",
        "Fix the relative links, typically by making them root-relative.",
        |p, _| ih(p) && {
            // Numeric segments (dates such as /2024/01/01/) repeat legitimately.
            let segs: Vec<String> = path_of(&p.url).split('/').filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit())).map(|s| s.to_string()).collect();
            let mut counts: HashMap<&str, usize> = HashMap::new();
            segs.iter().for_each(|s| *counts.entry(s.as_str()).or_insert(0) += 1);
            segs.windows(2).any(|w| w[0] == w[1]) || counts.values().any(|&c| c >= 3)
        });
    add!(
        v,
        "url.parameters",
        CAT,
        Opportunity,
        Low,
        "Contains Parameters",
        "Query strings can create many URLs with the same content and waste crawl budget.",
        "Use clean paths for indexable content, and canonicals for parameter variants.",
        |p, _| ih(p) && p.url.contains('?')
    );
    add!(v, "url.many_parameters", CAT, Warning, Low, "Over 4 Parameters",
        "Long query strings usually come from faceted navigation and generate near-endless URL combinations.",
        "Limit crawlable facets and canonicalise parameter combinations.",
        |p, _| ih(p) && query_keys(&p.url).len() > 4);
    add!(
        v,
        "url.over_115",
        CAT,
        Opportunity,
        Low,
        "Over 115 Characters",
        "Very long URLs are hard to read and share, and get truncated in search results.",
        "Shorten slugs and remove unnecessary folders.",
        |p, _| ih(p) && p.url.chars().count() > 115
    );
    add!(v, "url.internal_search", CAT, Warning, Medium, "Internal Search URLs",
        "Internal site-search result pages are crawlable. Google advises against letting them be indexed.",
        "Noindex search results and stop linking to them, or block them in robots.txt.",
        |p, _| ih(p) && query_keys(&p.url).iter().any(|k| ["q", "s", "search", "query", "keyword", "keywords", "term"].contains(&k.as_str())));
    add!(
        v,
        "url.session_id",
        CAT,
        Warning,
        Medium,
        "Session IDs in URL",
        "Session identifiers in URLs create a new URL per visitor and leak sessions when shared.",
        "Keep sessions in cookies, not URLs.",
        |p, _| ih(p)
            && (p.url.to_ascii_lowercase().contains(";jsessionid=")
                || query_keys(&p.url).iter().any(|k| [
                    "sid",
                    "sessionid",
                    "session_id",
                    "jsessionid",
                    "phpsessid",
                    "sessid"
                ]
                .contains(&k.as_str())))
    );
    add!(v, "url.tracking_parameters", CAT, Warning, Low, "Tracking Parameters on Internal URLs",
        "Internal links carry utm_ or click-ID parameters, which create duplicate URLs and corrupt analytics.",
        "Remove tracking parameters from internal links.",
        |p, _| ih(p) && query_keys(&p.url).iter().any(|k| k.starts_with("utm_") || ["gclid", "fbclid", "msclkid", "mc_cid", "mc_eid", "_ga"].contains(&k.as_str())));
    add!(
        v,
        "url.deep_folders",
        CAT,
        Opportunity,
        Low,
        "Over 5 Folders Deep",
        "Deeply nested folder structures make URLs long and suggest content is buried.",
        "Flatten the URL structure where possible.",
        |p, _| ih(p) && path_of(&p.url).split('/').filter(|s| !s.is_empty()).count() > 5
    );
    add!(v, "url.unsafe_characters", CAT, Warning, Low, "Unsafe Characters",
        "Characters such as ! * ' ( ) , { } | \\ ^ ` often get mangled by email clients, CMSs and analytics tools.",
        "Use only letters, numbers and hyphens in slugs.",
        |p, _| ih(p) && {
            let path = path_of(&p.url);
            path.chars().any(|c| "!*'(),".contains(c)) || ["%7B", "%7D", "%7C", "%5C", "%5E", "%60"].iter().any(|e| path.to_ascii_uppercase().contains(e))
        });
    add!(
        v,
        "url.index_file",
        CAT,
        Warning,
        Low,
        "Index File in URL",
        "URLs ending in index.html or default.aspx duplicate the folder URL.",
        "Link to the folder URL and redirect the index-file version.",
        |p, _| ih(p) && {
            let l = path_of(&p.url).to_ascii_lowercase();
            [
                "/index.html",
                "/index.htm",
                "/index.php",
                "/default.aspx",
                "/default.asp",
                "/index.asp",
            ]
            .iter()
            .any(|e| l.ends_with(e))
        }
    );
    add!(
        v,
        "url.double_encoded",
        CAT,
        Warning,
        Low,
        "Double-Encoded Characters",
        "%25 in a URL means an already-encoded character was encoded again, usually by a bug.",
        "Fix the link generation so characters are encoded once.",
        |p, _| ih(p) && p.url.contains("%25")
    );
    add!(
        v,
        "url.trailing_slash_duplicate",
        CAT,
        Warning,
        Medium,
        "Duplicate via Trailing Slash",
        "The URL works both with and without a trailing slash, creating two copies of the page.",
        "Pick one form and 301-redirect the other.",
        |p, x| ih(p) && path_of(&p.url) != "/" && !p.url.contains('?') && {
            let alt = match p.url.strip_suffix('/') {
                Some(s) => s.to_string(),
                None => format!("{}/", p.url),
            };
            x.page(&alt).is_some_and(ih)
        }
    );
    add!(
        v,
        "url.case_duplicate",
        CAT,
        Warning,
        Medium,
        "Duplicate via Letter Case",
        "Several URLs that differ only by letter case all return 200, creating duplicate pages.",
        "Redirect case variants to the lower-case URL.",
        |p, x| ih(p) && x.case_variants(p)
    );
    add!(
        v,
        "url.www_not_consolidated",
        CAT,
        Warning,
        Medium,
        "www and non-www Not Consolidated",
        "Both the www and non-www host serve the site without redirecting to one version.",
        "301-redirect one host to the other.",
        |p, x| x.is_start(p) && x.crawl.site.alt_host_redirects == Some(false)
    );
}

fn titles(v: &mut Vec<Check>) {
    const CAT: &str = "Page Titles";
    fn title(h: &HtmlData) -> &str {
        first(&h.titles)
    }
    add!(v, "titles.missing", CAT, Issue, High, "Missing",
        "Pages with no <title> element. The title is the main headline in search results and browser tabs.",
        "Write a unique, descriptive title for every page.",
        |p, _| hd(p).is_some_and(|h| h.titles.is_empty()));
    add!(
        v,
        "titles.empty",
        CAT,
        Issue,
        High,
        "Empty",
        "A <title> element exists but has no text.",
        "Add a descriptive title.",
        |p, _| hd(p).is_some_and(|h| !h.titles.is_empty() && title(h).is_empty())
    );
    add!(
        v,
        "titles.multiple",
        CAT,
        Issue,
        Medium,
        "Multiple",
        "More than one <title> element. Search engines may pick the wrong one.",
        "Keep a single title element in the <head>.",
        |p, _| hd(p).is_some_and(|h| h.titles.len() > 1)
    );
    add!(
        v,
        "titles.duplicate",
        CAT,
        Issue,
        Medium,
        "Duplicate",
        "Indexable pages share the same title, making them hard to tell apart in search results.",
        "Give every indexable page a unique title.",
        |p, x| ihd(p).is_some_and(|h| !title(h).is_empty() && x.dup_title(title(h)))
    );
    add!(
        v,
        "titles.over_60",
        CAT,
        Opportunity,
        Medium,
        "Over 60 Characters",
        "Long titles are likely to be cut off in search results.",
        "Keep titles to about 50–60 characters, with the key words first.",
        |p, _| hd(p).is_some_and(|h| title(h).chars().count() > 60)
    );
    add!(
        v,
        "titles.below_30",
        CAT,
        Opportunity,
        Medium,
        "Below 30 Characters",
        "Short titles leave out useful keywords and context.",
        "Expand the title to describe the page fully.",
        |p, _| hd(p).is_some_and(|h| {
            let n = title(h).chars().count();
            n > 0 && n < 30
        })
    );
    add!(
        v,
        "titles.over_580px",
        CAT,
        Opportunity,
        Medium,
        "Over 580 Pixels",
        "The title is wider than the space Google shows on desktop and will be truncated.",
        "Shorten the title or use narrower words.",
        |p, _| hd(p).is_some_and(|h| pixel_width(title(h), 20.0) > 580)
    );
    add!(
        v,
        "titles.below_200px",
        CAT,
        Opportunity,
        Low,
        "Below 200 Pixels",
        "The title uses less than a third of the available space.",
        "Use the space to describe the page better.",
        |p, _| hd(p).is_some_and(|h| {
            let w = pixel_width(title(h), 20.0);
            w > 0 && w < 200
        })
    );
    add!(
        v,
        "titles.same_as_h1",
        CAT,
        Opportunity,
        Low,
        "Same as H1",
        "The title repeats the H1 exactly, missing a chance to target more search phrases.",
        "Vary the title and H1 while keeping them consistent.",
        |p, _| hd(p)
            .is_some_and(|h| !title(h).is_empty() && title(h).eq_ignore_ascii_case(first(&h.h1)))
    );
    add!(
        v,
        "titles.outside_head",
        CAT,
        Issue,
        High,
        "Outside <head>",
        "A <title> appears in the <body>. Search engines may ignore it.",
        "Move the title into the <head>, and check nothing in the head closes it early.",
        |p, _| hd(p).is_some_and(|h| h.title_outside_head)
    );
    add!(
        v,
        "titles.same_as_description",
        CAT,
        Warning,
        Low,
        "Same as Meta Description",
        "The title and meta description are identical, wasting the description's space.",
        "Write a description that expands on the title.",
        |p, _| hd(p)
            .is_some_and(|h| !title(h).is_empty() && title(h) == first(&h.meta_descriptions))
    );
    add!(
        v,
        "titles.generic",
        CAT,
        Warning,
        Medium,
        "Generic Title",
        "Titles such as Home, Untitled or Welcome say nothing about the page.",
        "Write a title that describes this specific page.",
        |p, _| hd(p).is_some_and(|h| [
            "home",
            "homepage",
            "untitled",
            "page",
            "new page",
            "index",
            "welcome",
            "default",
            "untitled document",
            "document",
            "title"
        ]
        .contains(&title(h).trim().to_lowercase().as_str()))
    );
    add!(
        v,
        "titles.keyword_stuffing",
        CAT,
        Warning,
        Low,
        "Repeated Words",
        "A word appears three or more times in the title, which reads as keyword stuffing.",
        "Use each keyword once and write for people.",
        |p, _| hd(p).is_some_and(|h| stuffed(title(h)))
    );
    add!(
        v,
        "titles.all_caps",
        CAT,
        Opportunity,
        Low,
        "All Uppercase",
        "All-caps titles are harder to read and look like shouting in search results.",
        "Use sentence or title case.",
        |p, _| hd(p).is_some_and(|h| all_caps(title(h)))
    );
    add!(
        v,
        "titles.placeholder",
        CAT,
        Issue,
        Medium,
        "Placeholder Text",
        "The title contains template or placeholder text such as lorem ipsum or {{ }}.",
        "Replace placeholders with the real title.",
        |p, _| hd(p).is_some_and(|h| placeholder(title(h)))
    );
    add!(
        v,
        "titles.domain_only",
        CAT,
        Warning,
        Low,
        "Domain Name Only",
        "The title is just the domain name.",
        "Describe the page content in the title.",
        |p, _| hd(p).is_some_and(|h| {
            let t = title(h).trim().to_lowercase();
            !t.is_empty() && (t == host(&p.url) || t == bare_host(&p.url))
        })
    );
    add!(
        v,
        "titles.double_encoded",
        CAT,
        Warning,
        Low,
        "Double-Encoded Entities",
        "The title shows raw entities such as &amp;amp; because they were encoded twice.",
        "Encode special characters once.",
        |p, _| hd(p).is_some_and(|h| double_encoded(title(h)))
    );
}

fn descriptions(v: &mut Vec<Check>) {
    const CAT: &str = "Meta Description";
    fn desc(h: &HtmlData) -> &str {
        first(&h.meta_descriptions)
    }
    add!(v, "description.missing", CAT, Opportunity, Medium, "Missing",
        "No meta description. Search engines will pick a snippet from the page, which may be less compelling.",
        "Write a unique description that summarises the page and invites the click.",
        |p, _| ihd(p).is_some_and(|h| h.meta_descriptions.is_empty()));
    add!(
        v,
        "description.empty",
        CAT,
        Opportunity,
        Medium,
        "Empty",
        "The meta description tag exists but is empty.",
        "Fill in the description.",
        |p, _| ihd(p).is_some_and(|h| !h.meta_descriptions.is_empty() && desc(h).is_empty())
    );
    add!(
        v,
        "description.multiple",
        CAT,
        Issue,
        Medium,
        "Multiple",
        "More than one meta description tag. Search engines may pick either.",
        "Keep a single meta description.",
        |p, _| hd(p).is_some_and(|h| h.meta_descriptions.len() > 1)
    );
    add!(
        v,
        "description.duplicate",
        CAT,
        Opportunity,
        Medium,
        "Duplicate",
        "Indexable pages share the same description.",
        "Write a unique description for each page.",
        |p, x| ihd(p).is_some_and(|h| !desc(h).is_empty() && x.dup_desc(desc(h)))
    );
    add!(
        v,
        "description.over_155",
        CAT,
        Opportunity,
        Low,
        "Over 155 Characters",
        "Long descriptions get truncated in search results.",
        "Aim for 120–155 characters.",
        |p, _| ihd(p).is_some_and(|h| desc(h).chars().count() > 155)
    );
    add!(
        v,
        "description.below_70",
        CAT,
        Opportunity,
        Low,
        "Below 70 Characters",
        "Short descriptions leave out persuasive detail.",
        "Expand the description to about 120–155 characters.",
        |p, _| ihd(p).is_some_and(|h| {
            let n = desc(h).chars().count();
            n > 0 && n < 70
        })
    );
    add!(
        v,
        "description.over_990px",
        CAT,
        Opportunity,
        Low,
        "Over 990 Pixels",
        "The description is wider than Google's desktop snippet and will be cut off.",
        "Shorten the description.",
        |p, _| ihd(p).is_some_and(|h| pixel_width(desc(h), 14.0) > 990)
    );
    add!(
        v,
        "description.below_400px",
        CAT,
        Opportunity,
        Low,
        "Below 400 Pixels",
        "The description fills less than half the snippet space.",
        "Use the space to sell the page.",
        |p, _| ihd(p).is_some_and(|h| {
            let w = pixel_width(desc(h), 14.0);
            w > 0 && w < 400
        })
    );
    add!(
        v,
        "description.outside_head",
        CAT,
        Issue,
        High,
        "Outside <head>",
        "The meta description appears in the <body>, where search engines ignore it.",
        "Move it into the <head>.",
        |p, _| hd(p).is_some_and(|h| h.description_outside_head)
    );
    add!(
        v,
        "description.placeholder",
        CAT,
        Issue,
        Medium,
        "Placeholder Text",
        "The description contains placeholder or template text.",
        "Replace it with a real description.",
        |p, _| hd(p).is_some_and(|h| placeholder(desc(h)))
    );
    add!(
        v,
        "description.all_caps",
        CAT,
        Opportunity,
        Low,
        "All Uppercase",
        "All-caps descriptions are hard to read.",
        "Use normal sentence case.",
        |p, _| hd(p).is_some_and(|h| all_caps(desc(h)))
    );
    add!(
        v,
        "description.keyword_stuffing",
        CAT,
        Warning,
        Low,
        "Repeated Words",
        "Words repeated three or more times read as keyword stuffing.",
        "Write naturally for the reader.",
        |p, _| hd(p).is_some_and(|h| {
            let d = desc(h);
            stuffed(d) && d.split_whitespace().count() < 40
        })
    );
    add!(
        v,
        "description.double_encoded",
        CAT,
        Warning,
        Low,
        "Double-Encoded Entities",
        "Raw entities such as &amp;amp; show in the snippet.",
        "Encode special characters once.",
        |p, _| hd(p).is_some_and(|h| double_encoded(desc(h)))
    );
    add!(
        v,
        "description.contains_html",
        CAT,
        Warning,
        Low,
        "Contains HTML Tags",
        "The description contains HTML markup, which shows as raw text in snippets.",
        "Use plain text in meta descriptions.",
        |p, _| hd(p).is_some_and(|h| {
            let d = desc(h);
            d.contains("</") || d.contains("<br") || d.contains("<p>")
        })
    );

    const KCAT: &str = "Meta Keywords";
    add!(v, "keywords.present", KCAT, Opportunity, Low, "Meta Keywords Present",
        "Google and Bing ignore meta keywords, and the tag tells competitors which terms you target.",
        "Remove the meta keywords tag.",
        |p, _| hd(p).is_some_and(|h| !h.meta_keywords.is_empty()));
    add!(
        v,
        "keywords.multiple",
        KCAT,
        Warning,
        Low,
        "Multiple Meta Keywords",
        "More than one meta keywords tag on the page.",
        "Remove the duplicate tags.",
        |p, _| hd(p).is_some_and(|h| h.meta_keywords.len() > 1)
    );
    add!(
        v,
        "keywords.duplicate",
        KCAT,
        Opportunity,
        Low,
        "Duplicate Meta Keywords",
        "Pages share an identical keywords list, usually from a template default.",
        "Remove the tag from the template.",
        |p, x| ihd(p).is_some_and(
            |h| !first(&h.meta_keywords).is_empty() && x.dup_keywords(first(&h.meta_keywords))
        )
    );
}

fn headings(v: &mut Vec<Check>) {
    const H1: &str = "H1";
    add!(
        v,
        "h1.missing",
        H1,
        Issue,
        Medium,
        "Missing",
        "No H1 heading. The H1 tells visitors and search engines what the page is about.",
        "Add one descriptive H1.",
        |p, _| hd(p).is_some_and(|h| h.h1.is_empty())
    );
    add!(
        v,
        "h1.empty",
        H1,
        Issue,
        Medium,
        "Empty",
        "The H1 element has no text.",
        "Put a descriptive heading in the H1.",
        |p, _| hd(p).is_some_and(|h| h.h1.first().is_some_and(|s| s.is_empty()))
    );
    add!(
        v,
        "h1.duplicate",
        H1,
        Opportunity,
        Low,
        "Duplicate",
        "Indexable pages share the same H1.",
        "Make each H1 specific to its page.",
        |p, x| ihd(p).is_some_and(|h| !first(&h.h1).is_empty() && x.dup_h1(first(&h.h1)))
    );
    add!(
        v,
        "h1.multiple",
        H1,
        Warning,
        Low,
        "Multiple",
        "More than one H1. Allowed in HTML5, but a single H1 gives a clearer main topic.",
        "Use one H1 and H2–H6 for sections.",
        |p, _| hd(p).is_some_and(|h| h.h1.len() > 1)
    );
    add!(
        v,
        "h1.over_70",
        H1,
        Opportunity,
        Low,
        "Over 70 Characters",
        "Very long H1s are hard to scan.",
        "Keep the H1 short; put detail in the body.",
        |p, _| hd(p).is_some_and(|h| first(&h.h1).chars().count() > 70)
    );
    add!(
        v,
        "h1.image_only",
        H1,
        Warning,
        Low,
        "Image-Only H1",
        "The H1 contains only an image, so its text comes from alt text.",
        "Use real text in the H1.",
        |p, _| hd(p).is_some_and(|h| h.h1_image_only)
    );
    add!(
        v,
        "h1.repeated_on_page",
        H1,
        Warning,
        Low,
        "Same H1 Repeated on Page",
        "The same H1 text appears more than once on the page.",
        "Remove the repeated heading.",
        |p, _| hd(p).is_some_and(|h| {
            let set: HashSet<&String> = h.h1.iter().filter(|s| !s.is_empty()).collect();
            set.len() < h.h1.iter().filter(|s| !s.is_empty()).count()
        })
    );

    const H2: &str = "H2";
    add!(
        v,
        "h2.missing",
        H2,
        Warning,
        Low,
        "Missing",
        "No H2 subheadings. Subheadings structure content for readers and search engines.",
        "Break content into sections with H2s.",
        |p, _| ihd(p).is_some_and(|h| h.h2.is_empty() && h.word_count >= 300)
    );
    add!(
        v,
        "h2.empty",
        H2,
        Warning,
        Low,
        "Empty",
        "An H2 element has no text.",
        "Remove empty headings or add text.",
        |p, _| hd(p).is_some_and(|h| h.h2.iter().any(|s| s.is_empty()))
    );
    add!(
        v,
        "h2.duplicate",
        H2,
        Opportunity,
        Low,
        "Duplicate",
        "Indexable pages share the same first H2.",
        "Make subheadings specific to each page.",
        |p, x| ihd(p).is_some_and(|h| !first(&h.h2).is_empty() && x.dup_h2(first(&h.h2)))
    );
    add!(
        v,
        "h2.over_70",
        H2,
        Opportunity,
        Low,
        "Over 70 Characters",
        "Very long H2s are hard to scan.",
        "Shorten subheadings.",
        |p, _| hd(p).is_some_and(|h| h.h2.iter().any(|s| s.chars().count() > 70))
    );
    add!(
        v,
        "h2.same_as_h1",
        H2,
        Warning,
        Low,
        "Same as H1",
        "The first H2 repeats the H1.",
        "Use the H2 to introduce a sub-topic.",
        |p, _| hd(p).is_some_and(
            |h| !first(&h.h1).is_empty() && first(&h.h1).eq_ignore_ascii_case(first(&h.h2))
        )
    );

    const HD: &str = "Headings";
    add!(v, "headings.non_sequential", HD, Warning, Low, "Non-Sequential Heading Levels",
        "Heading levels jump (for example H2 straight to H4), which breaks the outline screen readers rely on.",
        "Step down one level at a time.",
        |p, _| hd(p).is_some_and(|h| h.heading_levels.windows(2).any(|w| w[1] > w[0] + 1)));
    add!(
        v,
        "headings.first_not_h1",
        HD,
        Warning,
        Low,
        "First Heading Is Not H1",
        "The page's first heading is lower than H1.",
        "Start the content with the H1.",
        |p, _| hd(p).is_some_and(|h| h.heading_levels.first().is_some_and(|&l| l != 1))
    );
    add!(
        v,
        "headings.empty",
        HD,
        Warning,
        Low,
        "Empty Headings",
        "Heading elements with no text confuse screen-reader navigation.",
        "Remove empty headings.",
        |p, _| hd(p).is_some_and(|h| h.empty_headings > 0)
    );
    add!(
        v,
        "headings.none",
        HD,
        Warning,
        Medium,
        "No Headings",
        "The page has no headings at all.",
        "Structure content with an H1 and subheadings.",
        |p, _| ihd(p).is_some_and(|h| h.heading_levels.is_empty())
    );
}

fn content(v: &mut Vec<Check>) {
    const CAT: &str = "Content";
    add!(
        v,
        "content.thin",
        CAT,
        Issue,
        Medium,
        "Thin Content (Under 50 Words)",
        "Indexable pages with almost no text. They rarely rank and can drag down site quality.",
        "Add useful content, merge the page with a related one, or noindex it.",
        |p, _| ihd(p).is_some_and(|h| h.word_count < 50)
    );
    add!(
        v,
        "content.low",
        CAT,
        Opportunity,
        Medium,
        "Low Content (50–199 Words)",
        "Indexable pages with little text may not satisfy searchers.",
        "Expand the content where it helps visitors.",
        |p, _| ihd(p).is_some_and(|h| (50..200).contains(&h.word_count))
    );
    add!(
        v,
        "content.exact_duplicate",
        CAT,
        Issue,
        High,
        "Exact Duplicate",
        "Indexable pages with identical text. Search engines choose one and ignore the others.",
        "Consolidate duplicates with 301 redirects or canonicals.",
        |p, x| ihd(p).is_some_and(|h| h.word_count >= 20 && x.dup_content(h.content_hash))
    );
    add!(
        v,
        "content.near_duplicate",
        CAT,
        Warning,
        Medium,
        "Near Duplicate",
        "Indexable pages that are about 90% or more similar to another page.",
        "Differentiate the pages or consolidate them.",
        |p, x| x.near_dup(p)
    );
    add!(
        v,
        "content.lorem_ipsum",
        CAT,
        Issue,
        High,
        "Lorem Ipsum Placeholder",
        "The page still contains lorem ipsum placeholder text.",
        "Replace placeholder copy with real content.",
        |p, _| hd(p).is_some_and(|h| h.lorem_ipsum)
    );
    add!(
        v,
        "content.soft_404",
        CAT,
        Issue,
        High,
        "Soft 404",
        "The page says 'not found' but returns 200, so search engines may index an error page.",
        "Return a real 404 or 410 status for missing content.",
        |p, _| hd(p).is_some_and(|h| {
            let s = format!("{} {}", first(&h.titles), first(&h.h1)).to_lowercase();
            [
                "404",
                "not found",
                "page not found",
                "does not exist",
                "doesn't exist",
                "no longer available",
            ]
            .iter()
            .any(|k| s.contains(k))
        })
    );
    add!(
        v,
        "content.readability_difficult",
        CAT,
        Opportunity,
        Low,
        "Readability Difficult",
        "Flesch reading-ease score between 30 and 50: college-level text.",
        "Use shorter sentences and simpler words.",
        |p, _| ihd(p)
            .is_some_and(|h| english(h) && h.word_count >= 100 && (30.0..50.0).contains(&h.flesch))
    );
    add!(
        v,
        "content.readability_very_difficult",
        CAT,
        Opportunity,
        Low,
        "Readability Very Difficult",
        "Flesch reading-ease score below 30: hard even for graduates.",
        "Break up long sentences and cut jargon.",
        |p, _| ihd(p).is_some_and(|h| english(h) && h.word_count >= 100 && h.flesch < 30.0)
    );
    add!(v, "content.low_text_ratio", CAT, Opportunity, Low, "Low Text-to-HTML Ratio",
        "Under 10% of the HTML is visible text, usually from heavy markup, inline code or thin content.",
        "Trim unused markup and inline code, and add meaningful text.",
        |p, _| ihd(p).is_some_and(|h| p.size > 5 * 1024 && h.text_ratio < 10.0));
    add!(
        v,
        "content.under_construction",
        CAT,
        Warning,
        Medium,
        "Coming Soon or Under Construction",
        "The page says it is coming soon or under construction but is open to indexing.",
        "Noindex unfinished pages, or publish the content.",
        |p, _| ihd(p).is_some_and(|h| {
            let s = format!("{} {}", first(&h.titles), first(&h.h1)).to_lowercase();
            s.contains("coming soon") || s.contains("under construction")
        })
    );
}

fn images(v: &mut Vec<Check>) {
    const CAT: &str = "Images";
    fn image_file(p: &Page) -> bool {
        p.is_internal && p.status == 200 && p.kind() == "Image"
    }
    add!(v, "images.missing_alt_attribute", CAT, Issue, Medium, "Missing Alt Attribute",
        "Images with no alt attribute. Screen readers announce the file name and search engines lose context.",
        "Add alt text describing the image, or alt=\"\" for purely decorative images.",
        |p, _| hd(p).is_some_and(|h| h.images.iter().any(|i| i.alt.is_none())));
    add!(v, "images.empty_alt", CAT, Opportunity, Low, "Empty Alt Text",
        "Images with alt=\"\". Correct for decorative images, but informative images need a description.",
        "Check each one and describe the meaningful images.",
        |p, _| hd(p).is_some_and(|h| h.images.iter().any(|i| i.alt.as_deref() == Some(""))));
    add!(
        v,
        "images.alt_over_100",
        CAT,
        Opportunity,
        Low,
        "Alt Text Over 100 Characters",
        "Very long alt text is tiring for screen-reader users.",
        "Keep alt text concise; use a caption for detail.",
        |p, _| hd(p).is_some_and(|h| h
            .images
            .iter()
            .any(|i| i.alt.as_deref().is_some_and(|a| a.chars().count() > 100)))
    );
    add!(
        v,
        "images.missing_dimensions",
        CAT,
        Opportunity,
        Medium,
        "Missing Width and Height",
        "Images without width and height attributes cause layout shift (CLS) as they load.",
        "Set width and height (or CSS aspect-ratio) on every image.",
        |p, _| hd(p).is_some_and(|h| h.images.iter().any(|i| !i.has_width || !i.has_height))
    );
    add!(
        v,
        "images.broken",
        CAT,
        Issue,
        High,
        "Broken Images",
        "Pages show images that return errors or fail to load.",
        "Fix the image URLs or replace the images.",
        |p, x| hd(p).is_some_and(|h| h.images.iter().any(|i| broken(x, &i.url)))
    );
    add!(
        v,
        "images.redirected",
        CAT,
        Warning,
        Low,
        "Redirected Images",
        "Image URLs that redirect add a round trip before the image loads.",
        "Reference the final image URL.",
        |p, x| hd(p).is_some_and(|h| h.images.iter().any(|i| redirecting(x, &i.url)))
    );
    add!(
        v,
        "images.blocked",
        CAT,
        Warning,
        Low,
        "Images Blocked by Robots.txt",
        "Search engines cannot fetch these images, so they cannot appear in image search.",
        "Allow image paths in robots.txt.",
        |p, x| hd(p).is_some_and(|h| h
            .images
            .iter()
            .any(|i| x.page(&i.url).is_some_and(|t| t.blocked_by_robots)))
    );
    add!(
        v,
        "images.generic_alt",
        CAT,
        Warning,
        Low,
        "Generic Alt Text",
        "Alt text such as 'image' or 'photo' adds no information.",
        "Describe what the image shows.",
        |p, _| hd(p).is_some_and(
            |h| h.images.iter().any(|i| i.alt.as_deref().is_some_and(|a| [
                "image",
                "img",
                "photo",
                "picture",
                "pic",
                "graphic",
                "icon",
                "untitled",
                "placeholder",
                "banner"
            ]
            .contains(&a.trim().to_lowercase().as_str())))
        )
    );
    add!(
        v,
        "images.filename_alt",
        CAT,
        Warning,
        Low,
        "File Name Used as Alt Text",
        "Alt text that is a file name (for example IMG_1234.jpg), usually filled in by a CMS.",
        "Write a real description.",
        |p, _| hd(p).is_some_and(
            |h| h.images.iter().any(|i| i.alt.as_deref().is_some_and(|a| {
                let l = a.trim().to_lowercase();
                [".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".avif"]
                    .iter()
                    .any(|e| l.ends_with(e))
            }))
        )
    );
    add!(
        v,
        "images.duplicate_alt",
        CAT,
        Opportunity,
        Low,
        "Same Alt Text on Different Images",
        "Different images on the page share identical alt text.",
        "Describe each image individually.",
        |p, _| hd(p).is_some_and(|h| {
            let mut seen: HashMap<&str, String> = HashMap::new();
            h.images
                .iter()
                .filter_map(|i| {
                    i.alt
                        .as_deref()
                        .filter(|a| a.len() > 3)
                        .map(|a| (a, image_stem(&i.url)))
                })
                .any(|(a, u)| matches!(seen.insert(a, u.clone()), Some(prev) if prev != u))
        })
    );
    add!(
        v,
        "images.not_lazy",
        CAT,
        Opportunity,
        Low,
        "Images Not Lazy-Loaded",
        "The page has many images and the ones further down load immediately.",
        "Add loading=\"lazy\" to images below the fold.",
        |p, _| hd(p)
            .is_some_and(|h| h.images.len() > 10 && h.images.iter().skip(5).any(|i| !i.lazy))
    );
    add!(
        v,
        "images.over_100kb",
        CAT,
        Opportunity,
        Medium,
        "Over 100 KB",
        "Image files between 100 KB and 500 KB slow page loads.",
        "Compress and resize images; serve WebP or AVIF.",
        move |p, _| image_file(p) && (100 * 1024..500 * 1024).contains(&p.size)
    );
    add!(
        v,
        "images.over_500kb",
        CAT,
        Warning,
        High,
        "Over 500 KB",
        "Very large image files that badly hurt loading speed, especially on mobile.",
        "Resize to display size and compress, or use a modern format.",
        move |p, _| image_file(p) && p.size >= 500 * 1024
    );
    add!(
        v,
        "images.legacy_format",
        CAT,
        Opportunity,
        Medium,
        "Legacy Format (BMP/TIFF)",
        "BMP and TIFF images are huge and poorly supported in browsers.",
        "Convert to WebP, AVIF, JPEG or PNG.",
        move |p, _| image_file(p) && {
            let c = p.content_type.to_ascii_lowercase();
            c.contains("bmp") || c.contains("tiff")
        }
    );
    add!(
        v,
        "images.next_gen",
        CAT,
        Opportunity,
        Low,
        "Not in Next-Gen Format",
        "Large JPEG or PNG images that would be much smaller as WebP or AVIF.",
        "Serve WebP or AVIF, with fallbacks where needed.",
        move |p, _| image_file(p) && p.size > 100 * 1024 && {
            let c = p.content_type.to_ascii_lowercase();
            c.contains("jpeg") || c.contains("jpg") || c.contains("png")
        }
    );
    add!(
        v,
        "images.wrong_content_type",
        CAT,
        Warning,
        Medium,
        "Image URL Not Serving an Image",
        "An <img> source returns something other than an image, such as an HTML error page.",
        "Fix the image URL or the server's Content-Type.",
        |p, _| p.is_internal
            && p.status == 200
            && p.source == Some(Source::Image)
            && !p.content_type.is_empty()
            && !p.content_type.to_ascii_lowercase().starts_with("image/")
    );
    add!(v, "images.non_descriptive_filename", CAT, Opportunity, Low, "Non-Descriptive File Name",
        "Camera or CMS default names (IMG_1234, DSC0001, image001) give search engines no clue about the image.",
        "Rename images with descriptive, hyphenated words.",
        move |p, _| image_file(p) && {
            static RE: OnceLock<Regex> = OnceLock::new();
            let re = RE.get_or_init(|| Regex::new(r"(?i)^(img|image|dsc|dscn|dcim|photo|pic|screenshot|untitled|unnamed|whatsapp)([-_ ]?\d.*)?$|^\d+$").unwrap());
            let name = path_of(&p.url).rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(n, _)| n.to_string()).unwrap_or_default();
            !name.is_empty() && re.is_match(&name)
        });
}

fn canonicals(v: &mut Vec<Check>) {
    const CAT: &str = "Canonicals";
    fn canon(h: &HtmlData) -> &str {
        first(&h.canonicals)
    }
    add!(v, "canonicals.missing", CAT, Warning, Medium, "Missing",
        "No canonical tag. Without one, parameter and tracking variants of the URL compete with it.",
        "Add a self-referencing canonical to every indexable page.",
        |p, _| hd(p).is_some_and(|h| h.canonicals_raw.is_empty() && link_canonical_header(p).is_none() && !p.is_noindex()));
    add!(v, "canonicals.multiple", CAT, Warning, Low, "Multiple (Same URL)",
        "More than one canonical tag pointing to the same URL. Redundant, and fragile if they ever diverge.",
        "Keep a single canonical tag.",
        |p, _| hd(p).is_some_and(|h| h.canonicals_raw.len() > 1 && h.canonicals.iter().collect::<HashSet<_>>().len() == 1));
    add!(
        v,
        "canonicals.conflicting",
        CAT,
        Issue,
        High,
        "Multiple Conflicting",
        "Several canonical tags point to different URLs, so search engines ignore all of them.",
        "Keep one canonical pointing at the preferred URL.",
        |p, _| hd(p).is_some_and(|h| h.canonicals.iter().collect::<HashSet<_>>().len() > 1)
    );
    add!(
        v,
        "canonicals.canonicalised",
        CAT,
        Warning,
        Low,
        "Canonicalised",
        "The page names a different URL as canonical, so it is unlikely to be indexed itself.",
        "Check this is intended, and link to the canonical URL instead.",
        |p, _| ih(p) && !self_canonical(p)
    );
    add!(v, "canonicals.relative", CAT, Warning, Low, "Relative Canonical URL",
        "The canonical uses a relative URL, which breaks easily when pages are copied or served on other hosts.",
        "Use absolute canonical URLs.",
        |p, _| hd(p).is_some_and(|h| h.canonicals_raw.iter().any(|c| !c.trim().is_empty() && !c.trim().starts_with("http"))));
    add!(
        v,
        "canonicals.outside_head",
        CAT,
        Issue,
        High,
        "Outside <head>",
        "A canonical tag in the <body> is ignored by search engines.",
        "Move the canonical into the <head>.",
        |p, _| hd(p).is_some_and(|h| h.canonical_outside_head)
    );
    add!(
        v,
        "canonicals.to_http",
        CAT,
        Issue,
        High,
        "Canonical to HTTP",
        "An HTTPS page names an HTTP URL as canonical.",
        "Point canonicals at HTTPS URLs.",
        |p, _| hd(p).is_some_and(|h| p.is_https() && canon(h).starts_with("http://"))
    );
    add!(
        v,
        "canonicals.other_domain",
        CAT,
        Warning,
        Medium,
        "Canonical to Another Domain",
        "The canonical points to a different domain, handing this page's ranking to that site.",
        "Make sure cross-domain canonicals are intentional (for example, syndicated content).",
        |p, _| hd(p)
            .is_some_and(|h| !canon(h).is_empty() && bare_host(canon(h)) != bare_host(&p.url))
    );
    add!(
        v,
        "canonicals.to_redirect",
        CAT,
        Issue,
        High,
        "Canonical to Redirect",
        "The canonical URL redirects. Search engines may ignore the hint.",
        "Point the canonical at the final destination URL.",
        |p, x| hd(p)
            .is_some_and(|h| !canon(h).is_empty() && canon(h) != p.url && redirecting(x, canon(h)))
    );
    add!(
        v,
        "canonicals.to_4xx",
        CAT,
        Issue,
        High,
        "Canonical to Client Error (4xx)",
        "The canonical URL is broken.",
        "Point the canonical at a live page.",
        |p, x| hd(p).is_some_and(|h| x.status(canon(h)).is_some_and(|s| (400..500).contains(&s)))
    );
    add!(
        v,
        "canonicals.to_5xx",
        CAT,
        Issue,
        High,
        "Canonical to Server Error (5xx)",
        "The canonical URL returns a server error.",
        "Fix the canonical target.",
        |p, x| hd(p).is_some_and(|h| x.status(canon(h)).is_some_and(|s| s >= 500))
    );
    add!(
        v,
        "canonicals.to_noindex",
        CAT,
        Issue,
        High,
        "Canonical to Noindex Page",
        "The canonical target is noindex, sending conflicting signals.",
        "Point the canonical at an indexable page.",
        |p, x| hd(p).is_some_and(|h| !canon(h).is_empty()
            && canon(h) != p.url
            && x.page(canon(h)).is_some_and(|t| ih(t) && t.is_noindex()))
    );
    add!(
        v,
        "canonicals.chain",
        CAT,
        Warning,
        Medium,
        "Canonical Chain",
        "The canonical target names yet another URL as its own canonical.",
        "Point every page directly at the final canonical URL.",
        |p, x| hd(p).is_some_and(|h| !canon(h).is_empty()
            && canon(h) != p.url
            && x.page(canon(h))
                .is_some_and(|t| ih(t) && !self_canonical(t)))
    );
    add!(
        v,
        "canonicals.blocked",
        CAT,
        Issue,
        Medium,
        "Canonical Blocked by Robots.txt",
        "Search engines cannot crawl the canonical URL to confirm it.",
        "Allow the canonical URL in robots.txt.",
        |p, x| hd(p).is_some_and(|h| x.page(canon(h)).is_some_and(|t| t.blocked_by_robots))
    );
    add!(
        v,
        "canonicals.invalid",
        CAT,
        Issue,
        High,
        "Invalid Canonical URL",
        "The canonical href is not a valid http(s) URL.",
        "Use a valid absolute URL.",
        |p, _| hd(p).is_some_and(|h| h.canonicals_raw.len() > h.canonicals.len()
            || h.canonicals_raw.iter().any(|c| c.trim().is_empty()))
    );
    add!(
        v,
        "canonicals.fragment",
        CAT,
        Warning,
        Low,
        "Canonical Contains Fragment",
        "The canonical includes a #fragment, which search engines strip.",
        "Remove the fragment.",
        |p, _| hd(p).is_some_and(|h| h.canonicals_raw.iter().any(|c| c.contains('#')))
    );
    add!(
        v,
        "canonicals.header_conflict",
        CAT,
        Issue,
        High,
        "HTTP Header and HTML Canonical Conflict",
        "The Link header and the HTML tag name different canonical URLs.",
        "Use one method, or make both agree.",
        |p, _| hd(p)
            .is_some_and(|h| link_canonical_header(p)
                .is_some_and(|hc| !canon(h).is_empty() && hc != canon(h)))
    );
    add!(v, "canonicals.unlinked", CAT, Warning, Low, "Unlinked Canonical Target",
        "The canonical URL gets no internal links, so you are telling search engines to prefer a page your site does not link to.",
        "Link to the canonical URL internally.",
        |p, x| hd(p).is_some_and(|h| !canon(h).is_empty() && canon(h) != p.url && x.page(canon(h)).is_some_and(|t| !x.linked(t))));
}

fn pagination(v: &mut Vec<Check>) {
    const CAT: &str = "Pagination";
    add!(
        v,
        "pagination.non_200",
        CAT,
        Warning,
        Medium,
        "Pagination URL Not 200",
        "rel=next or rel=prev points at a URL that redirects or errors.",
        "Point pagination links at live pages.",
        |p, x| hd(p).is_some_and(|h| h
            .rel_next
            .iter()
            .chain(h.rel_prev.iter())
            .any(|u| x.status(u).is_some_and(|s| s != 200)))
    );
    add!(
        v,
        "pagination.multiple",
        CAT,
        Issue,
        Medium,
        "Multiple rel=next or rel=prev",
        "More than one next or prev link makes the sequence ambiguous.",
        "Keep one of each.",
        |p, _| hd(p).is_some_and(|h| h.rel_next.len() > 1 || h.rel_prev.len() > 1)
    );
    add!(v, "pagination.non_indexable", CAT, Warning, Low, "Pagination URL Non-Indexable",
        "Paginated pages that are noindex or canonicalised, so items deeper in the series may not be found.",
        "Keep paginated pages indexable with self-referencing canonicals.",
        |p, x| hd(p).is_some_and(|h| h.rel_next.iter().chain(h.rel_prev.iter()).any(|u| x.page(u).is_some_and(|t| ih(t) && !indexable(t)))));
    add!(v, "pagination.canonical_to_first", CAT, Warning, Medium, "Paginated Page Canonicalised",
        "A later page in a series names another page as canonical. Google advises self-referencing canonicals on paginated pages.",
        "Give each paginated page a self-referencing canonical.",
        |p, _| hd(p).is_some_and(|h| !h.rel_prev.is_empty()) && !self_canonical(p));
    add!(v, "pagination.unlinked", CAT, Warning, Low, "Unlinked Pagination URL",
        "rel=next points to a URL that is not linked with an <a> on the page. Google uses normal links for discovery.",
        "Add a visible link to the next page.",
        |p, _| hd(p).is_some_and(|h| h.rel_next.iter().any(|n| !h.links.iter().any(|l| &l.url == n))));
    add!(
        v,
        "pagination.loop",
        CAT,
        Issue,
        Medium,
        "Pagination Loop",
        "rel=next or rel=prev points to the page itself.",
        "Point to the actual next and previous pages.",
        |p, _| hd(p).is_some_and(|h| h
            .rel_next
            .iter()
            .chain(h.rel_prev.iter())
            .any(|u| u == &p.url))
    );
}

fn directives(v: &mut Vec<Check>) {
    const CAT: &str = "Directives";
    let dir = |p: &Page, d: &str| ih(p) && p.has_directive(d);
    add!(
        v,
        "directives.noindex",
        CAT,
        Warning,
        High,
        "Noindex",
        "Pages that tell search engines not to index them.",
        "Make sure each noindex is intended, especially on important pages.",
        move |p, _| dir(p, "noindex")
    );
    add!(
        v,
        "directives.nofollow",
        CAT,
        Warning,
        High,
        "Nofollow",
        "Pages that tell search engines not to follow any links on them.",
        "Remove page-level nofollow unless you mean to cut off these links.",
        move |p, _| dir(p, "nofollow")
    );
    add!(
        v,
        "directives.none",
        CAT,
        Warning,
        High,
        "None",
        "The 'none' directive equals noindex, nofollow.",
        "Check it is intended.",
        move |p, _| dir(p, "none")
    );
    add!(
        v,
        "directives.noarchive",
        CAT,
        Warning,
        Low,
        "Noarchive",
        "Search engines are asked not to keep a cached copy.",
        "Remove it unless there is a reason.",
        move |p, _| dir(p, "noarchive")
    );
    add!(
        v,
        "directives.nosnippet",
        CAT,
        Warning,
        Medium,
        "Nosnippet",
        "No text snippet or video preview will show in results, which usually lowers clicks.",
        "Remove nosnippet unless required.",
        move |p, _| dir(p, "nosnippet")
    );
    add!(
        v,
        "directives.max_snippet",
        CAT,
        Warning,
        Low,
        "Max-Snippet Limit",
        "A max-snippet limit shortens the text shown in search results.",
        "Check the limit is intended.",
        |p, _| ih(p) && directive_value(p, "max-snippet").is_some_and(|v| v != "-1")
    );
    add!(v, "directives.max_image_preview", CAT, Warning, Low, "Limited Image Preview",
        "max-image-preview is none or standard, which blocks large image previews in Discover and results.",
        "Use max-image-preview:large.",
        |p, _| ih(p) && { let d = p.robots_directives().replace(' ', ""); d.contains("max-image-preview:none") || d.contains("max-image-preview:standard") });
    add!(
        v,
        "directives.max_video_preview",
        CAT,
        Warning,
        Low,
        "Limited Video Preview",
        "A max-video-preview limit restricts video previews in results.",
        "Check the limit is intended.",
        |p, _| ih(p) && directive_value(p, "max-video-preview").is_some_and(|v| v != "-1")
    );
    add!(
        v,
        "directives.noimageindex",
        CAT,
        Warning,
        Low,
        "Noimageindex",
        "Images on the page will not be indexed.",
        "Remove it if you want image search traffic.",
        move |p, _| dir(p, "noimageindex")
    );
    add!(
        v,
        "directives.notranslate",
        CAT,
        Warning,
        Low,
        "Notranslate",
        "Search engines will not offer a translation of the page.",
        "Remove it unless translation would be harmful.",
        move |p, _| dir(p, "notranslate")
    );
    add!(
        v,
        "directives.unavailable_after",
        CAT,
        Warning,
        Medium,
        "Unavailable_After",
        "The page will drop out of search results after a set date.",
        "Check the date is right.",
        |p, _| ih(p) && p.robots_directives().contains("unavailable_after")
    );
    add!(
        v,
        "directives.x_robots_noindex",
        CAT,
        Warning,
        High,
        "Noindex via X-Robots-Tag",
        "The HTTP header carries noindex. It is easy to miss because it is not in the HTML.",
        "Make sure the server rule setting this header is intended.",
        |p, _| p.is_internal
            && p.status == 200
            && p.headers_all("x-robots-tag").any(|v| v
                .to_ascii_lowercase()
                .split([',', ';'])
                .map(crate::model::directive_name)
                .any(|d| d == "noindex" || d == "none"))
    );
    add!(v, "directives.conflicting", CAT, Issue, High, "Conflicting Directives",
        "The page says both index and noindex, or follow and nofollow. Search engines apply the most restrictive one.",
        "Keep one consistent set of directives.",
        |p, _| ih(p) && {
            let toks: HashSet<String> = p.directive_names().into_iter().collect();
            (toks.contains("index") && toks.contains("noindex")) || (toks.contains("follow") && toks.contains("nofollow"))
        });
    add!(
        v,
        "directives.outside_head",
        CAT,
        Issue,
        High,
        "Robots Meta Outside <head>",
        "A robots meta tag in the body may be ignored.",
        "Move it into the <head>.",
        |p, _| hd(p).is_some_and(|h| h.robots_outside_head)
    );
    add!(
        v,
        "directives.multiple_meta",
        CAT,
        Warning,
        Low,
        "Multiple Robots Meta Tags",
        "Several robots meta tags are combined, which can produce surprising results.",
        "Merge them into one tag.",
        |p, _| hd(p).is_some_and(|h| h.meta_robots.len() > 1)
    );
    add!(
        v,
        "directives.noindex_with_canonical",
        CAT,
        Warning,
        Medium,
        "Noindex With Canonical to Another URL",
        "A noindex page also names a different canonical, which are conflicting signals.",
        "Use either noindex or a canonical, not both.",
        |p, _| ih(p) && p.is_noindex() && !self_canonical(p)
    );
    add!(v, "directives.noindex_linked_heavily", CAT, Warning, Low, "Noindex Page With Many Inlinks",
        "A noindex page receives 10 or more internal links, spending link value on a page that cannot rank.",
        "Link to indexable pages instead, or reconsider the noindex.",
        |p, x| ih(p) && p.is_noindex() && x.inlinks_of(p).len() >= 10);
}

fn robots_and_sitemaps(v: &mut Vec<Check>) {
    const CAT: &str = "Robots.txt";
    fn robots<'a>(p: &Page, x: &'a Ctx) -> Option<&'a crate::model::RobotsInfo> {
        if x.is_start(p) {
            x.crawl.site.robots.as_ref()
        } else {
            None
        }
    }
    add!(v, "robots.missing", CAT, Warning, Medium, "Robots.txt Missing",
        "There is no robots.txt file. Crawlers will crawl everything, and you cannot point them at your sitemap.",
        "Add a robots.txt with a Sitemap line.",
        |p, x| robots(p, x).is_some_and(|r| (400..500).contains(&r.status)));
    add!(v, "robots.server_error", CAT, Issue, High, "Robots.txt Server Error",
        "robots.txt returns a 5xx error. Google then treats the whole site as disallowed until it recovers.",
        "Make robots.txt return 200 (or 404 if you don't need one).",
        |p, x| robots(p, x).is_some_and(|r| r.status >= 500));
    add!(
        v,
        "robots.unreachable",
        CAT,
        Issue,
        High,
        "Robots.txt Unreachable",
        "robots.txt could not be fetched at all. Crawlers may stop crawling the site.",
        "Fix the server or network problem.",
        |p, x| robots(p, x).is_some_and(|r| r.status == 0)
    );
    add!(
        v,
        "robots.disallow_all",
        CAT,
        Issue,
        High,
        "Robots.txt Blocks Entire Site",
        "robots.txt disallows / for the crawler, so nothing can be crawled.",
        "Remove the site-wide Disallow unless the site is meant to be hidden.",
        |p, x| robots(p, x).is_some_and(|r| r.disallow_all)
    );
    add!(
        v,
        "robots.no_sitemap",
        CAT,
        Opportunity,
        Low,
        "No Sitemap in Robots.txt",
        "robots.txt has no Sitemap directive to help crawlers find your sitemap.",
        "Add a 'Sitemap: https://…/sitemap.xml' line.",
        |p, x| robots(p, x).is_some_and(|r| r.status == 200 && r.sitemap_directives.is_empty())
    );
    add!(
        v,
        "robots.crawl_delay",
        CAT,
        Warning,
        Low,
        "Crawl-Delay Set",
        "Google ignores Crawl-delay, while Bing and others slow down a lot because of it.",
        "Remove Crawl-delay unless the server truly cannot cope.",
        |p, x| robots(p, x).is_some_and(|r| r.crawl_delay.is_some())
    );
    add!(
        v,
        "robots.too_large",
        CAT,
        Warning,
        Medium,
        "Robots.txt Over 500 KiB",
        "Google ignores robots.txt rules after the first 500 KiB.",
        "Simplify the rules with wildcards.",
        |p, x| robots(p, x).is_some_and(|r| r.size > 500 * 1024)
    );
    add!(
        v,
        "robots.invalid_lines",
        CAT,
        Warning,
        Low,
        "Robots.txt Syntax Errors",
        "Lines that are not valid robots.txt rules are ignored and may hide a mistake.",
        "Fix or remove the invalid lines.",
        |p, x| robots(p, x).is_some_and(|r| r.invalid_lines > 0)
    );
    add!(
        v,
        "robots.noindex_rule",
        CAT,
        Warning,
        Medium,
        "Unsupported Noindex Rule",
        "Noindex or nofollow lines in robots.txt have not been supported by Google since 2019.",
        "Use meta robots or X-Robots-Tag instead.",
        |p, x| robots(p, x).is_some_and(|r| r.noindex_lines > 0)
    );
    add!(
        v,
        "robots.redirected",
        CAT,
        Warning,
        Low,
        "Robots.txt Redirects",
        "robots.txt redirects. Crawlers follow a limited number of hops.",
        "Serve robots.txt directly on each host.",
        |p, x| robots(p, x).is_some_and(|r| r.redirected)
    );
    add!(
        v,
        "robots.relative_sitemap",
        CAT,
        Warning,
        Low,
        "Relative Sitemap URL",
        "Sitemap lines must use full absolute URLs.",
        "Use an absolute URL in the Sitemap directive.",
        |p, x| robots(p, x)
            .is_some_and(|r| r.sitemap_directives.iter().any(|s| !s.starts_with("http")))
    );

    const SM: &str = "Sitemaps";
    let site = |p: &Page, x: &Ctx| x.is_start(p) && x.crawl.site.sitemaps_checked;
    add!(v, "sitemaps.none", SM, Warning, Medium, "No XML Sitemap Found",
        "No sitemap was found in robots.txt or at /sitemap.xml. Sitemaps help search engines find every page.",
        "Publish an XML sitemap and reference it in robots.txt.",
        move |p, x| site(p, x) && !x.has_sitemap());
    add!(
        v,
        "sitemaps.error",
        SM,
        Issue,
        High,
        "Sitemap Returns Error",
        "A sitemap listed in robots.txt or a sitemap index does not return 200.",
        "Fix the sitemap URL or remove the reference.",
        move |p, x| site(p, x) && x.crawl.site.sitemaps.iter().any(|s| s.status != 200)
    );
    add!(
        v,
        "sitemaps.invalid_xml",
        SM,
        Issue,
        High,
        "Invalid Sitemap XML",
        "A sitemap could not be parsed as XML, so search engines will reject it.",
        "Validate and fix the XML.",
        move |p, x| site(p, x)
            && x.crawl
                .site
                .sitemaps
                .iter()
                .any(|s| s.status == 200 && !s.valid_xml)
    );
    add!(
        v,
        "sitemaps.over_50k",
        SM,
        Issue,
        High,
        "Sitemap Over 50,000 URLs",
        "A sitemap lists more than the 50,000-URL limit.",
        "Split it into several sitemaps under a sitemap index.",
        move |p, x| site(p, x)
            && x.crawl
                .site
                .sitemaps
                .iter()
                .any(|s| !s.is_index && s.url_count > 50_000)
    );
    add!(
        v,
        "sitemaps.over_50mb",
        SM,
        Issue,
        High,
        "Sitemap Over 50 MB",
        "A sitemap is larger than the 50 MB uncompressed limit.",
        "Split the sitemap.",
        move |p, x| site(p, x)
            && x.crawl
                .site
                .sitemaps
                .iter()
                .any(|s| s.size > 50 * 1024 * 1024)
    );
    add!(
        v,
        "sitemaps.not_in_robots",
        SM,
        Opportunity,
        Low,
        "Sitemap Not in Robots.txt",
        "The sitemap exists but robots.txt does not reference it.",
        "Add a Sitemap line to robots.txt.",
        move |p, x| site(p, x)
            && x.crawl
                .site
                .sitemaps
                .iter()
                .any(|s| s.status == 200 && !s.in_robots)
    );
    add!(
        v,
        "sitemaps.invalid_lastmod",
        SM,
        Warning,
        Low,
        "Invalid Lastmod Dates",
        "lastmod values are not valid W3C dates, so search engines ignore them.",
        "Use YYYY-MM-DD or full ISO 8601 timestamps.",
        move |p, x| site(p, x) && x.crawl.site.sitemaps.iter().any(|s| s.invalid_lastmod > 0)
    );
    add!(v, "sitemaps.other_host", SM, Warning, Medium, "Sitemap Lists Other Hosts",
        "The sitemap contains URLs on other hosts, which search engines reject unless cross-host submission is verified.",
        "Keep each sitemap to URLs on its own host.",
        move |p, x| site(p, x) && x.crawl.site.sitemaps.iter().any(|s| s.other_host_urls > 0));
    add!(
        v,
        "sitemaps.empty",
        SM,
        Warning,
        Medium,
        "Empty Sitemap",
        "A valid sitemap lists no URLs.",
        "Fill the sitemap or remove it.",
        move |p, x| site(p, x)
            && x.crawl
                .site
                .sitemaps
                .iter()
                .any(|s| s.valid_xml && s.url_count == 0)
    );
    let ins = |p: &Page, x: &Ctx| p.is_internal && x.in_sitemap(p);
    add!(
        v,
        "sitemaps.redirect",
        SM,
        Warning,
        Medium,
        "Redirect in Sitemap",
        "Sitemaps should only list final URLs, not redirects.",
        "Replace redirecting URLs with their destinations.",
        move |p, x| ins(p, x) && p.is_redirect()
    );
    add!(
        v,
        "sitemaps.client_error",
        SM,
        Issue,
        High,
        "Client Error (4xx) in Sitemap",
        "The sitemap lists broken URLs.",
        "Remove them or fix the pages.",
        move |p, x| ins(p, x) && (400..500).contains(&p.status) && p.status != 429
    );
    add!(
        v,
        "sitemaps.server_error",
        SM,
        Issue,
        High,
        "Server Error (5xx) in Sitemap",
        "Sitemap URLs return server errors.",
        "Fix the server errors.",
        move |p, x| ins(p, x) && p.status >= 500
    );
    add!(
        v,
        "sitemaps.noindex",
        SM,
        Issue,
        Medium,
        "Noindex URL in Sitemap",
        "The sitemap asks for pages to be indexed that themselves say noindex.",
        "Remove noindex URLs from the sitemap.",
        move |p, x| ins(p, x) && ih(p) && p.is_noindex()
    );
    add!(
        v,
        "sitemaps.canonicalised",
        SM,
        Warning,
        Medium,
        "Canonicalised URL in Sitemap",
        "Sitemap URLs that name another URL as canonical.",
        "List only canonical URLs.",
        move |p, x| ins(p, x) && ih(p) && !self_canonical(p)
    );
    add!(
        v,
        "sitemaps.blocked",
        SM,
        Issue,
        Medium,
        "Blocked URL in Sitemap",
        "Sitemap URLs are disallowed by robots.txt.",
        "Unblock them or remove them from the sitemap.",
        move |p, x| ins(p, x) && p.blocked_by_robots
    );
    add!(v, "sitemaps.orphan", SM, Warning, Medium, "Orphan URL (in Sitemap, No Inlinks)",
        "Pages listed in the sitemap that no crawled page links to. They get little internal authority.",
        "Link to them from relevant pages, or remove them if obsolete.",
        move |p, x| ins(p, x) && ih(p) && !x.is_start(p) && !x.linked(p));
    add!(
        v,
        "sitemaps.missing_indexable",
        SM,
        Opportunity,
        Low,
        "Indexable Page Not in Sitemap",
        "Indexable pages missing from the sitemap.",
        "Add every indexable page to the sitemap.",
        |p, x| x.has_sitemap()
            && !x.crawl.site.sitemaps_partial
            && indexable(p)
            && !x.in_sitemap(p)
    );
    add!(
        v,
        "sitemaps.http_url",
        SM,
        Warning,
        Medium,
        "HTTP URL in Sitemap",
        "An HTTPS site's sitemap lists http:// URLs.",
        "List the HTTPS versions.",
        move |p, x| ins(p, x)
            && p.url.starts_with("http://")
            && x.crawl.start_url.starts_with("https://")
    );
}

fn hreflang(v: &mut Vec<Check>) {
    const CAT: &str = "Hreflang";
    fn hl(p: &Page) -> Option<&HtmlData> {
        hd(p).filter(|h| !h.hreflang.is_empty())
    }
    /// Codes the page uses for itself (a page may be both "en" and "en-US", or x-default).
    fn self_codes(p: &Page, h: &HtmlData) -> Vec<String> {
        h.hreflang
            .iter()
            .filter(|(_, u)| u == &p.url)
            .map(|(c, _)| c.to_ascii_lowercase())
            .collect()
    }
    add!(v, "hreflang.missing_return", CAT, Issue, High, "Missing Return Links",
        "Pages this page names as alternates do not link back. Hreflang only works when the annotations are reciprocal.",
        "Add matching hreflang tags on every alternate page.",
        |p, x| hl(p).is_some_and(|h| h.hreflang.iter().any(|(_, u)| u != &p.url && x.page(u).and_then(hd).is_some_and(|t| !t.hreflang.iter().any(|(_, back)| back == &p.url)))));
    add!(v, "hreflang.inconsistent_return", CAT, Warning, Medium, "Inconsistent Language in Return Links",
        "An alternate page links back to this page with a different language-region code than this page uses for itself.",
        "Use the same code for a URL everywhere it is referenced.",
        |p, x| hl(p).is_some_and(|h| {
            let me = self_codes(p, h);
            !me.is_empty() && h.hreflang.iter().any(|(_, u)| u != &p.url && x.page(u).and_then(hd).is_some_and(|t| t.hreflang.iter().any(|(c, back)| back == &p.url && !c.eq_ignore_ascii_case("x-default") && !me.contains(&c.to_ascii_lowercase()))))
        }));
    add!(
        v,
        "hreflang.non_canonical",
        CAT,
        Issue,
        Medium,
        "Hreflang to Non-Canonical URL",
        "Alternates point at URLs that canonicalise elsewhere.",
        "Reference only canonical URLs in hreflang.",
        |p, x| hl(p).is_some_and(|h| h
            .hreflang
            .iter()
            .any(|(_, u)| x.page(u).is_some_and(|t| ih(t) && !self_canonical(t))))
    );
    add!(
        v,
        "hreflang.noindex",
        CAT,
        Issue,
        Medium,
        "Hreflang to Noindex URL",
        "Alternates point at noindex pages.",
        "Reference only indexable pages.",
        |p, x| hl(p).is_some_and(|h| h
            .hreflang
            .iter()
            .any(|(_, u)| x.page(u).is_some_and(|t| ih(t) && t.is_noindex())))
    );
    add!(
        v,
        "hreflang.to_redirect",
        CAT,
        Issue,
        Medium,
        "Hreflang to Redirect",
        "Alternate URLs redirect.",
        "Reference the final URLs.",
        |p, x| hl(p).is_some_and(|h| h.hreflang.iter().any(|(_, u)| redirecting(x, u)))
    );
    add!(
        v,
        "hreflang.to_error",
        CAT,
        Issue,
        High,
        "Hreflang to Broken URL",
        "Alternate URLs return errors.",
        "Fix the alternate pages or remove the annotations.",
        |p, x| hl(p).is_some_and(|h| h.hreflang.iter().any(|(_, u)| broken(x, u)))
    );
    add!(v, "hreflang.invalid_code", CAT, Issue, High, "Invalid Language or Region Code",
        "Codes must be ISO 639-1 languages with optional ISO 3166-1 regions (for example en-GB, not en-UK or en_GB).",
        "Correct the codes.",
        |p, _| hl(p).is_some_and(|h| h.hreflang.iter().any(|(c, _)| !valid_hreflang(c))));
    add!(
        v,
        "hreflang.duplicate_code",
        CAT,
        Issue,
        Medium,
        "Same Code for Multiple URLs",
        "One language code points to several different URLs.",
        "Use one URL per language-region code.",
        |p, _| {
            hl(p).is_some_and(|h| {
            let mut m: HashMap<String, &str> = HashMap::new();
            h.hreflang.iter().any(|(c, u)| matches!(m.insert(c.to_ascii_lowercase(), u.as_str()), Some(prev) if prev != u))
        })
        }
    );
    add!(
        v,
        "hreflang.missing_self",
        CAT,
        Warning,
        Medium,
        "Missing Self-Reference",
        "The page's hreflang set does not include the page itself.",
        "Add a hreflang tag for the page's own URL.",
        |p, _| hl(p).is_some_and(|h| !h.hreflang.iter().any(|(_, u)| u == &p.url))
    );
    add!(
        v,
        "hreflang.missing_x_default",
        CAT,
        Opportunity,
        Low,
        "Missing x-default",
        "No x-default fallback for visitors whose language is not covered.",
        "Add hreflang=\"x-default\" pointing at the default page.",
        |p, _| hl(p).is_some_and(|h| !h
            .hreflang
            .iter()
            .any(|(c, _)| c.eq_ignore_ascii_case("x-default")))
    );
    add!(
        v,
        "hreflang.outside_head",
        CAT,
        Issue,
        High,
        "Hreflang Outside <head>",
        "Hreflang links in the body are ignored.",
        "Move them into the <head>.",
        |p, _| hd(p).is_some_and(|h| h.hreflang_outside_head)
    );
    add!(
        v,
        "hreflang.on_non_canonical",
        CAT,
        Warning,
        Medium,
        "Hreflang on Non-Canonical Page",
        "A canonicalised page carries hreflang tags, which Google will not use.",
        "Put hreflang only on canonical pages.",
        |p, _| hl(p).is_some() && !self_canonical(p)
    );
    add!(
        v,
        "hreflang.lang_mismatch",
        CAT,
        Warning,
        Low,
        "Hreflang Conflicts With HTML Lang",
        "The page's own hreflang language differs from its <html lang>.",
        "Make the two agree.",
        |p, _| hl(p).is_some_and(|h| {
            let me: Vec<String> = self_codes(p, h)
                .into_iter()
                .filter(|c| c != "x-default")
                .collect();
            match h.lang.as_deref() {
                Some(l) if !l.trim().is_empty() && !me.is_empty() => {
                    !me.iter().any(|c| lang_primary(c) == lang_primary(l))
                }
                _ => false,
            }
        })
    );
}

fn structured_data(v: &mut Vec<Check>) {
    const CAT: &str = "Structured Data";
    add!(
        v,
        "sd.parse_error",
        CAT,
        Issue,
        High,
        "JSON-LD Parse Error",
        "A JSON-LD block is not valid JSON, so search engines ignore it.",
        "Fix the syntax (trailing commas and unescaped quotes are common causes).",
        |p, _| hd(p).is_some_and(|h| !h.json_ld_errors.is_empty())
    );
    add!(
        v,
        "sd.missing_context",
        CAT,
        Issue,
        Medium,
        "Missing @context",
        "A JSON-LD block has no @context, so its vocabulary is undefined.",
        "Add \"@context\": \"https://schema.org\".",
        |p, _| hd(p).is_some_and(|h| h
            .json_ld
            .iter()
            .flat_map(|v| match v {
                Value::Array(a) => a.iter().collect::<Vec<_>>(),
                o => vec![o],
            })
            .any(|o| o.is_object() && o.get("@context").is_none()))
    );
    add!(
        v,
        "sd.missing_type",
        CAT,
        Issue,
        Medium,
        "Missing @type",
        "A JSON-LD entity has no @type, so it is ignored.",
        "Add the schema.org type.",
        |p, _| hd(p).is_some_and(|h| sd_roots(h)
            .iter()
            .any(|o| o.is_object() && o.get("@type").is_none()))
    );
    add!(
        v,
        "sd.non_schema_context",
        CAT,
        Warning,
        Low,
        "Non-Schema.org Context",
        "The @context is not schema.org, which Google's rich results require.",
        "Use https://schema.org.",
        |p, _| hd(p).is_some_and(|h| h.json_ld.iter().any(|v| v
            .get("@context")
            .and_then(|c| c.as_str())
            .is_some_and(|c| !c.contains("schema.org"))))
    );
    add!(
        v,
        "sd.none",
        CAT,
        Opportunity,
        Low,
        "No Structured Data",
        "The page has no structured data, so it cannot earn rich results.",
        "Add relevant schema.org markup (Organization, Article, Product, BreadcrumbList…).",
        |p, _| ihd(p).is_some_and(|h| h.json_ld.is_empty()
            && h.json_ld_errors.is_empty()
            && !h.microdata
            && !h.rdfa)
    );
    add!(
        v,
        "sd.data_vocabulary",
        CAT,
        Warning,
        Medium,
        "Deprecated data-vocabulary.org",
        "data-vocabulary.org markup is no longer supported for rich results.",
        "Migrate to schema.org.",
        |p, _| hd(p).is_some_and(|h| h
            .json_ld
            .iter()
            .any(|v| v.to_string().contains("data-vocabulary.org")))
    );
    add!(
        v,
        "sd.invalid_date",
        CAT,
        Warning,
        Medium,
        "Invalid Date Format",
        "Date properties are not in ISO 8601 format, so they may be ignored.",
        "Use ISO 8601 dates such as 2026-11-23 or 2026-11-23T10:00:00+05:30.",
        |p, _| hd(p).is_some_and(|h| sd_items(h).iter().any(|i| [
            "datePublished",
            "dateModified",
            "startDate",
            "endDate",
            "uploadDate",
            "datePosted",
            "validThrough"
        ]
        .iter()
        .any(|k| i
            .get(*k)
            .and_then(|d| d.as_str())
            .is_some_and(|d| !valid_iso_date(d)))))
    );
    add!(
        v,
        "sd.howto_retired",
        CAT,
        Warning,
        Low,
        "HowTo Rich Results Retired",
        "Google no longer shows HowTo rich results.",
        "Keep the markup only if other consumers use it.",
        |p, _| hd(p).is_some_and(|h| sd_items(h)
            .iter()
            .any(|i| type_names(i).iter().any(|t| t == "HowTo")))
    );
    add!(
        v,
        "sd.faq_limited",
        CAT,
        Warning,
        Low,
        "FAQ Rich Results Limited",
        "Since 2023 Google shows FAQ rich results only for well-known government and health sites.",
        "Expect no FAQ rich result unless the site qualifies.",
        |p, _| hd(p).is_some_and(|h| sd_items(h)
            .iter()
            .any(|i| type_names(i).iter().any(|t| t == "FAQPage")))
    );

    // (types, property alternatives, required?, label)
    let props: &[(&[&str], &[&str], bool, &str)] = &[
        (&["Product"], &["name"], true, "Product: name"),
        (
            &["Product"],
            &["offers", "review", "aggregateRating"],
            true,
            "Product: offers, review or aggregateRating",
        ),
        (&["Product"], &["image"], false, "Product: image"),
        (
            &["Product"],
            &["description"],
            false,
            "Product: description",
        ),
        (
            &["Offer"],
            &["price", "priceSpecification"],
            true,
            "Offer: price",
        ),
        (
            &["Offer"],
            &["priceCurrency", "priceSpecification"],
            true,
            "Offer: priceCurrency",
        ),
        (
            &[
                "LocalBusiness",
                "Restaurant",
                "Store",
                "Dentist",
                "MedicalBusiness",
                "AutomotiveBusiness",
                "LodgingBusiness",
                "FoodEstablishment",
                "ProfessionalService",
                "HomeAndConstructionBusiness",
            ],
            &["name"],
            true,
            "LocalBusiness: name",
        ),
        (
            &[
                "LocalBusiness",
                "Restaurant",
                "Store",
                "Dentist",
                "MedicalBusiness",
                "AutomotiveBusiness",
                "LodgingBusiness",
                "FoodEstablishment",
                "ProfessionalService",
                "HomeAndConstructionBusiness",
            ],
            &["address"],
            true,
            "LocalBusiness: address",
        ),
        (
            &[
                "LocalBusiness",
                "Restaurant",
                "Store",
                "Dentist",
                "MedicalBusiness",
                "AutomotiveBusiness",
                "LodgingBusiness",
                "FoodEstablishment",
                "ProfessionalService",
                "HomeAndConstructionBusiness",
            ],
            &["telephone"],
            false,
            "LocalBusiness: telephone",
        ),
        (
            &[
                "LocalBusiness",
                "Restaurant",
                "Store",
                "Dentist",
                "MedicalBusiness",
                "AutomotiveBusiness",
                "LodgingBusiness",
                "FoodEstablishment",
                "ProfessionalService",
                "HomeAndConstructionBusiness",
            ],
            &["openingHoursSpecification", "openingHours"],
            false,
            "LocalBusiness: opening hours",
        ),
        (
            &["BreadcrumbList"],
            &["itemListElement"],
            true,
            "BreadcrumbList: itemListElement",
        ),
        (&["ListItem"], &["position"], true, "ListItem: position"),
        (
            &["ListItem"],
            &["name", "item"],
            true,
            "ListItem: name or item",
        ),
        (&["FAQPage"], &["mainEntity"], true, "FAQPage: mainEntity"),
        (&["QAPage"], &["mainEntity"], true, "QAPage: mainEntity"),
        (&["Question"], &["name"], true, "Question: name"),
        (
            &["Question"],
            &["acceptedAnswer", "suggestedAnswer"],
            true,
            "Question: acceptedAnswer or suggestedAnswer",
        ),
        (&["Answer"], &["text"], true, "Answer: text"),
        (
            &[
                "Event",
                "MusicEvent",
                "SportsEvent",
                "BusinessEvent",
                "EducationEvent",
                "Festival",
            ],
            &["name"],
            true,
            "Event: name",
        ),
        (
            &[
                "Event",
                "MusicEvent",
                "SportsEvent",
                "BusinessEvent",
                "EducationEvent",
                "Festival",
            ],
            &["startDate"],
            true,
            "Event: startDate",
        ),
        (
            &[
                "Event",
                "MusicEvent",
                "SportsEvent",
                "BusinessEvent",
                "EducationEvent",
                "Festival",
            ],
            &["location"],
            true,
            "Event: location",
        ),
        (
            &[
                "Event",
                "MusicEvent",
                "SportsEvent",
                "BusinessEvent",
                "EducationEvent",
                "Festival",
            ],
            &["image"],
            false,
            "Event: image",
        ),
        (
            &[
                "Event",
                "MusicEvent",
                "SportsEvent",
                "BusinessEvent",
                "EducationEvent",
                "Festival",
            ],
            &["description"],
            false,
            "Event: description",
        ),
        (&["Recipe"], &["name"], true, "Recipe: name"),
        (&["Recipe"], &["image"], true, "Recipe: image"),
        (
            &["Recipe"],
            &["recipeIngredient"],
            false,
            "Recipe: recipeIngredient",
        ),
        (
            &["Recipe"],
            &["recipeInstructions"],
            false,
            "Recipe: recipeInstructions",
        ),
        (&["JobPosting"], &["title"], true, "JobPosting: title"),
        (
            &["JobPosting"],
            &["datePosted"],
            true,
            "JobPosting: datePosted",
        ),
        (
            &["JobPosting"],
            &["description"],
            true,
            "JobPosting: description",
        ),
        (
            &["JobPosting"],
            &["hiringOrganization"],
            true,
            "JobPosting: hiringOrganization",
        ),
        (
            &["JobPosting"],
            &["jobLocation", "jobLocationType"],
            true,
            "JobPosting: jobLocation",
        ),
        (
            &["JobPosting"],
            &["validThrough"],
            false,
            "JobPosting: validThrough",
        ),
        (
            &["JobPosting"],
            &["baseSalary"],
            false,
            "JobPosting: baseSalary",
        ),
        (&["VideoObject"], &["name"], true, "VideoObject: name"),
        (
            &["VideoObject"],
            &["thumbnailUrl"],
            true,
            "VideoObject: thumbnailUrl",
        ),
        (
            &["VideoObject"],
            &["uploadDate"],
            true,
            "VideoObject: uploadDate",
        ),
        (
            &["VideoObject"],
            &["description"],
            false,
            "VideoObject: description",
        ),
        (
            &["VideoObject"],
            &["contentUrl", "embedUrl"],
            false,
            "VideoObject: contentUrl or embedUrl",
        ),
        (&["Review"], &["author"], true, "Review: author"),
        (&["Review"], &["reviewRating"], true, "Review: reviewRating"),
        (
            &["AggregateRating"],
            &["ratingValue"],
            true,
            "AggregateRating: ratingValue",
        ),
        (
            &["AggregateRating"],
            &["ratingCount", "reviewCount"],
            true,
            "AggregateRating: ratingCount or reviewCount",
        ),
        (&["Course"], &["name"], true, "Course: name"),
        (&["Course"], &["description"], true, "Course: description"),
        (
            &[
                "SoftwareApplication",
                "MobileApplication",
                "WebApplication",
                "VideoGame",
            ],
            &["name"],
            true,
            "SoftwareApplication: name",
        ),
        (
            &[
                "SoftwareApplication",
                "MobileApplication",
                "WebApplication",
                "VideoGame",
            ],
            &["aggregateRating", "review"],
            true,
            "SoftwareApplication: aggregateRating or review",
        ),
        (&["Dataset"], &["name"], true, "Dataset: name"),
        (&["Dataset"], &["description"], true, "Dataset: description"),
        (&["Movie"], &["name"], true, "Movie: name"),
        (&["Movie"], &["image"], true, "Movie: image"),
        (
            &[
                "Article",
                "NewsArticle",
                "BlogPosting",
                "TechArticle",
                "ScholarlyArticle",
                "Report",
            ],
            &["headline"],
            false,
            "Article: headline",
        ),
        (
            &[
                "Article",
                "NewsArticle",
                "BlogPosting",
                "TechArticle",
                "ScholarlyArticle",
                "Report",
            ],
            &["image"],
            false,
            "Article: image",
        ),
        (
            &[
                "Article",
                "NewsArticle",
                "BlogPosting",
                "TechArticle",
                "ScholarlyArticle",
                "Report",
            ],
            &["datePublished"],
            false,
            "Article: datePublished",
        ),
        (
            &[
                "Article",
                "NewsArticle",
                "BlogPosting",
                "TechArticle",
                "ScholarlyArticle",
                "Report",
            ],
            &["dateModified"],
            false,
            "Article: dateModified",
        ),
        (
            &[
                "Article",
                "NewsArticle",
                "BlogPosting",
                "TechArticle",
                "ScholarlyArticle",
                "Report",
            ],
            &["author"],
            false,
            "Article: author",
        ),
        (
            &[
                "Organization",
                "Corporation",
                "NGO",
                "EducationalOrganization",
            ],
            &["name"],
            false,
            "Organization: name",
        ),
        (
            &[
                "Organization",
                "Corporation",
                "NGO",
                "EducationalOrganization",
            ],
            &["url"],
            false,
            "Organization: url",
        ),
        (
            &[
                "Organization",
                "Corporation",
                "NGO",
                "EducationalOrganization",
            ],
            &["logo"],
            false,
            "Organization: logo",
        ),
        (&["WebSite"], &["name"], false, "WebSite: name"),
    ];
    for &(types, alts, required, label) in props {
        let types: Vec<String> = types.iter().map(|s| s.to_string()).collect();
        let alts: Vec<String> = alts.iter().map(|s| s.to_string()).collect();
        let offer_rule = types == ["Offer"];
        let slug = label
            .to_lowercase()
            .replace([':', ','], "")
            .replace(' ', "_");
        let (kind, priority, what) = if required {
            (Kind::Issue, Priority::Medium, "required")
        } else {
            (Kind::Opportunity, Priority::Low, "recommended")
        };
        v.push(Check {
            id: format!("sd.{slug}"),
            category: CAT,
            name: format!("{label} Missing"),
            kind,
            priority,
            description: format!(
                "{} markup lacks the {what} property {}{}.",
                label.split(':').next().unwrap_or(label),
                alts.join(" / "),
                if required {
                    ", so it is not eligible for rich results"
                } else {
                    ", which Google recommends for richer results"
                }
            ),
            fix: format!(
                "Add {} to the {} entity.",
                alts.join(" or "),
                label.split(':').next().unwrap_or(label)
            ),
            test: t(move |p, _| {
                hd(p).is_some_and(|h| {
                    let items = sd_items(h);
                    // Google only requires Offer price data for Product offers; a service
                    // catalogue Offer without a price is valid.
                    // Recommended properties are judged on top-level entities only: a nested
                    // Organization (a client, a sourceOrganization) is an attribute, not the subject.
                    let candidates = if offer_rule {
                        product_offers(&items)
                    } else if required {
                        items.clone()
                    } else {
                        sd_roots(h)
                    };
                    candidates.iter().any(|i| {
                        let names = type_names(i);
                        names.iter().any(|n| types.contains(n))
                            && !sd_ref(i)
                            && !alts.iter().any(|a| sd_has(&items, i, a))
                    })
                })
            }),
        });
    }
}

fn social(v: &mut Vec<Check>) {
    const CAT: &str = "Social";
    for (key, label, pri) in [
        ("og:title", "Open Graph Title", Priority::Low),
        ("og:description", "Open Graph Description", Priority::Low),
        ("og:image", "Open Graph Image", Priority::Medium),
        ("og:url", "Open Graph URL", Priority::Low),
        ("og:type", "Open Graph Type", Priority::Low),
        ("og:site_name", "Open Graph Site Name", Priority::Low),
    ] {
        v.push(Check {
            id: format!("social.missing_{}", key.replace(':', "_")),
            category: CAT,
            name: format!("Missing {label}"),
            kind: Kind::Opportunity,
            priority: pri,
            description: format!("No {key} tag. Shared links on Facebook, LinkedIn, WhatsApp and Slack will look plainer or pick the wrong details."),
            fix: format!("Add <meta property=\"{key}\" content=\"…\">."),
            test: t(move |p, _| ihd(p).is_some_and(|h| og(h, key).is_none())),
        });
    }
    add!(
        v,
        "social.og_image_relative",
        CAT,
        Warning,
        Low,
        "Open Graph Image Relative URL",
        "og:image must be an absolute URL or social networks cannot load it.",
        "Use a full https:// URL.",
        |p, _| hd(p).is_some_and(|h| og(h, "og:image")
            .is_some_and(|u| !u.trim().is_empty() && !u.trim().starts_with("http")))
    );
    add!(
        v,
        "social.og_image_http",
        CAT,
        Warning,
        Low,
        "Open Graph Image Over HTTP",
        "An insecure og:image may be refused by some platforms.",
        "Serve the share image over HTTPS.",
        |p, _| hd(p).is_some_and(
            |h| p.is_https() && og(h, "og:image").is_some_and(|u| u.starts_with("http://"))
        )
    );
    add!(
        v,
        "social.og_url_mismatch",
        CAT,
        Warning,
        Low,
        "Open Graph URL Differs From Canonical",
        "og:url points somewhere other than the canonical URL, splitting share counts.",
        "Set og:url to the canonical URL.",
        |p, _| hd(p).is_some_and(|h| og(h, "og:url").is_some_and(|u| {
            let target = if first(&h.canonicals).is_empty() {
                p.url.as_str()
            } else {
                first(&h.canonicals)
            };
            !u.trim().is_empty() && u.trim().trim_end_matches('/') != target.trim_end_matches('/')
        }))
    );
    add!(
        v,
        "social.og_multiple_titles",
        CAT,
        Warning,
        Low,
        "Multiple Open Graph Titles",
        "More than one og:title tag; platforms may pick the wrong one.",
        "Keep one og:title.",
        |p, _| hd(p).is_some_and(|h| h.og.iter().filter(|(k, _)| k == "og:title").count() > 1)
    );
    add!(
        v,
        "social.empty_tags",
        CAT,
        Warning,
        Low,
        "Empty Social Tags",
        "Open Graph or Twitter tags with empty content.",
        "Fill in or remove the empty tags.",
        |p, _| hd(p).is_some_and(|h| h
            .og
            .iter()
            .chain(h.twitter.iter())
            .any(|(_, v)| v.trim().is_empty()))
    );
    add!(
        v,
        "social.missing_twitter_card",
        CAT,
        Opportunity,
        Low,
        "Missing Twitter Card",
        "No twitter:card tag, so X (Twitter) shows a basic link instead of a card.",
        "Add <meta name=\"twitter:card\" content=\"summary_large_image\">.",
        |p, _| ihd(p).is_some_and(|h| tw(h, "twitter:card").is_none())
    );
    add!(
        v,
        "social.invalid_twitter_card",
        CAT,
        Warning,
        Low,
        "Invalid Twitter Card Type",
        "twitter:card must be summary, summary_large_image, app or player.",
        "Use a valid card type.",
        |p, _| hd(p).is_some_and(|h| tw(h, "twitter:card").is_some_and(|c| ![
            "summary",
            "summary_large_image",
            "app",
            "player"
        ]
        .contains(&c.trim().to_lowercase().as_str())))
    );
}

fn links(v: &mut Vec<Check>) {
    const CAT: &str = "Links";
    fn internal_link(x: &Ctx, url: &str, from: &Page) -> bool {
        x.page(url)
            .map(|t| t.is_internal)
            .unwrap_or_else(|| host(url) == host(&from.url))
    }
    add!(
        v,
        "links.internal_broken",
        CAT,
        Issue,
        High,
        "Internal Outlinks to Broken Pages",
        "The page links to internal URLs that return errors.",
        "Fix or remove the broken links.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| internal_link(x, &l.url, p) && broken(x, &l.url)))
    );
    add!(
        v,
        "links.external_broken",
        CAT,
        Warning,
        Medium,
        "External Outlinks to Broken Pages",
        "The page links to external URLs that return errors or no response.",
        "Update or remove the dead external links.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| x.page(&l.url).is_some_and(|t| !t.is_internal) && broken(x, &l.url)))
    );
    add!(
        v,
        "links.internal_redirects",
        CAT,
        Warning,
        Low,
        "Internal Outlinks to Redirects",
        "The page links to internal URLs that redirect. Each one costs an extra request.",
        "Link directly to the final URL.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| internal_link(x, &l.url, p) && redirecting(x, &l.url)))
    );
    add!(
        v,
        "links.internal_nofollow",
        CAT,
        Warning,
        Low,
        "Internal Nofollow Outlinks",
        "Internal links marked nofollow stop link value flowing through your own site.",
        "Remove nofollow from internal links.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| l.nofollow && internal_link(x, &l.url, p)))
    );
    add!(
        v,
        "links.no_anchor_text",
        CAT,
        Warning,
        Low,
        "Internal Outlinks Without Anchor Text",
        "Internal links with no text, alt text or label give no context about the target.",
        "Use descriptive link text.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| l.anchor.is_empty() && internal_link(x, &l.url, p)))
    );
    add!(
        v,
        "links.non_descriptive_anchor",
        CAT,
        Opportunity,
        Low,
        "Non-Descriptive Anchor Text",
        "Links such as 'click here' or 'read more' tell users and search engines nothing.",
        "Describe the destination in the link text.",
        |p, _| hd(p).is_some_and(|h| h.generic_anchor_links > 0)
    );
    add!(v, "links.http_from_https", CAT, Warning, Medium, "Links to HTTP Internal URLs",
        "An HTTPS page links to HTTP versions of internal URLs, causing redirects or mixed signals.",
        "Update links to HTTPS.",
        |p, _| hd(p).is_some_and(|h| p.is_https() && h.links.iter().any(|l| l.url.starts_with("http://") && bare_host(&l.url) == bare_host(&p.url))));
    add!(
        v,
        "links.too_many",
        CAT,
        Opportunity,
        Low,
        "Over 150 Outlinks",
        "Pages with a very large number of links dilute the value passed to each.",
        "Trim navigation and footer links to what matters.",
        |p, _| hd(p).is_some_and(|h| h.links.len() > 150)
    );
    add!(
        v,
        "links.many_external",
        CAT,
        Warning,
        Low,
        "Over 50 External Outlinks",
        "Many external links can look spammy and leak visitors.",
        "Review whether every external link is needed.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .filter(|l| !internal_link(x, &l.url, p))
            .count()
            > 50)
    );
    add!(
        v,
        "links.no_internal_outlinks",
        CAT,
        Warning,
        Medium,
        "No Internal Outlinks",
        "A dead-end page with no links into the rest of the site.",
        "Add relevant internal links and navigation.",
        |p, x| hd(p).is_some_and(|h| !h
            .links
            .iter()
            .any(|l| internal_link(x, &l.url, p) && l.url != p.url))
    );
    add!(
        v,
        "links.no_inlinks",
        CAT,
        Warning,
        Medium,
        "No Internal Inlinks",
        "No crawled page links to this URL, so search engines and visitors can barely find it.",
        "Link to it from relevant pages.",
        |p, x| ih(p) && !x.is_start(p) && real(p) && p.source != Some(Source::Seed) && !x.linked(p)
    );
    add!(
        v,
        "links.single_inlink",
        CAT,
        Opportunity,
        Low,
        "Only One Followed Inlink",
        "The page has a single followed internal link pointing at it.",
        "Add more contextual internal links to important pages.",
        |p, x| indexable(p) && !x.is_start(p) && {
            let pages = &x.crawl.pages;
            x.inlinks_of(p)
                .iter()
                .filter(|(s, l)| {
                    !pages[*s]
                        .html
                        .as_ref()
                        .map(|h| h.links[*l].nofollow)
                        .unwrap_or(true)
                })
                .count()
                == 1
        }
    );
    add!(
        v,
        "links.only_nofollow_inlinks",
        CAT,
        Warning,
        Medium,
        "Only Nofollow Inlinks",
        "Every internal link to the page is nofollow, so no internal link value reaches it.",
        "Use followed links to pages you want indexed.",
        |p, x| ih(p) && {
            let pages = &x.crawl.pages;
            let ins = x.inlinks_of(p);
            !ins.is_empty()
                && ins.iter().all(|(s, l)| {
                    pages[*s]
                        .html
                        .as_ref()
                        .map(|h| h.links[*l].nofollow)
                        .unwrap_or(false)
                })
        }
    );
    add!(
        v,
        "links.javascript_href",
        CAT,
        Warning,
        Low,
        "JavaScript Links",
        "Links with href=\"javascript:…\" cannot be followed by crawlers.",
        "Use real URLs in href and attach behaviour with event listeners.",
        |p, _| hd(p).is_some_and(|h| h.javascript_links > 0)
    );
    add!(
        v,
        "links.whitespace_href",
        CAT,
        Warning,
        Low,
        "Whitespace in Link URLs",
        "Link hrefs containing spaces or stray whitespace often break.",
        "Remove or encode whitespace in URLs.",
        |p, _| hd(p).is_some_and(|h| h.links_with_whitespace > 0)
    );
    add!(v, "links.broken_bookmark", CAT, Warning, Low, "Broken Bookmarks",
        "Links to #fragments that do not exist on the target page, so visitors land at the top instead.",
        "Fix the fragment or add the matching id on the target page.",
        |p, x| hd(p).is_some_and(|h| h.links.iter().any(|l| l.fragment.as_deref().is_some_and(|f| {
            !f.starts_with('!') && !f.starts_with('/') && !f.contains('=') && f != "top" && f != "main" && f != "content"
                && x.page(&l.url).and_then(hd).is_some_and(|t| t.ids.binary_search(&f.to_string()).is_err() && t.ids.len() < 5000)
        }))));
    add!(v, "links.local_or_private", CAT, Issue, Medium, "Links to Localhost or Private IPs",
        "Links point at localhost, .local or private-network addresses, usually leftovers from development.",
        "Replace them with public URLs.",
        |p, _| hd(p).is_some_and(|h| h.links.iter().any(|l| private_host(&host(&l.url)))) && !private_host(&host(&p.url)));
    add!(v, "links.depth_4_5", CAT, Opportunity, Low, "Crawl Depth 4–5",
        "Pages four or five clicks from the start page get crawled less often and receive less authority.",
        "Link to important pages closer to the homepage.",
        |p, _| ih(p) && (4..=5).contains(&p.depth) && p.source != Some(Source::Sitemap));
    add!(
        v,
        "links.depth_over_5",
        CAT,
        Warning,
        Medium,
        "Crawl Depth Over 5",
        "Pages buried more than five clicks deep are hard for crawlers and visitors to reach.",
        "Flatten the architecture with hub pages and contextual links.",
        |p, _| ih(p) && p.depth > 5 && p.source != Some(Source::Sitemap)
    );
    add!(
        v,
        "links.internal_sponsored_ugc",
        CAT,
        Warning,
        Low,
        "Internal Links Marked Sponsored or UGC",
        "rel=sponsored and rel=ugc are meant for external links.",
        "Remove those rel values from internal links.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| (l.sponsored || l.ugc) && internal_link(x, &l.url, p)))
    );
    add!(
        v,
        "links.to_blocked",
        CAT,
        Warning,
        Low,
        "Links to Blocked URLs",
        "The page links to internal URLs that robots.txt blocks.",
        "Remove the links or unblock the URLs.",
        |p, x| hd(p).is_some_and(|h| h
            .links
            .iter()
            .any(|l| x.page(&l.url).is_some_and(|t| t.blocked_by_robots)))
    );
    add!(
        v,
        "links.image_link_no_alt",
        CAT,
        Warning,
        Medium,
        "Image Links Without Alt Text",
        "Linked images with no alt text leave the link without any name.",
        "Add alt text describing the link destination.",
        |p, _| hd(p).is_some_and(|h| h.links.iter().any(|l| l.image_link && l.anchor.is_empty()))
    );
}

fn resources(v: &mut Vec<Check>) {
    const CAT: &str = "Resources";
    add!(
        v,
        "resources.broken_js",
        CAT,
        Issue,
        High,
        "Broken JavaScript Files",
        "Script files referenced by the page return errors, which can break features.",
        "Fix the script URLs or remove the references.",
        |p, x| hd(p).is_some_and(|h| h.scripts.iter().any(|s| broken(x, &s.url)))
    );
    add!(
        v,
        "resources.broken_css",
        CAT,
        Issue,
        High,
        "Broken CSS Files",
        "Stylesheets referenced by the page return errors, so it may render unstyled.",
        "Fix the stylesheet URLs.",
        |p, x| hd(p).is_some_and(|h| h.stylesheets.iter().any(|s| broken(x, s)))
    );
    add!(
        v,
        "resources.redirected",
        CAT,
        Warning,
        Low,
        "Redirected CSS or JavaScript",
        "Script or stylesheet URLs redirect, delaying rendering.",
        "Reference the final URLs.",
        |p, x| hd(p).is_some_and(|h| h
            .scripts
            .iter()
            .map(|s| &s.url)
            .chain(h.stylesheets.iter())
            .any(|u| redirecting(x, u)))
    );
    add!(v, "resources.blocked", CAT, Warning, Medium, "Uses Resources Blocked by Robots.txt",
        "The page needs CSS, JavaScript or images that robots.txt blocks, so search engines cannot render it properly.",
        "Allow those resources in robots.txt.",
        |p, x| hd(p).is_some_and(|h| h.scripts.iter().map(|s| &s.url).chain(h.stylesheets.iter()).chain(h.images.iter().map(|i| &i.url)).any(|u| x.page(u).is_some_and(|t| t.blocked_by_robots))));
    add!(
        v,
        "resources.many_scripts",
        CAT,
        Opportunity,
        Low,
        "Over 20 Script Files",
        "Many separate scripts add requests and parsing time.",
        "Bundle scripts and remove unused ones.",
        |p, _| hd(p).is_some_and(|h| h.scripts.len() > 20)
    );
    add!(
        v,
        "resources.many_stylesheets",
        CAT,
        Opportunity,
        Low,
        "Over 10 Stylesheets",
        "Many stylesheets delay rendering.",
        "Combine stylesheets and inline critical CSS.",
        |p, _| hd(p).is_some_and(|h| h.stylesheets.len() > 10)
    );
    add!(
        v,
        "resources.large_inline_js",
        CAT,
        Opportunity,
        Low,
        "Over 50 KB Inline JavaScript",
        "Large inline scripts bloat every HTML response and cannot be cached.",
        "Move large scripts into cacheable external files.",
        |p, _| hd(p).is_some_and(|h| h.inline_script_bytes > 50 * 1024)
    );
    add!(
        v,
        "resources.large_inline_css",
        CAT,
        Opportunity,
        Low,
        "Over 50 KB Inline CSS",
        "Large inline styles bloat every HTML response.",
        "Inline only critical CSS and load the rest from a file.",
        |p, _| hd(p).is_some_and(|h| h.inline_styles_bytes > 50 * 1024)
    );
    add!(
        v,
        "resources.document_write",
        CAT,
        Warning,
        Low,
        "Uses document.write()",
        "document.write blocks parsing, and Chrome may block it on slow connections.",
        "Insert content with DOM methods instead.",
        |p, _| hd(p).is_some_and(|h| h.document_write)
    );
    add!(v, "resources.js_rendered", CAT, Warning, Medium, "Content Likely Rendered by JavaScript",
        "The HTML has almost no text but heavy scripts, so the content probably appears only after JavaScript runs. Rendering is delayed and not guaranteed for every crawler.",
        "Server-render or pre-render the main content.",
        |p, _| ihd(p).is_some_and(js_rendered));
    add!(
        v,
        "resources.many_third_party",
        CAT,
        Opportunity,
        Low,
        "Scripts From Over 10 Third-Party Domains",
        "Every third-party domain adds DNS, connection and privacy cost.",
        "Audit third-party tags and remove what is not essential.",
        |p, _| hd(p).is_some_and(|h| {
            let me = bare_host(&p.url);
            h.scripts
                .iter()
                .map(|s| bare_host(&s.url))
                .filter(|d| d != &me)
                .collect::<HashSet<_>>()
                .len()
                > 10
        })
    );
}

fn performance(v: &mut Vec<Check>) {
    const CAT: &str = "Performance";
    add!(
        v,
        "performance.slow_1s",
        CAT,
        Warning,
        Medium,
        "Response Time 1–3 Seconds",
        "The server took over a second to deliver the HTML, delaying everything else.",
        "Add caching, optimise database queries or use a CDN.",
        |p, _| ih(p) && (1000..3000).contains(&p.response_ms)
    );
    add!(
        v,
        "performance.slow_3s",
        CAT,
        Issue,
        High,
        "Response Time Over 3 Seconds",
        "Very slow server responses lose visitors and reduce how much search engines crawl.",
        "Investigate server performance urgently.",
        |p, _| ih(p) && p.response_ms >= 3000
    );
    add!(
        v,
        "performance.large_html",
        CAT,
        Warning,
        Medium,
        "HTML Over 1 MB",
        "Very large HTML documents are slow to download and parse.",
        "Remove inline data and unused markup, and paginate long lists.",
        |p, _| ih(p) && (1024 * 1024..15 * 1024 * 1024).contains(&p.size)
    );
    add!(
        v,
        "performance.html_truncated",
        CAT,
        Issue,
        High,
        "HTML Over 15 MB",
        "Googlebot only reads the first 15 MB of an HTML file; the rest is ignored.",
        "Reduce the document size drastically.",
        |p, _| p.is_internal && p.html.is_some() && p.size >= 15 * 1024 * 1024
    );
    add!(v, "performance.uncompressed", CAT, Opportunity, Medium, "Text Not Compressed",
        "HTML, CSS or JavaScript sent without gzip or Brotli compression, often 3–5 times larger than needed.",
        "Enable Brotli or gzip compression on the server.",
        |p, _| p.is_internal && p.status == 200 && matches!(p.kind(), "HTML" | "CSS" | "JavaScript") && p.content_encoding.is_empty() && p.size > 1400);
    add!(
        v,
        "performance.no_http2",
        CAT,
        Opportunity,
        Low,
        "Not Served Over HTTP/2",
        "HTTP/1.1 loads resources less efficiently than HTTP/2 or HTTP/3.",
        "Enable HTTP/2 on the server or CDN.",
        |p, _| ih(p) && p.http_version.starts_with("HTTP/1")
    );
    add!(v, "performance.no_cache_headers", CAT, Opportunity, Medium, "Static Files Without Cache Headers",
        "Images, CSS and JavaScript without Cache-Control or Expires get downloaded again on every visit.",
        "Send long Cache-Control max-age values for versioned static files.",
        |p, _| is_static(p) && !header_has(p, "cache-control") && !header_has(p, "expires"));
    add!(
        v,
        "performance.short_cache",
        CAT,
        Opportunity,
        Low,
        "Static Files Cached Under 7 Days",
        "Short cache lifetimes make returning visitors download files again.",
        "Use max-age of at least a week, ideally a year with versioned file names.",
        |p, _| is_static(p)
            && p.header("cache-control")
                .and_then(max_age)
                .is_some_and(|a| a < 7 * 24 * 3600)
    );
    add!(
        v,
        "performance.no_validators",
        CAT,
        Opportunity,
        Low,
        "Static Files Without ETag or Last-Modified",
        "Without validators, browsers cannot cheaply check whether a cached file changed.",
        "Enable ETag or Last-Modified headers.",
        |p, _| is_static(p) && !header_has(p, "etag") && !header_has(p, "last-modified")
    );
    add!(v, "performance.render_blocking_js", CAT, Opportunity, Medium, "Render-Blocking Scripts",
        "Scripts in the <head> without async or defer stop the page rendering until they download and run.",
        "Add defer (or async) to scripts in the head.",
        |p, _| hd(p).is_some_and(|h| h.scripts.iter().any(|s| s.in_head && !s.is_async && !s.defer && !s.module)));
    add!(
        v,
        "performance.many_render_blocking_css",
        CAT,
        Opportunity,
        Low,
        "Over 5 Render-Blocking Stylesheets",
        "Each stylesheet in the head must load before the page paints.",
        "Combine stylesheets and inline critical CSS.",
        |p, _| hd(p).is_some_and(|h| h.stylesheets.len() > 5)
    );
    add!(
        v,
        "performance.dom_size",
        CAT,
        Opportunity,
        Medium,
        "Excessive DOM Size",
        "More than 1,500 elements slows rendering and interaction (INP).",
        "Simplify markup and render long lists on demand.",
        |p, _| hd(p).is_some_and(|h| h.dom_elements > 1500)
    );
    add!(
        v,
        "performance.many_requests",
        CAT,
        Opportunity,
        Low,
        "Over 75 Resources",
        "The page references more than 75 images, scripts, stylesheets and frames.",
        "Combine, lazy-load or remove resources.",
        |p, _| hd(p).is_some_and(|h| h.images.len()
            + h.scripts.len()
            + h.stylesheets.len()
            + h.iframes.len()
            > 75)
    );
    add!(
        v,
        "performance.iframe_no_size",
        CAT,
        Opportunity,
        Low,
        "Iframes Without Dimensions",
        "Iframes without width and height cause layout shift as they load.",
        "Set width and height, or reserve space with CSS.",
        |p, _| hd(p).is_some_and(|h| h.iframes.iter().any(|f| !f.has_size))
    );
    add!(v, "performance.font_display", CAT, Opportunity, Low, "Web Fonts Without font-display",
        "@font-face rules without font-display can hide text while fonts load and cause layout shift.",
        "Add font-display: swap (or optional).",
        |p, _| hd(p).is_some_and(|h| h.font_face_without_display));
    add!(v, "performance.page_weight", CAT, Warning, Medium, "Page Weight Over 3 MB",
        "The HTML plus measured images, CSS and scripts exceed 3 MB, which is slow on mobile networks.",
        "Compress images, trim scripts and lazy-load media.",
        |p, x| hd(p).is_some_and(|h| {
            let res: usize = h.images.iter().map(|i| &i.url).chain(h.scripts.iter().map(|s| &s.url)).chain(h.stylesheets.iter())
                .collect::<HashSet<_>>().into_iter().filter_map(|u| x.page(u)).map(|t| t.transfer_size.max(t.size)).sum();
            p.transfer_size + res > 3 * 1024 * 1024
        }));
    add!(
        v,
        "performance.large_js",
        CAT,
        Opportunity,
        Medium,
        "JavaScript File Over 500 KB",
        "Large script files take long to download, parse and run.",
        "Split the bundle, tree-shake and minify.",
        |p, _| p.is_internal && p.status == 200 && p.kind() == "JavaScript" && p.size > 500 * 1024
    );
    add!(
        v,
        "performance.large_css",
        CAT,
        Opportunity,
        Low,
        "CSS File Over 200 KB",
        "Large stylesheets delay first paint.",
        "Remove unused CSS and split per page.",
        |p, _| p.is_internal && p.status == 200 && p.kind() == "CSS" && p.size > 200 * 1024
    );
}

fn mobile_accessibility(v: &mut Vec<Check>) {
    const M: &str = "Mobile";
    add!(
        v,
        "mobile.missing_viewport",
        M,
        Issue,
        High,
        "Missing Viewport",
        "Without a viewport meta tag, phones render the desktop layout zoomed out.",
        "Add <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">.",
        |p, _| hd(p).is_some_and(|h| h.viewport.is_none())
    );
    add!(
        v,
        "mobile.viewport_not_device_width",
        M,
        Warning,
        Medium,
        "Viewport Not Device Width",
        "The viewport is fixed instead of matching the screen width.",
        "Use width=device-width.",
        |p, _| hd(p).is_some_and(|h| h.viewport.as_deref().is_some_and(|vp| !vp
            .replace(' ', "")
            .to_lowercase()
            .contains("width=device-width")))
    );
    add!(v, "mobile.zoom_disabled", M, Warning, Medium, "Zoom Disabled",
        "user-scalable=no or a low maximum-scale stops people zooming, which fails accessibility guidelines.",
        "Remove user-scalable=no and maximum-scale limits.",
        |p, _| hd(p).is_some_and(|h| h.viewport.as_deref().is_some_and(|vp| {
            let s = vp.replace(' ', "").to_lowercase();
            s.contains("user-scalable=no") || s.contains("user-scalable=0")
                || s.split(',').find_map(|kv| kv.strip_prefix("maximum-scale=")).and_then(|n| n.parse::<f32>().ok()).is_some_and(|n| n < 2.0)
        })));
    add!(v, "mobile.plugins", M, Issue, Medium, "Uses Plugins",
        "<object>, <embed> or <applet> content such as Flash does not work on mobile or modern browsers.",
        "Replace plugin content with HTML5.",
        |p, _| hd(p).is_some_and(|h| h.uses_plugins));
    add!(
        v,
        "mobile.frames",
        M,
        Issue,
        Medium,
        "Uses Frames",
        "Framesets are obsolete, poorly indexed and unusable on mobile.",
        "Rebuild the page without frames.",
        |p, _| hd(p).is_some_and(|h| h.uses_frames)
    );
    add!(
        v,
        "mobile.no_touch_icon",
        M,
        Opportunity,
        Low,
        "Missing Apple Touch Icon",
        "No apple-touch-icon, so home-screen shortcuts on iOS use a screenshot.",
        "Add a 180×180 apple-touch-icon.",
        |p, x| x.is_start(p) && hd(p).is_some_and(|h| !h.apple_touch_icon)
    );
    add!(
        v,
        "mobile.no_favicon",
        M,
        Opportunity,
        Low,
        "Missing Favicon",
        "No favicon link. Google shows favicons beside mobile results.",
        "Add a favicon of at least 48×48 pixels.",
        |p, x| x.is_start(p) && hd(p).is_some_and(|h| !h.favicon)
    );

    const A: &str = "Accessibility";
    add!(
        v,
        "a11y.missing_lang",
        A,
        Warning,
        Medium,
        "Missing HTML Lang",
        "No lang on <html>, so screen readers may use the wrong pronunciation.",
        "Add <html lang=\"en\"> (or the right language).",
        |p, _| hd(p).is_some_and(|h| h.lang.as_deref().is_none_or(|l| l.trim().is_empty()))
    );
    add!(
        v,
        "a11y.invalid_lang",
        A,
        Warning,
        Medium,
        "Invalid HTML Lang",
        "The lang attribute is not a valid language code.",
        "Use a valid BCP 47 code such as en or en-GB.",
        |p, _| hd(p).is_some_and(|h| h
            .lang
            .as_deref()
            .is_some_and(|l| !l.trim().is_empty() && !valid_html_lang(l)))
    );
    add!(
        v,
        "a11y.links_without_text",
        A,
        Warning,
        Medium,
        "Links Without Discernible Text",
        "Links with no text or label are announced as just 'link'.",
        "Give every link text or an aria-label.",
        |p, _| hd(p).is_some_and(|h| h.empty_links > 0)
    );
    add!(
        v,
        "a11y.buttons_without_name",
        A,
        Warning,
        Medium,
        "Buttons Without Accessible Name",
        "Buttons with no text or label cannot be understood by screen-reader users.",
        "Add text or an aria-label.",
        |p, _| hd(p).is_some_and(|h| h.buttons_without_name > 0)
    );
    add!(
        v,
        "a11y.inputs_without_label",
        A,
        Warning,
        Medium,
        "Form Fields Without Labels",
        "Inputs with no associated label are hard to use with assistive technology.",
        "Associate a <label> with every field.",
        |p, _| hd(p).is_some_and(|h| h.inputs_without_label > 0)
    );
    add!(
        v,
        "a11y.iframe_title",
        A,
        Warning,
        Low,
        "Iframes Without Title",
        "Iframes without a title are announced without context.",
        "Add a descriptive title attribute.",
        |p, _| hd(p).is_some_and(|h| h.iframes.iter().any(|f| !f.has_title))
    );
    add!(
        v,
        "a11y.duplicate_ids",
        A,
        Warning,
        Low,
        "Duplicate Element IDs",
        "Repeated id values break label associations, ARIA references and in-page links.",
        "Make every id unique.",
        |p, _| hd(p).is_some_and(|h| h.duplicate_ids > 0)
    );
    add!(
        v,
        "a11y.positive_tabindex",
        A,
        Warning,
        Low,
        "Positive Tabindex",
        "tabindex values above 0 override the natural keyboard order and confuse keyboard users.",
        "Use tabindex 0 or -1 and order the DOM instead.",
        |p, _| hd(p).is_some_and(|h| h.positive_tabindex > 0)
    );
    add!(
        v,
        "a11y.missing_main",
        A,
        Opportunity,
        Low,
        "Missing Main Landmark",
        "No <main> element, so screen-reader users cannot jump straight to the content.",
        "Wrap the primary content in <main>.",
        |p, _| ihd(p).is_some_and(|h| !h.has_main)
    );
    add!(
        v,
        "a11y.autoplay",
        A,
        Warning,
        Medium,
        "Autoplaying Media With Sound",
        "Audio or unmuted video that plays automatically disrupts visitors and screen readers.",
        "Don't autoplay with sound; offer play controls.",
        |p, _| hd(p).is_some_and(|h| h.autoplay_media)
    );
    add!(
        v,
        "a11y.video_captions",
        A,
        Warning,
        Medium,
        "Videos Without Captions",
        "<video> elements with no captions track exclude deaf and hard-of-hearing visitors.",
        "Add <track kind=\"captions\">.",
        |p, _| hd(p).is_some_and(|h| h.videos_without_captions > 0)
    );
    add!(
        v,
        "a11y.table_headers",
        A,
        Opportunity,
        Low,
        "Tables Without Header Cells",
        "Data tables with no <th> cells are hard to understand with a screen reader.",
        "Mark header cells with <th> (or role=presentation for layout tables).",
        |p, _| hd(p).is_some_and(|h| h.tables_without_headers > 0)
    );
    add!(
        v,
        "a11y.aria_hidden_body",
        A,
        Issue,
        High,
        "Body Hidden From Assistive Technology",
        "aria-hidden=\"true\" on <body> hides the entire page from screen readers.",
        "Remove aria-hidden from the body.",
        |p, _| hd(p).is_some_and(|h| h.aria_hidden_body)
    );
}

fn validation(v: &mut Vec<Check>) {
    const CAT: &str = "Validation";
    add!(
        v,
        "validation.missing_doctype",
        CAT,
        Warning,
        Medium,
        "Missing Doctype",
        "Without <!DOCTYPE html>, browsers use quirks mode and layouts can break.",
        "Start the document with <!DOCTYPE html>.",
        |p, _| hd(p).is_some_and(|h| !h.has_doctype)
    );
    add!(
        v,
        "validation.missing_head",
        CAT,
        Warning,
        Low,
        "Missing <head> Element",
        "The document has no <head> tag, relying on the parser to infer it.",
        "Add explicit <head> and <body> elements.",
        |p, _| hd(p).is_some_and(|h| h.head_tags == 0)
    );
    add!(
        v,
        "validation.multiple_head",
        CAT,
        Issue,
        High,
        "Multiple <head> Elements",
        "More than one <head>. Metadata in the extra one may be ignored.",
        "Merge into a single <head>.",
        |p, _| hd(p).is_some_and(|h| h.head_tags > 1)
    );
    add!(
        v,
        "validation.multiple_body",
        CAT,
        Issue,
        Medium,
        "Multiple <body> Elements",
        "More than one <body> tag, usually from template includes.",
        "Keep a single <body>.",
        |p, _| hd(p).is_some_and(|h| h.body_tags > 1)
    );
    add!(v, "validation.invalid_in_head", CAT, Issue, High, "Invalid Elements in <head>",
        "Elements such as <div>, <img> or <iframe> in the head make browsers and Google close the head early, so later meta tags (robots, canonical, hreflang) can be ignored.",
        "Move non-metadata elements into the <body>.",
        |p, _| hd(p).is_some_and(|h| !h.invalid_in_head.is_empty()));
    add!(v, "validation.missing_charset", CAT, Warning, Medium, "Missing Character Encoding",
        "Neither the Content-Type header nor a meta tag declares the charset, so text may display garbled.",
        "Add <meta charset=\"utf-8\"> as the first element in the head.",
        |p, _| hd(p).is_some_and(|h| h.charset.is_none() && header_charset(p).is_none()));
    add!(
        v,
        "validation.non_utf8",
        CAT,
        Opportunity,
        Low,
        "Not UTF-8",
        "The page uses a legacy encoding instead of UTF-8.",
        "Convert content to UTF-8.",
        |p, _| hd(p).is_some_and(|h| h
            .charset
            .as_deref()
            .map(charset_norm)
            .or_else(|| header_charset(p))
            .is_some_and(|c| c != "utf-8" && c != "utf8"))
    );
    add!(
        v,
        "validation.charset_mismatch",
        CAT,
        Warning,
        Medium,
        "Charset Header and Meta Conflict",
        "The HTTP header and the meta tag declare different encodings.",
        "Make them agree (ideally both UTF-8).",
        |p, _| hd(p).is_some_and(|h| {
            match (header_charset(p), h.charset.as_deref().map(charset_norm)) {
                (Some(a), Some(b)) => a.replace("utf8", "utf-8") != b.replace("utf8", "utf-8"),
                _ => false,
            }
        })
    );
    add!(
        v,
        "validation.deprecated_tags",
        CAT,
        Warning,
        Low,
        "Deprecated HTML Elements",
        "Obsolete elements such as <center>, <font> or <marquee>.",
        "Replace them with CSS and modern elements.",
        |p, _| hd(p).is_some_and(|h| !h.deprecated_tags.is_empty())
    );
    add!(
        v,
        "validation.missing_content_type",
        CAT,
        Warning,
        Medium,
        "Missing Content-Type Header",
        "The server sends no Content-Type, so browsers have to guess.",
        "Send Content-Type: text/html; charset=utf-8.",
        |p, _| p.is_internal
            && p.status == 200
            && real(p)
            && p.content_type.is_empty()
            && p.size > 0
    );
    add!(
        v,
        "validation.base_href",
        CAT,
        Warning,
        Low,
        "Uses <base href>",
        "A base element changes how every relative link resolves, a common source of broken links.",
        "Remove <base> and use root-relative links, unless it is required.",
        |p, _| hd(p).is_some_and(|h| h.base_href.is_some())
    );
    add!(
        v,
        "validation.content_language_mismatch",
        CAT,
        Warning,
        Low,
        "Content-Language Conflicts With HTML Lang",
        "The Content-Language header and <html lang> name different languages.",
        "Make them agree.",
        |p, _| hd(p).is_some_and(
            |h| match (p.header("content-language"), h.lang.as_deref()) {
                (Some(cl), Some(l)) if !l.trim().is_empty() && !cl.contains(',') =>
                    lang_primary(cl) != lang_primary(l),
                _ => false,
            }
        )
    );

    const AMP: &str = "AMP";
    add!(
        v,
        "amp.link_not_200",
        AMP,
        Issue,
        High,
        "AMP Link Not 200",
        "The page's amphtml link points at a URL that does not return 200.",
        "Fix or remove the amphtml link.",
        |p, x| hd(p).is_some_and(|h| h
            .amphtml
            .as_deref()
            .is_some_and(|u| x.status(u).is_some_and(|s| s != 200)))
    );
    add!(
        v,
        "amp.missing_canonical",
        AMP,
        Issue,
        High,
        "AMP Page Missing Canonical",
        "AMP pages must link to their canonical (non-AMP or self) URL.",
        "Add a canonical link to the AMP page.",
        |p, _| hd(p).is_some_and(|h| h.is_amp && h.canonicals.is_empty())
    );
    add!(
        v,
        "amp.target_not_amp",
        AMP,
        Warning,
        Medium,
        "AMP Link Target Is Not AMP",
        "The amphtml link points at a page that is not marked as AMP.",
        "Point amphtml at the AMP version, or remove it.",
        |p, x| hd(p).is_some_and(|h| h
            .amphtml
            .as_deref()
            .is_some_and(|u| x.page(u).and_then(hd).is_some_and(|t| !t.is_amp)))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_has_over_300_unique_checks() {
        let c = catalogue();
        assert!(c.len() > 300, "only {} checks", c.len());
        let ids: HashSet<&str> = c.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), c.len(), "duplicate check ids");
        for check in c {
            assert!(
                CATEGORIES.contains(&check.category),
                "{} has unknown category {}",
                check.id,
                check.category
            );
            assert!(
                !check.description.is_empty() && !check.fix.is_empty(),
                "{} lacks text",
                check.id
            );
        }
    }

    /// Does check `id` fire on a single internal 200 page with this HTML and headers?
    fn fires(id: &str, html: &str, headers: &[(&str, &str)]) -> bool {
        let url = "https://example.com/page";
        let page = Page {
            id: 1,
            url: url.into(),
            source: Some(Source::Seed),
            is_internal: true,
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            html: Some(Box::new(crate::parse::parse_html(
                html,
                &Url::parse(url).unwrap(),
            ))),
            ..Default::default()
        };
        let crawl = crate::model::Crawl {
            pages: vec![page],
            ..Default::default()
        };
        let audit = crate::audit::run(&crawl);
        !audit.hits[find(id).unwrap()].is_empty()
    }

    /// URLs among several internal 200 HTML pages on which check `id` fires.
    fn fires_on(id: &str, pages: &[(&str, &str)]) -> Vec<String> {
        let pages: Vec<Page> = pages
            .iter()
            .enumerate()
            .map(|(i, (url, html))| Page {
                id: i + 1,
                url: url.to_string(),
                source: Some(if i == 0 { Source::Seed } else { Source::Link }),
                is_internal: true,
                status: 200,
                content_type: "text/html".into(),
                html: Some(Box::new(crate::parse::parse_html(
                    html,
                    &Url::parse(url).unwrap(),
                ))),
                ..Default::default()
            })
            .collect();
        let crawl = crate::model::Crawl {
            pages,
            ..Default::default()
        };
        let audit = crate::audit::run(&crawl);
        audit.hits[find(id).unwrap()]
            .iter()
            .map(|&i| crawl.pages[i].url.clone())
            .collect()
    }

    #[test]
    fn page_with_two_self_codes_is_consistent() {
        let alts = r#"<link rel="alternate" hreflang="en" href="https://e.com/en-US/"><link rel="alternate" hreflang="en-US" href="https://e.com/en-US/"><link rel="alternate" hreflang="de" href="https://e.com/de/">"#;
        let en = format!("<html lang=\"en-US\"><head>{alts}</head><body></body></html>");
        let de = r#"<html lang="de"><head><link rel="alternate" hreflang="en-US" href="https://e.com/en-US/"><link rel="alternate" hreflang="de" href="https://e.com/de/"></head><body></body></html>"#;
        let pages = [
            ("https://e.com/en-US/", en.as_str()),
            ("https://e.com/de/", de),
        ];
        assert!(fires_on("hreflang.inconsistent_return", &pages).is_empty());
        assert!(fires_on("hreflang.lang_mismatch", &pages).is_empty());
    }

    fn ld(json: &str) -> String {
        format!("<html><head><script type=\"application/ld+json\">{json}</script></head><body></body></html>")
    }

    #[test]
    fn json_ld_references_and_service_offers_are_not_errors() {
        let graph = r#"{"@context":"https://schema.org","@graph":[
            {"@type":"Organization","@id":"https://example.com/#org","name":"Co","url":"https://example.com/","logo":"https://example.com/l.png",
             "hasOfferCatalog":{"@type":"OfferCatalog","itemListElement":[{"@type":"Offer","itemOffered":{"@type":"Service","name":"Design"}}]}},
            {"@type":"WebPage","publisher":{"@type":"Organization","@id":"https://example.com/#org"},"isPartOf":{"@type":"WebSite","@id":"https://example.com/#site"}}]}"#;
        for id in [
            "sd.organization_name",
            "sd.organization_url",
            "sd.organization_logo",
            "sd.website_name",
            "sd.offer_price",
            "sd.offer_pricecurrency",
        ] {
            assert!(!fires(id, &ld(graph), &[]), "{id} fired");
        }
        let product = r#"{"@context":"https://schema.org","@type":"Product","name":"Van","offers":{"@type":"Offer","availability":"InStock"}}"#;
        assert!(fires("sd.offer_price", &ld(product), &[]));
        assert!(fires("sd.offer_pricecurrency", &ld(product), &[]));
    }

    #[test]
    fn theme_variants_of_one_image_share_alt() {
        let same = r#"<html><body><img src="/logo-light-96.webp" alt="Acme Logo"><img src="/logo-dark-96.webp" alt="Acme Logo"></body></html>"#;
        assert!(!fires("images.duplicate_alt", same, &[]));
        let diff = r#"<html><body><img src="/van-front.jpg" alt="Our van"><img src="/van-back.jpg" alt="Our van"></body></html>"#;
        assert!(fires("images.duplicate_alt", diff, &[]));
    }

    #[test]
    fn cookie_attributes_are_parsed_not_substring_matched() {
        let html = "<html><body></body></html>";
        assert!(fires(
            "security.cookie_not_secure",
            html,
            &[(
                "set-cookie",
                "insecure_pref=1; Path=/; HttpOnly; SameSite=Lax"
            )]
        ));
        assert!(!fires(
            "security.cookie_not_secure",
            html,
            &[("set-cookie", "a=1; Path=/; Secure; HttpOnly; SameSite=Lax")]
        ));
    }

    #[test]
    fn stopwords_are_not_keyword_stuffing() {
        assert!(!stuffed(
            "What changed, what Google says and what recovery costs"
        ));
        assert!(stuffed("Cheap vans, cheap van parts, cheap van builds"));
    }

    #[test]
    fn language_codes() {
        assert!(valid_hreflang("en-GB"));
        assert!(valid_hreflang("zh-Hant-TW"));
        assert!(valid_hreflang("x-default"));
        assert!(!valid_hreflang("en-UK"));
        assert!(!valid_hreflang("en_GB"));
        assert!(!valid_hreflang("english"));
        assert!(valid_html_lang("en-US"));
        assert!(!valid_html_lang("english"));
    }
}
