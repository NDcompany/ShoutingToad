//! The audit engine: builds cross-page context and runs every check.

use crate::checks::{catalogue, Check};
use crate::model::{Crawl, HtmlData, Page, Source};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

/// FxHash (as used in rustc): much faster than SipHash for the URL lookups the link checks
/// do millions of times. Keys come from the crawl itself, so HashDoS resistance is not needed.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some((c, r)) = rest.split_first_chunk::<8>() {
            self.add(u64::from_le_bytes(*c));
            rest = r;
        }
        for &b in rest {
            self.add(b as u64);
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

impl FxHasher {
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

pub type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxSet<K> = HashSet<K, BuildHasherDefault<FxHasher>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    Issue,
    Warning,
    Opportunity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    High,
    Medium,
    Low,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Issue => "Issue",
            Kind::Warning => "Warning",
            Kind::Opportunity => "Opportunity",
        }
    }
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
        }
    }
}

/// Internal HTML page that returned 200.
pub fn ih(p: &Page) -> bool {
    p.is_internal && p.status == 200 && p.html.is_some()
}

/// HTML data of an internal 200 HTML page.
pub fn hd(p: &Page) -> Option<&HtmlData> {
    if ih(p) {
        p.html.as_deref()
    } else {
        None
    }
}

pub fn self_canonical(p: &Page) -> bool {
    let c = p.canonical();
    c.is_empty() || c == p.url
}

pub fn indexable(p: &Page) -> bool {
    ih(p) && !p.is_noindex() && self_canonical(p) && !p.blocked_by_robots
}

/// HTML data of an indexable page.
pub fn ihd(p: &Page) -> Option<&HtmlData> {
    if indexable(p) {
        p.html.as_deref()
    } else {
        None
    }
}

pub fn indexability_label(p: &Page) -> &'static str {
    if p.blocked_by_robots {
        "Blocked by robots.txt"
    } else if p.status == 0 {
        "No Response"
    } else if p.is_redirect() {
        "Redirected"
    } else if (400..500).contains(&p.status) {
        "Client Error"
    } else if p.status >= 500 {
        "Server Error"
    } else if p.html.is_none() {
        "Non-HTML"
    } else if p.is_noindex() {
        "Noindex"
    } else if !self_canonical(p) {
        "Canonicalised"
    } else if !p.is_internal {
        "External"
    } else {
        "Indexable"
    }
}

/// Approximate rendered width (Arial) of `s` at `font_px`.
pub fn pixel_width(s: &str, font_px: f32) -> usize {
    let em: f32 = s
        .chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | '\'' | '|' | '!' | '.' | ',' | ':' | ';' => 0.24,
            'f' | 't' | 'r' | 'I' | ' ' | '(' | ')' | '[' | ']' | '/' | '-' => 0.33,
            'm' | 'w' => 0.83,
            'M' | 'W' => 0.9,
            '0'..='9' => 0.556,
            c if c.is_ascii_uppercase() => 0.68,
            c if c.is_ascii_lowercase() => 0.53,
            c if c.is_ascii() => 0.5,
            _ => 0.9,
        })
        .sum();
    (em * font_px).round() as usize
}

pub struct Ctx<'a> {
    pub crawl: &'a Crawl,
    by_url: FxMap<&'a str, usize>,
    by_id: FxMap<usize, usize>,
    /// Per page: (source page index, link index in source's links).
    pub inlinks: Vec<Vec<(usize, usize)>>,
    titles: FxMap<&'a str, usize>,
    descs: FxMap<&'a str, usize>,
    keywords: FxMap<&'a str, usize>,
    h1s: FxMap<&'a str, usize>,
    h2s: FxMap<&'a str, usize>,
    hashes: FxMap<u64, usize>,
    near_dup: FxSet<usize>,
    sitemap: FxSet<&'a str>,
    ok_urls_lower: FxMap<String, usize>,
    start_id: Option<usize>,
    redirect_targets: HashSet<&'a str>,
    /// Per installed rule-pack test: its slot in this crawl's `HtmlData::rule_probes`.
    pub rule_slots: Vec<Option<usize>>,
}

