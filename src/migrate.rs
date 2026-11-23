//! Site migrations: propose a new URL for every indexable URL of the old site, export the
//! redirect map as CSV and as server configuration, and validate live redirects against a map.

use crate::audit::{self, FxMap, FxSet};
use crate::model::{Crawl, Page};
use crate::parse::fnv64;
use crate::report::csv_row;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Same URL on both crawls.
    Same,
    Path,
    Title,
    H1,
    Content,
    Slug,
    /// No match: the closest parent page is only a suggestion.
    Parent,
    Unmatched,
    /// Edited by hand in the app.
    Manual,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Same => "Same URL",
            Method::Path => "Path match",
            Method::Title => "Title match",
            Method::H1 => "H1 match",
            Method::Content => "Content match",
            Method::Slug => "Slug match",
            Method::Parent => "Unmatched",
            Method::Unmatched => "Unmatched",
            Method::Manual => "Manual",
        }
    }

    /// Rows that go into the server snippets. Unmatched rows (even with a parent suggestion)
    /// need a human decision first.
    pub fn redirects(self) -> bool {
        !matches!(self, Method::Parent | Method::Unmatched)
    }
}

#[derive(Clone, Debug)]
pub struct Mapping {
    pub old: String,
    /// Empty when nothing was found.
    pub new: String,
    /// 0-100.
    pub confidence: u8,
    pub method: Method,
    pub reason: String,
}

// ------------------------------------------------------------------ matching

/// Lower-case path without trailing slash, plus the query: "/Blog/Post/?a=1" -> "/blog/post?a=1".
pub fn path_key(u: &Url) -> String {
    let mut k = u.path().trim_end_matches('/').to_lowercase();
    if let Some(q) = u.query().filter(|q| !q.is_empty()) {
        k.push('?');
        k.push_str(q);
    }
    k
}

/// Title or H1 without a trailing " | Brand" / " - Brand" part, lower-cased, punctuation-free.
fn text_key(s: &str) -> String {
    let mut s = s.trim();
    for sep in [" | ", " - ", " – ", " — ", " · ", " :: ", " » "] {
        if let Some((head, _)) = s.rsplit_once(sep) {
            if head.trim().chars().count() >= 3 {
                s = head;
            }
        }
    }
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).and_then(|&c| hex(c)),
            b.get(i + 2).and_then(|&c| hex(c)),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Words of the last path segment: "/blog/Green-Tree_Frogs.html" -> ["frogs", "green", "tree"].
fn slug_tokens(u: &Url) -> Vec<String> {
    let seg = u
        .path_segments()
        .and_then(|mut s| s.rfind(|s| !s.is_empty()))
        .unwrap_or("");
    words(seg)
}

/// Words of the whole path, to break ties between equal slugs in different sections.
fn path_tokens(u: &Url) -> Vec<String> {
    words(u.path())
}

fn words(seg: &str) -> Vec<String> {
    let seg = decode(seg).to_lowercase();
    let seg = match seg.rsplit_once('.') {
        Some((stem, "html" | "htm" | "php" | "asp" | "aspx" | "jsp" | "cfm" | "shtml")) => {
            stem.to_string()
        }
        _ => seg,
    };
    let mut t: Vec<String> = seg
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    t.sort();
    t.dedup();
    t
}

/// Dice coefficient of two sorted, de-duplicated token lists.
fn dice(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.iter().filter(|t| b.binary_search(t).is_ok()).count();
    2.0 * common as f32 / (a.len() + b.len()) as f32
}

/// Hashes of the distinct words (3+ letters) of a page's visible text.
fn word_set(p: &Page) -> FxSet<u64> {
    p.html
        .as_deref()
        .map(|h| {
            h.text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.chars().count() >= 3)
                .map(|w| fnv64(w.to_lowercase().as_bytes()))
                .collect()
        })
        .unwrap_or_default()
}

fn jaccard(a: &FxSet<u64>, b: &FxSet<u64>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.iter().filter(|w| b.contains(w)).count();
    common as f32 / (a.len() + b.len() - common) as f32
}

/// Index over the new site's indexable pages.
struct Target<'a> {
    pages: Vec<(&'a Page, Url)>,
    by_path: FxMap<String, usize>,
    by_title: FxMap<String, Vec<usize>>,
    by_h1: FxMap<String, Vec<usize>>,
    by_lead: FxMap<u64, Vec<usize>>,
    /// Simhash split in 8 bands of 8 bits: two pages within distance 7 share a band.
    bands: Vec<FxMap<u8, Vec<usize>>>,
    slugs: Vec<Vec<String>>,
    by_token: FxMap<String, Vec<usize>>,
}

/// More candidates than this for one title, H1 or slug word: not distinctive.
const MAX_CANDIDATES: usize = 20;
/// Simhash bucket members scanned per band (template-heavy sites put many pages in one bucket).
const MAX_BUCKET: usize = 2000;

fn lead_key(p: &Page) -> Option<u64> {
    let lead = p.html.as_deref()?.lead.trim();
    (lead.chars().count() >= 60).then(|| fnv64(text_key(lead).as_bytes()))
}

impl<'a> Target<'a> {
    fn new(crawl: &'a Crawl) -> Target<'a> {
        let pages: Vec<(&Page, Url)> = crawl
            .pages
            .iter()
            .filter(|p| audit::indexable(p))
            .filter_map(|p| Url::parse(&p.url).ok().map(|u| (p, u)))
            .collect();
        let mut t = Target {
            by_path: FxMap::default(),
            by_title: FxMap::default(),
            by_h1: FxMap::default(),
            by_lead: FxMap::default(),
            bands: vec![FxMap::default(); 8],
            slugs: Vec::with_capacity(pages.len()),
            by_token: FxMap::default(),
            pages: Vec::new(),
        };
        for (i, (p, u)) in pages.iter().enumerate() {
            t.by_path.entry(path_key(u)).or_insert(i);
            for (map, s) in [(&mut t.by_title, p.title()), (&mut t.by_h1, p.h1())] {
                let k = text_key(s);
                if !k.is_empty() {
                    map.entry(k).or_default().push(i);
                }
            }
            if let Some(k) = lead_key(p) {
                t.by_lead.entry(k).or_default().push(i);
            }
            if p.word_count() >= 20 {
                let sh = p.html.as_deref().map(|h| h.simhash).unwrap_or(0);
                for (b, band) in t.bands.iter_mut().enumerate() {
                    band.entry((sh >> (b * 8)) as u8).or_default().push(i);
                }
            }
            let slug = slug_tokens(u);
            for tok in &slug {
                t.by_token.entry(tok.clone()).or_default().push(i);
            }
            t.slugs.push(slug);
        }
        t.pages = pages;
        t
    }

    /// Best of several candidates by slug similarity, ties broken by whole-path similarity.
    fn best_of(&self, cands: &[usize], slug: &[String], path: &[String]) -> (usize, (f32, f32)) {
        let score = |i: usize| {
            let s = dice(slug, &self.slugs[i]);
            (s, s + 0.01 * dice(path, &path_tokens(&self.pages[i].1)))
        };
        cands
            .iter()
            .map(|&i| (i, score(i)))
            .fold((cands[0], (-1.0, -1.0)), |best, c| {
                if c.1 .1 > best.1 .1 {
                    c
                } else {
                    best
                }
            })
    }

    fn propose(&self, old: &Page) -> Mapping {
        let mut m = Mapping {
            old: old.url.clone(),
            new: String::new(),
            confidence: 0,
            method: Method::Unmatched,
            reason: "No similar page on the new site".into(),
        };
        let Ok(u) = Url::parse(&old.url) else {
            return m;
        };
        let hit = |m: &mut Mapping, i: usize, method: Method, confidence: f32, reason: String| {
            m.new = self.pages[i].0.url.clone();
            m.method = method;
            m.confidence = confidence.round().clamp(0.0, 100.0) as u8;
            m.reason = format!("{}: {reason}", method.label());
        };
        let key = path_key(&u);
        if let Some(&i) = self.by_path.get(&key) {
            if self.pages[i].0.url == old.url {
                hit(
                    &mut m,
                    i,
                    Method::Same,
                    100.0,
                    "the URL still exists".into(),
                );
            } else {
                hit(&mut m, i, Method::Path, 100.0, "same path".into());
            }
            return m;
        }
        let (slug, path) = (slug_tokens(&u), path_tokens(&u));
        for (method, what, map, text, sure, base) in [
            (
                Method::Title,
                "title",
                &self.by_title,
                old.title(),
                90.0,
                70.0,
            ),
            (Method::H1, "H1", &self.by_h1, old.h1(), 85.0, 65.0),
        ] {
            let k = text_key(text);
            match map.get(&k).map(|v| &v[..]) {
                Some([i]) => {
                    hit(&mut m, *i, method, sure, format!("same {what} \"{k}\""));
                    return m;
                }
                Some(c) if c.len() <= MAX_CANDIDATES => {
                    let (i, (s, _)) = self.best_of(c, &slug, &path);
                    hit(
                        &mut m,
                        i,
                        method,
                        base + 10.0 * s,
                        format!(
                            "same {what} \"{k}\" on {} pages, closest URL picked",
                            c.len()
                        ),
                    );
                    return m;
                }
                _ => {}
            }
        }

        // Content: same lead paragraph, or a close simhash, confirmed by word overlap.
        let mut cands: Vec<(usize, u32)> = Vec::new();
        let sh = old.html.as_deref().map(|h| h.simhash).unwrap_or(0);
        let dist = |i: usize| {
            (self.pages[i]
                .0
                .html
                .as_deref()
                .map(|h| h.simhash)
                .unwrap_or(0)
                ^ sh)
                .count_ones()
        };
        if let Some(c) = lead_key(old).and_then(|k| self.by_lead.get(&k)) {
            cands.extend(c.iter().take(MAX_CANDIDATES).map(|&i| (i, dist(i))));
        }
        if old.word_count() >= 20 {
            for (b, band) in self.bands.iter().enumerate() {
                if let Some(members) = band.get(&((sh >> (b * 8)) as u8)) {
                    cands.extend(
                        members
                            .iter()
                            .take(MAX_BUCKET)
                            .map(|&i| (i, dist(i)))
                            .filter(|&(_, d)| d <= 12),
                    );
                }
            }
        }
        cands.sort_unstable_by_key(|&(i, d)| (d, i));
        cands.dedup_by_key(|c| c.0);
        if !cands.is_empty() {
            let words = word_set(old);
            let lead = lead_key(old);
            let best = cands
                .iter()
                .take(5)
                .map(|&(i, d)| {
                    let p = self.pages[i].0;
                    let overlap = jaccard(&words, &word_set(p));
                    let same_lead = lead.is_some() && lead == lead_key(p);
                    let sim = (1.0 - d as f32 / 32.0).max(0.0);
                    let score = 0.5 * sim + 0.5 * overlap + if same_lead { 0.25 } else { 0.0 };
                    (i, d, overlap, same_lead, score.min(1.0))
                })
                .fold(None, |b: Option<(usize, u32, f32, bool, f32)>, c| match b {
                    Some(b) if b.4 >= c.4 => Some(b),
                    _ => Some(c),
                });
            if let Some((i, d, overlap, same_lead, score)) = best.filter(|b| b.4 >= 0.6) {
                hit(
                    &mut m,
                    i,
                    Method::Content,
                    40.0 + 40.0 * score,
                    format!(
                        "similar content (simhash distance {d}, {:.0}% word overlap{})",
                        overlap * 100.0,
                        if same_lead {
                            ", same first paragraph"
                        } else {
                            ""
                        }
                    ),
                );
                return m;
            }
        }

        // Slug: candidates share a not-too-common slug word.
        let mut seen = FxSet::default();
        let mut cands = Vec::new();
        for tok in &slug {
            if let Some(c) = self.by_token.get(tok).filter(|c| c.len() <= 500) {
                cands.extend(c.iter().copied().filter(|&i| seen.insert(i)));
            }
        }
        let best = (!cands.is_empty()).then(|| self.best_of(&cands, &slug, &path));
        // A partial slug match needs 2+ shared words or the same parent folder:
        // "/plugins/classic-editor" must not map to "/docs/block-editor" on "editor" alone.
        let parent = |x: &Url| {
            let p = x.path().trim_end_matches('/');
            p[..p.rfind('/').unwrap_or(0)].to_string()
        };
        let plausible = |&(i, (s, _)): &(usize, (f32, f32))| {
            let shared = slug
                .iter()
                .filter(|t| self.slugs[i].binary_search(t).is_ok())
                .count();
            s >= 1.0 || (s >= 0.5 && (shared >= 2 || parent(&u) == parent(&self.pages[i].1)))
        };
        if let Some((i, (s, _))) = best.filter(plausible) {
            hit(
                &mut m,
                i,
                Method::Slug,
                30.0 + 40.0 * s,
                format!("similar URL words ({:.0}% in common)", s * 100.0),
            );
            return m;
        }

        // Nothing: suggest the closest existing parent ("/blog/2020/post" -> "/blog").
        let mut parent = key.split('?').next().unwrap_or("").to_string();
        while let Some(cut) = parent.rfind('/') {
            parent.truncate(cut);
            if let Some(&i) = self.by_path.get(&parent) {
                let shown = if parent.is_empty() { "/" } else { &parent };
                hit(
                    &mut m,
                    i,
                    Method::Parent,
                    10.0,
                    format!("no similar page; closest parent is {shown}"),
                );
                break;
            }
        }
        m
    }
}

/// Propose a new URL for every indexable page of `old`, in crawl order.
pub fn propose(old: &Crawl, new: &Crawl) -> Vec<Mapping> {
    let target = Target::new(new);
    let olds: Vec<&Page> = old.pages.iter().filter(|p| audit::indexable(p)).collect();
    if target.pages.is_empty() {
        return olds
            .iter()
            .map(|p| Mapping {
                old: p.url.clone(),
                new: String::new(),
                confidence: 0,
                method: Method::Unmatched,
                reason: "Unmatched: the new crawl has no indexable pages".into(),
            })
            .collect();
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let chunk = olds.len().div_ceil(threads).max(256);
    std::thread::scope(|s| {
        let parts: Vec<_> = olds
            .chunks(chunk)
            .map(|c| {
                let t = &target;
                s.spawn(move || c.iter().map(|p| t.propose(p)).collect::<Vec<_>>())
            })
            .collect();
        parts
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

// ------------------------------------------------------------------ exports

pub fn map_csv(maps: &[Mapping]) -> String {
    let mut out = csv_row(&["old", "new", "confidence", "reason"]);
    for m in maps {
        out.push_str(&csv_row(&[
            &m.old,
            &m.new,
            &m.confidence.to_string(),
            &m.reason,
        ]));
    }
    out
}

/// A redirect that can go into server configuration.
struct Rule {
    old: Url,
    new: String,
}

fn rules(maps: &[Mapping]) -> Vec<Rule> {
    let mut seen = FxSet::default();
    let mut out: Vec<Rule> = maps
        .iter()
        .filter(|m| m.method.redirects() && !m.new.is_empty() && m.old != m.new)
        .filter_map(|m| {
            let old = Url::parse(&m.old).ok()?;
            let new = Url::parse(&m.new).ok()?.to_string();
            let key = (old.path().to_string(), old.query().map(str::to_string));
            seen.insert(key).then_some(Rule { old, new })
        })
        .collect();
    // query-specific rules first: they must win over the plain path rule
    out.sort_by_key(|r| r.old.query().is_none());
    out
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Apache (.htaccess, mod_rewrite). Patterns match the decoded path, regex-escaped; the
/// targets escape `$` and `%`, which mod_rewrite would read as back-references.
pub fn htaccess(maps: &[Mapping]) -> String {
    let mut out = String::from(
        "# Redirects generated by ShoutingToad. Put these near the top of the old site's .htaccess.\n# Old URLs with a query string only match that exact query; other URLs keep their query string.\nRewriteEngine On\n\n",
    );
    for r in rules(maps) {
        let path = decode(r.old.path());
        if has_control(&path) || has_control(&r.new) {
            let _ = writeln!(out, "# skipped (control characters): {}", r.old);
            continue;
        }
        let pattern = format!(
            "^{}/?$",
            regex::escape(path.trim_start_matches('/').trim_end_matches('/'))
        );
        let target = r
            .new
            .replace('\\', "\\\\")
            .replace('$', "\\$")
            .replace('%', "\\%");
        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
        let mut flags = "R=301,L,NE";
        if let Some(q) = r.old.query() {
            let _ = writeln!(
                out,
                "RewriteCond %{{QUERY_STRING}} {}",
                quote(&format!("^{}$", regex::escape(q)))
            );
            flags = "R=301,L,NE,QSD";
        }
        let _ = writeln!(
            out,
            "RewriteRule {} {} [{flags}]",
            quote(&pattern),
            quote(&target)
        );
    }
    out
}

/// Nginx: two maps (decoded path, and raw path + query) and the `if` that uses them.
pub fn nginx(maps: &[Mapping]) -> String {
    // nginx expands $variables in map values: percent-encode `$` in targets (same URL).
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let (mut by_path, mut by_uri) = (String::new(), String::new());
    let mut keys = FxSet::default();
    for r in rules(maps) {
        let target = quote(&r.new.replace('$', "%24"));
        if let Some(q) = r.old.query() {
            let raw = format!("{}?{q}", r.old.path());
            if keys.insert(raw.clone()) {
                let _ = writeln!(by_uri, "    {} {target};", quote(&raw));
            }
            continue;
        }
        let path = decode(r.old.path());
        if has_control(&path) {
            let _ = writeln!(by_path, "    # skipped (control characters): {}", r.old);
            continue;
        }
        // with and without the trailing slash, unless another rule owns the other form
        let trimmed = path.trim_end_matches('/');
        let mut forms = vec![path.clone()];
        if !trimmed.is_empty() {
            forms.push(if path.ends_with('/') {
                trimmed.to_string()
            } else {
                format!("{path}/")
            });
        }
        for f in forms {
            if keys.insert(f.clone()) {
                let _ = writeln!(by_path, "    {} {target};", quote(&f));
            }
        }
    }
    format!(
        "# Redirects generated by ShoutingToad.\n# 1. In the http {{}} block:\nmap $uri $st_redirect {{\n    default \"\";\n{by_path}}}\nmap $request_uri $st_redirect_query {{\n    default \"\";\n{by_uri}}}\n\n# 2. In the old site's server {{}} block:\n#    if ($st_redirect_query) {{ return 301 $st_redirect_query; }}\n#    if ($st_redirect) {{ return 301 $st_redirect$is_args$args; }}\n"
    )
}

/// Netlify `_redirects`. Sources stay percent-encoded (whitespace separates the fields);
/// `:` and `*` are encoded too, since Netlify reads them as placeholders and splats.
pub fn netlify(maps: &[Mapping]) -> String {
    let mut out = String::from("# Redirects generated by ShoutingToad (Netlify _redirects).\n");
    for r in rules(maps) {
        let src = r.old.path().replace(':', "%3A").replace('*', "%2A");
        if r.old.query().is_some() {
            let _ = writeln!(
                out,
                "# not expressible (fixed query string): {} -> {}",
                r.old, r.new
            );
            continue;
        }
        let _ = writeln!(out, "{src}  {}  301", r.new.replace(' ', "%20"));
    }
    out
}

/// Cloudflare bulk redirects CSV (no header): source without scheme so http and https
/// both match. Cloudflare cannot match a query string, so such rules are left out.
pub fn cloudflare(maps: &[Mapping]) -> String {
    let mut out = String::new();
    for r in rules(maps).into_iter().filter(|r| r.old.query().is_none()) {
        let src = format!("{}{}", r.old.host_str().unwrap_or(""), r.old.path());
        let src = match r.old.port() {
            Some(p) => src.replacen('/', &format!(":{p}/"), 1),
            None => src,
        };
        out.push_str(&csv_row(&[
            &src, &r.new, "301", "TRUE", "FALSE", "FALSE", "FALSE",
        ]));
    }
    out
}

/// Write redirect_map.csv and the four server snippets.
pub fn write_all(dir: &Path, maps: &[Mapping]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    for (name, data) in [
        ("redirect_map.csv", map_csv(maps)),
        ("redirects.htaccess", htaccess(maps)),
        ("redirects.nginx.conf", nginx(maps)),
        ("_redirects", netlify(maps)),
        ("cloudflare_bulk_redirects.csv", cloudflare(maps)),
    ] {
        let p = dir.join(name);
        std::fs::write(&p, data).map_err(|e| format!("could not write {}: {e}", p.display()))?;
    }
    Ok(())
}

// ------------------------------------------------------------------ validation

/// Parse CSV text (quoted fields, `""` escapes; one record per line).
fn csv_records(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(|line| {
            let (mut cells, mut cur, mut quoted) = (Vec::new(), String::new(), false);
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                match (c, quoted) {
                    ('"', true) if chars.peek() == Some(&'"') => {
                        cur.push('"');
                        chars.next();
                    }
                    ('"', _) => quoted = !quoted,
                    (',', false) => cells.push(std::mem::take(&mut cur)),
                    _ => cur.push(c),
                }
            }
            cells.push(cur);
            cells
        })
        .collect()
}

/// (old, new) pairs from a redirect map CSV. A header row and `#` comments are skipped.
pub fn read_map(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (n, rec) in csv_records(text.trim_start_matches('\u{feff}'))
        .into_iter()
        .enumerate()
    {
        let old = rec[0].trim();
        if old.is_empty() || old.starts_with('#') || (n == 0 && !old.contains("://")) {
            continue;
        }
        let new = rec.get(1).map(|s| s.trim()).unwrap_or("");
        if Url::parse(old).is_err() || Url::parse(new).is_err() {
            return Err(format!(
                "{} line {}: expected 'old URL,new URL', got '{}'",
                path.display(),
                n + 1,
                rec.join(",")
            ));
        }
        out.push((old.to_string(), new.to_string()));
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct Validation {
    pub old: String,
    pub expected: String,
    /// Status of the old URL (0: no response or not crawled).
    pub status: u16,
    pub hops: usize,
    pub final_url: String,
    pub final_status: Option<u16>,
    /// Empty when the redirect is right.
    pub problems: Vec<String>,
}

fn same_url(a: &str, b: &str) -> bool {
    let n = |s: &str| {
        Url::parse(s)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| s.to_string())
    };
    n(a) == n(b)
}

/// Check that every old URL answers with one permanent redirect straight to its mapped,
/// working target.
pub fn validate(map: &[(String, String)], crawl: &Crawl) -> Vec<Validation> {
    let ctx = audit::Ctx::urls_only(crawl);
    let lookup = |u: &str| {
        ctx.page(u)
            .or_else(|| Url::parse(u).ok().and_then(|x| ctx.page(x.as_str())))
    };
    map.iter()
        .map(|(old, expected)| {
            let mut v = Validation {
                old: old.clone(),
                expected: expected.clone(),
                status: 0,
                hops: 0,
                final_url: String::new(),
                final_status: None,
                problems: Vec::new(),
            };
            let Some(first) = lookup(old) else {
                v.problems.push("old URL not in the crawl".into());
                return v;
            };
            v.status = first.status;
            let mut cur = first;
            let mut seen = FxSet::default();
            let mut temporary = false;
            loop {
                if !seen.insert(cur.url.as_str()) {
                    v.problems.push("redirect loop".into());
                    break;
                }
                if !cur.is_redirect() {
                    v.final_url = cur.url.clone();
                    v.final_status = Some(cur.status);
                    break;
                }
                temporary |= !matches!(cur.status, 301 | 308);
                v.hops += 1;
                match cur.redirect_to.as_deref().and_then(lookup) {
                    Some(next) if v.hops <= 20 => cur = next,
                    _ => {
                        v.final_url = cur.redirect_to.clone().unwrap_or_default();
                        v.problems.push("redirect target not crawled".into());
                        break;
                    }
                }
            }
            let p = &mut v.problems;
            if first.blocked_by_robots {
                p.push("blocked by robots.txt (validate with --ignore-robots)".into());
            } else if first.status == 0 {
                p.push(format!(
                    "no response ({})",
                    first.error.as_deref().unwrap_or("connection failed")
                ));
            } else if v.hops == 0 && !same_url(old, expected) {
                p.push(if first.status == 200 {
                    "not redirected (200)".to_string()
                } else {
                    format!("not redirected ({})", first.status)
                });
            }
            if temporary {
                p.push(
                    if (300..400).contains(&first.status) && !matches!(first.status, 301 | 308) {
                        format!("temporary redirect ({})", first.status)
                    } else {
                        "temporary redirect in the chain".to_string()
                    },
                );
            }
            if v.hops > 1 {
                p.push(format!("redirect chain ({} hops)", v.hops));
            }
            if v.hops > 0 && !v.final_url.is_empty() && !same_url(&v.final_url, expected) {
                p.push("redirects to a different URL than mapped".into());
            }
            match v.final_status {
                Some(s) if v.hops > 0 && s != 200 => p.push(format!("target returns {s}")),
                _ => {}
            }
            v
        })
        .collect()
}

pub fn validation_csv(rows: &[Validation]) -> String {
    let mut out = csv_row(&[
        "Old URL",
        "Mapped Target",
        "Status",
        "Hops",
        "Final URL",
        "Final Status",
        "Result",
    ]);
    for v in rows {
        out.push_str(&csv_row(&[
            &v.old,
            &v.expected,
            &v.status.to_string(),
            &v.hops.to_string(),
            &v.final_url,
            &v.final_status.map(|s| s.to_string()).unwrap_or_default(),
            &if v.problems.is_empty() {
                "OK".to_string()
            } else {
                v.problems.join("; ")
            },
        ]));
    }
    out
}

// ------------------------------------------------------------------ CLI

/// `shouting-toad migrate OLD.shoutingtoad NEW.shoutingtoad --out DIR`
pub fn cli(args: &[String]) -> Result<i32, String> {
    let mut out = PathBuf::from("shoutingtoad-migration");
    let mut files = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().ok_or("--out needs a value")?.into(),
            s if s.starts_with("--") => return Err(format!("unknown option '{s}'")),
            s => files.push(PathBuf::from(s)),
        }
    }
    let [old, new] = &files[..] else {
        return Err("migrate needs two crawl files: OLD.shoutingtoad NEW.shoutingtoad".into());
    };
    let t = std::time::Instant::now();
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| Crawl::load(old));
        let b = Crawl::load(new);
        (a.join().unwrap_or_else(|_| Err("loader crashed".into())), b)
    });
    let (a, b) = (a?, b?);
    let t_map = std::time::Instant::now();
    let maps = propose(&a, &b);
    let map_secs = t_map.elapsed().as_secs_f32();
    write_all(&out, &maps)?;
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for m in &maps {
        let l = m.method.label();
        match counts.iter_mut().find(|(k, _)| *k == l) {
            Some(c) => c.1 += 1,
            None => counts.push((l, 1)),
        }
    }
    println!(
        "Mapped {} indexable old URLs to {} in {map_secs:.2}s ({:.1}s with loading)",
        maps.len(),
        b.start_url,
        t.elapsed().as_secs_f32()
    );
    for (l, n) in counts {
        println!("  {l:<14} {n}");
    }
    println!(
        "Wrote redirect_map.csv, redirects.htaccess, redirects.nginx.conf, _redirects and cloudflare_bulk_redirects.csv to {}",
        out.display()
    );
    std::mem::forget((a, b));
    Ok(0)
}

/// `shouting-toad validate-redirects MAP.csv --out DIR [--crawl FILE | crawl options]`:
/// without `--crawl`, the old URLs are crawled in list mode first.
pub fn validate_cli(args: &[String]) -> Result<i32, String> {
    let mut out = PathBuf::from("shoutingtoad-redirects");
    let (mut map_file, mut saved) = (None, None);
    let mut crawl_args = vec!["list".to_string()];
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().ok_or("--out needs a value")?.into(),
            "--crawl" => saved = Some(PathBuf::from(it.next().ok_or("--crawl needs a value")?)),
            s if s.starts_with("--") => crawl_args.push(s.to_string()),
            s if map_file.is_none() && !crawl_args.last().is_some_and(|l| takes_value(l)) => {
                map_file = Some(PathBuf::from(s))
            }
            s => crawl_args.push(s.to_string()),
        }
    }
    let map_file = map_file.ok_or("validate-redirects needs a redirect map CSV (old,new)")?;
    let map = read_map(&map_file)?;
    if map.is_empty() {
        return Err(format!("{} has no redirects", map_file.display()));
    }
    std::fs::create_dir_all(&out)
        .map_err(|e| format!("could not create {}: {e}", out.display()))?;
    let crawl_file = match saved {
        Some(f) => f,
        None => {
            let list = out.join("old_urls.txt");
            let urls: String = map.iter().map(|(o, _)| format!("{o}\n")).collect();
            std::fs::write(&list, urls)
                .map_err(|e| format!("could not write {}: {e}", list.display()))?;
            crawl_args.insert(1, list.display().to_string());
            crawl_args.extend(["--out".to_string(), out.display().to_string()]);
            crate::cli::crawl_cmd(&crawl_args)?;
            out.join("crawl.shoutingtoad")
        }
    };
    let crawl = Crawl::load(&crawl_file)?;
    let rows = validate(&map, &crawl);
    let p = out.join("redirect_validation.csv");
    std::fs::write(&p, validation_csv(&rows))
        .map_err(|e| format!("could not write {}: {e}", p.display()))?;
    let bad: Vec<&Validation> = rows.iter().filter(|v| !v.problems.is_empty()).collect();
    println!(
        "\nRedirect map: {} OK, {} with problems (details in {})",
        rows.len() - bad.len(),
        bad.len(),
        p.display()
    );
    for v in bad.iter().take(20) {
        println!("  {} -> {}: {}", v.old, v.expected, v.problems.join("; "));
    }
    if bad.len() > 20 {
        println!("  …and {} more", bad.len() - 20);
    }
    Ok(if bad.is_empty() { 0 } else { 2 })
}