impl<'a> Ctx<'a> {
    pub fn new(crawl: &'a Crawl) -> Ctx<'a> {
        let pages = &crawl.pages;
        let by_url: FxMap<&str, usize> = pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.url.as_str(), i))
            .collect();
        let by_id = pages.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        let mut inlinks = vec![Vec::new(); pages.len()];
        for (si, p) in pages.iter().enumerate() {
            // Only pages whose links are followed (internal 200 HTML); error pages' links are not.
            if let Some(h) = hd(p) {
                for (li, l) in h.links.iter().enumerate() {
                    if let Some(&ti) = by_url.get(l.url.as_str()) {
                        if ti != si {
                            inlinks[ti].push((si, li));
                        }
                    }
                }
            }
        }
        let mut titles = FxMap::default();
        let mut descs = FxMap::default();
        let mut keywords = FxMap::default();
        let mut h1s = FxMap::default();
        let mut h2s = FxMap::default();
        let mut hashes = FxMap::default();
        let mut ok_urls_lower: FxMap<String, usize> = FxMap::default();
        for p in pages {
            if ih(p) {
                *ok_urls_lower.entry(p.url.to_lowercase()).or_insert(0) += 1;
            }
            let Some(h) = ihd(p) else { continue };
            let bump = |m: &mut FxMap<&'a str, usize>, v: Option<&'a String>| {
                if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
                    *m.entry(v.as_str()).or_insert(0) += 1;
                }
            };
            bump(&mut titles, h.titles.first());
            bump(&mut descs, h.meta_descriptions.first());
            bump(&mut keywords, h.meta_keywords.first());
            bump(&mut h1s, h.h1.first());
            bump(&mut h2s, h.h2.first());
            if h.word_count >= 20 {
                *hashes.entry(h.content_hash).or_insert(0) += 1;
            }
        }
        let near_dup = near_duplicates(pages);
        let sitemap = crawl.site.sitemap_urls.iter().map(|s| s.as_str()).collect();
        let start_id = pages
            .iter()
            .find(|p| p.source == Some(Source::Seed))
            .map(|p| p.id);
        // URLs reached through a redirect from a crawled URL (link targets of 301s etc.).
        let redirect_targets = pages
            .iter()
            .filter(|p| {
                p.is_redirect() && !matches!(p.source, Some(Source::Robots) | Some(Source::Sitemap))
            })
            .filter_map(|p| p.redirect_to.as_deref())
            .collect();
        let mut ctx = Ctx {
            crawl,
            by_url,
            by_id,
            inlinks,
            titles,
            descs,
            keywords,
            h1s,
            h2s,
            hashes,
            near_dup,
            sitemap,
            ok_urls_lower,
            start_id,
            redirect_targets,
            rule_slots: crate::rules::slots(&crawl.site.rule_probes),
        };
        // When the start URL redirects (http -> https, bare -> www), site-level checks belong to the final page.
        if let Some(seed) = pages.iter().find(|p| p.source == Some(Source::Seed)) {
            if let (Some(f), _, false) = ctx.follow(&seed.url) {
                if f.is_internal {
                    ctx.start_id = Some(f.id);
                }
            }
        }
        ctx
    }

    /// The page has internal links pointing at it, directly or through a redirect.
    pub fn linked(&self, p: &Page) -> bool {
        !self.inlinks_of(p).is_empty() || self.redirect_targets.contains(p.url.as_str())
    }

    /// Only the URL index, for callers that just need [`Ctx::page`] / [`Ctx::follow`]
    /// (building the full context at 50k pages costs seconds).
    pub fn urls_only(crawl: &'a Crawl) -> Ctx<'a> {
        Ctx {
            crawl,
            by_url: crawl
                .pages
                .iter()
                .enumerate()
                .map(|(i, p)| (p.url.as_str(), i))
                .collect(),
            by_id: crawl
                .pages
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id, i))
                .collect(),
            inlinks: Vec::new(),
            titles: FxMap::default(),
            descs: FxMap::default(),
            keywords: FxMap::default(),
            h1s: FxMap::default(),
            h2s: FxMap::default(),
            hashes: FxMap::default(),
            near_dup: FxSet::default(),
            sitemap: FxSet::default(),
            ok_urls_lower: FxMap::default(),
            start_id: None,
            redirect_targets: Default::default(),
            rule_slots: crate::rules::slots(&crawl.site.rule_probes),
        }
    }

    pub fn page(&self, url: &str) -> Option<&'a Page> {
        self.by_url.get(url).map(|&i| &self.crawl.pages[i])
    }

    pub fn status(&self, url: &str) -> Option<u16> {
        self.page(url)
            .filter(|p| !p.blocked_by_robots)
            .map(|p| p.status)
    }

    pub fn index_of(&self, p: &Page) -> Option<usize> {
        self.by_id.get(&p.id).copied()
    }

    pub fn inlinks_of(&self, p: &Page) -> &[(usize, usize)] {
        self.index_of(p)
            .map(|i| self.inlinks[i].as_slice())
            .unwrap_or(&[])
    }

    pub fn is_start(&self, p: &Page) -> bool {
        Some(p.id) == self.start_id
    }

    pub fn in_sitemap(&self, p: &Page) -> bool {
        self.sitemap.contains(p.url.as_str())
    }

    pub fn has_sitemap(&self) -> bool {
        self.crawl.site.sitemaps.iter().any(|s| s.status == 200)
    }

    pub fn dup_title(&self, s: &str) -> bool {
        self.titles.get(s).copied().unwrap_or(0) > 1
    }
    pub fn dup_desc(&self, s: &str) -> bool {
        self.descs.get(s).copied().unwrap_or(0) > 1
    }
    pub fn dup_keywords(&self, s: &str) -> bool {
        self.keywords.get(s).copied().unwrap_or(0) > 1
    }
    pub fn dup_h1(&self, s: &str) -> bool {
        self.h1s.get(s).copied().unwrap_or(0) > 1
    }
    pub fn dup_h2(&self, s: &str) -> bool {
        self.h2s.get(s).copied().unwrap_or(0) > 1
    }
    pub fn dup_content(&self, h: u64) -> bool {
        self.hashes.get(&h).copied().unwrap_or(0) > 1
    }
    pub fn near_dup(&self, p: &Page) -> bool {
        self.near_dup.contains(&p.id)
    }
    pub fn case_variants(&self, p: &Page) -> bool {
        self.ok_urls_lower
            .get(&p.url.to_lowercase())
            .copied()
            .unwrap_or(0)
            > 1
    }

    /// Follow a redirect chain. Returns (final page, hops, looped).
    pub fn follow(&self, url: &str) -> (Option<&'a Page>, usize, bool) {
        let mut seen = HashSet::new();
        let mut cur = self.page(url);
        let mut hops = 0;
        while let Some(p) = cur {
            if !seen.insert(p.url.as_str()) {
                return (Some(p), hops, true);
            }
            match (&p.redirect_to, p.is_redirect()) {
                (Some(next), true) => {
                    hops += 1;
                    match self.page(next) {
                        Some(n) => cur = Some(n),
                        None => return (None, hops, false),
                    }
                }
                _ => return (Some(p), hops, false),
            }
            if hops > 20 {
                return (cur, hops, true);
            }
        }
        (None, hops, false)
    }

    /// Final status after redirects (None when the target was never fetched).
    pub fn final_status(&self, url: &str) -> Option<u16> {
        match self.follow(url) {
            (Some(p), _, false) if !p.blocked_by_robots => Some(p.status),
            _ => None,
        }
    }
}

/// Mark pages whose content is ~90%+ similar (simhash distance <= 6) but not identical.
// ponytail: LSH buckets capped at 400 entries; huge template-identical buckets compare partially.
fn near_duplicates(pages: &[Page]) -> FxSet<usize> {
    let cands: Vec<(usize, u64, u64)> = pages
        .iter()
        .filter_map(|p| {
            ihd(p)
                .filter(|h| h.word_count >= 50)
                .map(|h| (p.id, h.simhash, h.content_hash))
        })
        .collect();
    let mut out = FxSet::default();
    for band in 0..4 {
        let mut buckets: FxMap<u64, Vec<usize>> = FxMap::default();
        for (i, (_, sh, _)) in cands.iter().enumerate() {
            buckets
                .entry(sh >> (band * 16) & 0xffff)
                .or_default()
                .push(i);
        }
        for members in buckets.values() {
            let m = &members[..members.len().min(400)];
            for (a, &i) in m.iter().enumerate() {
                for &j in &m[a + 1..] {
                    let (ia, sa, ha) = cands[i];
                    let (ib, sb, hb) = cands[j];
                    if ha != hb && (sa ^ sb).count_ones() <= 6 {
                        out.insert(ia);
                        out.insert(ib);
                    }
                }
            }
        }
    }
    out
}