/// Crawl options followed by a value (so the value is not taken for the map file).
fn takes_value(opt: &str) -> bool {
    matches!(
        opt,
        "--max-urls"
            | "--max-depth"
            | "--threads"
            | "--delay"
            | "--timeout"
            | "--user-agent"
            | "--include"
            | "--exclude"
            | "--fail-on"
            | "--fail-on-check"
            | "--baseline"
            | "--fail-on-new"
            | "--summary-md"
            | "--badge"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HtmlData;

    fn page(url: &str, title: &str, h1: &str, text: &str) -> Page {
        let words: Vec<String> = text.split_whitespace().map(|w| w.to_lowercase()).collect();
        Page {
            url: url.into(),
            is_internal: true,
            status: 200,
            content_type: "text/html".into(),
            html: Some(Box::new(HtmlData {
                titles: vec![title.into()],
                h1: vec![h1.into()],
                word_count: words.len(),
                simhash: crate::parse::simhash(&words),
                text: text.into(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn crawl(pages: Vec<Page>) -> Crawl {
        Crawl {
            pages,
            ..Default::default()
        }
    }

    fn filler(seed: &str) -> String {
        (0..120).map(|i| format!("{seed}{} ", i * 7 % 23)).collect()
    }

    #[test]
    fn match_order_path_title_h1_content_slug_parent() {
        let story = "the green toad sat on a lily pad and sang about rain all night long while the moon rose over the pond and the reeds swayed gently in the warm wind ".repeat(4);
        let old = crawl(vec![
            page("https://old.test/About/", "About us | Old Co", "About", "a"),
            page("https://old.test/team", "Our Team | Old Co", "Team", "b"),
            page("https://old.test/x1", "Old x1", "Pond care guide", "c"),
            page("https://old.test/p?id=7", "Story", "Story", &story),
            page(
                "https://old.test/blog/green-tree-frogs.html",
                "GTF",
                "GTF",
                &filler("q"),
            ),
            page(
                "https://old.test/blog/2019/gone",
                "Gone",
                "Gone",
                &filler("z"),
            ),
        ]);
        let new = crawl(vec![
            page("https://new.test/", "Home - New Co", "Home", "h"),
            page("https://new.test/about", "Who we are", "About", "a"),
            page(
                "https://new.test/people",
                "Our team - New Co",
                "People",
                "b",
            ),
            page(
                "https://new.test/guides/pond",
                "Pond guide",
                "Pond Care Guide!",
                "c",
            ),
            page(
                "https://new.test/stories/toad",
                "A toad story",
                "Toad",
                &story,
            ),
            page("https://new.test/blog", "Blog", "Blog", &filler("w")),
            page(
                "https://new.test/frogs/green-tree-frogs",
                "Frogs",
                "Frogs",
                &filler("k"),
            ),
        ]);
        let m = propose(&old, &new);
        let got: Vec<(Method, &str)> = m.iter().map(|m| (m.method, m.new.as_str())).collect();
        assert_eq!(
            got,
            [
                (Method::Path, "https://new.test/about"),
                (Method::Title, "https://new.test/people"),
                (Method::H1, "https://new.test/guides/pond"),
                (Method::Content, "https://new.test/stories/toad"),
                (Method::Slug, "https://new.test/frogs/green-tree-frogs"),
                (Method::Parent, "https://new.test/blog"),
            ],
            "{m:#?}"
        );
        assert!(m[0].confidence == 100 && m[5].confidence < 20);
        assert!(m[3].confidence > m[4].confidence && m[4].confidence > m[5].confidence);
        assert!(m[5].reason.contains("/blog"));
        // the parent suggestion is not turned into a redirect
        assert!(!htaccess(&m).contains("gone"));
    }

    fn mapping(old: &str, new: &str) -> Mapping {
        Mapping {
            old: old.into(),
            new: new.into(),
            confidence: 100,
            method: Method::Manual,
            reason: String::new(),
        }
    }

    #[test]
    fn snippets_escape_paths_targets_and_queries() {
        let maps = vec![
            mapping("https://o.test/a b/(x)+y.html", "https://n.test/new$1%41"),
            mapping("https://o.test/list?id=5&s=a+b", "https://n.test/item/5"),
            mapping("https://o.test/list", "https://n.test/items"),
            mapping("https://o.test/caf%C3%A9/", "https://n.test/cafe"),
            mapping("https://o.test/q\"uote", "https://n.test/"),
            mapping("https://o.test/same", "https://o.test/same"),
            mapping("https://o.test/a:b*", "https://n.test/ab"),
        ];
        let h = htaccess(&maps);
        assert!(
            h.contains(
                r#"RewriteRule "^a b/\(x\)\+y\.html/?$" "https://n.test/new\$1\%41" [R=301,L,NE]"#
            ),
            "{h}"
        );
        let q = h
            .find("RewriteCond %{QUERY_STRING} \"^id=5\\&s=a\\+b$\"")
            .expect("query cond");
        assert!(
            q < h.find("\"^list/?$\"").unwrap(),
            "query rule first:\n{h}"
        );
        assert!(h.contains("\"https://n.test/item/5\" [R=301,L,NE,QSD]"));
        assert!(h.contains("\"^café/?$\""));
        assert!(
            h.contains(r#""^q\"uote/?$""#) || h.contains("q%22uote"),
            "{h}"
        );
        assert!(!h.contains("same"));

        let n = nginx(&maps);
        assert!(
            n.contains("\"/a b/(x)+y.html\" \"https://n.test/new%241%41\";"),
            "{n}"
        );
        assert!(n.contains("\"/a b/(x)+y.html/\""));
        assert!(n.contains("\"/list?id=5&s=a+b\" \"https://n.test/item/5\";"));
        assert!(n.contains("\"/café/\" \"https://n.test/cafe\";") && n.contains("\"/café\" "));
        assert!(n.contains("return 301 $st_redirect$is_args$args;"));

        let nf = netlify(&maps);
        assert!(
            nf.contains("/a%20b/(x)+y.html  https://n.test/new$1%41  301"),
            "{nf}"
        );
        assert!(nf.contains("# not expressible (fixed query string): https://o.test/list?id=5"));
        assert!(nf.contains("/a%3Ab%2A  https://n.test/ab  301"));

        let cf = cloudflare(&maps);
        assert!(
            cf.contains("o.test/list,https://n.test/items,301,TRUE,FALSE,FALSE,FALSE\n"),
            "{cf}"
        );
        assert!(!cf.contains("id=5"));
        assert_eq!(cf.lines().count(), 5);
    }

    #[test]
    fn map_csv_round_trip() {
        let maps = vec![Mapping {
            reason: "Title match: same title \"a, b\"".into(),
            ..mapping("https://o.test/x,y", "https://n.test/")
        }];
        let dir = std::env::temp_dir().join(format!("st-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("map.csv");
        std::fs::write(&f, map_csv(&maps)).unwrap();
        assert_eq!(
            read_map(&f).unwrap(),
            [(
                "https://o.test/x,y".to_string(),
                "https://n.test/".to_string()
            )]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keys() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert_eq!(
            path_key(&u("https://a.test/Blog/Post/?a=1")),
            "/blog/post?a=1"
        );
        assert_eq!(path_key(&u("http://b.test/")), "");
        assert_eq!(text_key("Green Frogs | Toad Shop"), "green frogs");
        assert_eq!(text_key("A - B - Brand"), "a b");
        assert_eq!(
            slug_tokens(&u("https://a.test/x/Green-Tree_frogs.html")),
            ["frogs", "green", "tree"]
        );
    }
}