/// Result of running every check over a crawl.
#[derive(Default, Clone)]
pub struct Audit {
    /// Per check: indices of affected pages.
    pub hits: Vec<Vec<usize>>,
    /// Per page: indices of checks it fails.
    pub page_issues: Vec<Vec<usize>>,
    pub inlinks: Vec<Vec<(usize, usize)>>,
}

impl Audit {
    pub fn triggered(&self) -> impl Iterator<Item = (usize, &Vec<usize>)> {
        self.hits.iter().enumerate().filter(|(_, h)| !h.is_empty())
    }

    /// 0-100 score: each triggered check costs points by type, priority and spread.
    pub fn health_score(&self, crawl: &Crawl) -> u32 {
        let html = crawl.pages.iter().filter(|p| ih(p)).count();
        // Nothing loaded (TLS failure, DNS error, every page 4xx/5xx): the site is not healthy.
        if html == 0
            && crawl
                .pages
                .iter()
                .any(|p| p.is_internal && !p.blocked_by_robots)
        {
            return 0;
        }
        let base = html.max(1) as f32;
        let cat = catalogue();
        let mut penalty = 0.0f32;
        for (i, hits) in self.triggered() {
            let c = &cat[i];
            let w = match (c.kind, c.priority) {
                (Kind::Issue, Priority::High) => 6.0,
                (Kind::Issue, Priority::Medium) => 3.5,
                (Kind::Issue, Priority::Low) => 2.0,
                (Kind::Warning, Priority::High) => 2.5,
                (Kind::Warning, Priority::Medium) => 1.5,
                (Kind::Warning, Priority::Low) => 0.7,
                (Kind::Opportunity, _) => 0.3,
            };
            let spread = (hits.len() as f32 / base).min(1.0);
            penalty += w * (0.35 + 0.65 * spread);
        }
        (100.0 - penalty).clamp(0.0, 100.0).round() as u32
    }
}

pub fn run(crawl: &Crawl) -> Audit {
    let ctx = Ctx::new(crawl);
    let checks: &[Check] = catalogue();
    let pages = &crawl.pages;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    // Work items are (check, page chunk) pairs pulled from a shared counter, so the few
    // expensive link checks spread over every thread instead of stalling one of them.
    let chunk = pages.len().div_ceil(threads * 4).max(512);
    let n_chunks = pages.len().div_ceil(chunk).max(1);
    let total = checks.len() * n_chunks;
    let next = AtomicUsize::new(0);
    let mut parts: Vec<(usize, Vec<usize>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (ctx, next) = (&ctx, &next);
                s.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let t = next.fetch_add(1, Ordering::Relaxed);
                        if t >= total {
                            break out;
                        }
                        let (c, k) = (&checks[t / n_chunks], t % n_chunks);
                        let range = k * chunk..((k + 1) * chunk).min(pages.len());
                        // a check that panics on odd data loses only its own chunk
                        let hits: Vec<usize> =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                range.filter(|&i| (c.test)(&pages[i], ctx)).collect()
                            }))
                            .unwrap_or_default();
                        if !hits.is_empty() {
                            out.push((t, hits));
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    parts.sort_unstable_by_key(|(t, _)| *t);
    let mut hits: Vec<Vec<usize>> = vec![Vec::new(); checks.len()];
    for (t, h) in parts {
        hits[t / n_chunks].extend(h);
    }
    let mut page_issues = vec![Vec::new(); pages.len()];
    for (ci, pages) in hits.iter().enumerate() {
        for &pi in pages {
            page_issues[pi].push(ci);
        }
    }
    Audit {
        hits,
        page_issues,
        inlinks: ctx.inlinks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_site_scores_zero() {
        let page = Page {
            id: 1,
            url: "https://e.com/".into(),
            source: Some(Source::Seed),
            is_internal: true,
            error: Some("Connection failed: invalid peer certificate".into()),
            ..Default::default()
        };
        let crawl = Crawl {
            pages: vec![page],
            ..Default::default()
        };
        assert_eq!(run(&crawl).health_score(&crawl), 0);
    }
}
