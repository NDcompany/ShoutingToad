//! Data captured for every URL the spider touches.

use serde::{Deserialize, Serialize};

/// How the spider found a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Seed,
    Link,
    Redirect,
    Image,
    Script,
    Stylesheet,
    Canonical,
    Hreflang,
    Pagination,
    Amp,
    Sitemap,
    Robots,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Links are most of a crawl file, so default fields are left out of it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub url: String,
    pub raw_href: String,
    pub anchor: String,
    /// Anchor came from an image alt attribute (image link).
    #[serde(skip_serializing_if = "is_false")]
    pub image_link: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub nofollow: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub sponsored: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub ugc: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub target_blank: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub noopener: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fragment: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Image {
    pub url: String,
    /// `None` = no alt attribute at all, `Some("")` = empty alt (decorative).
    pub alt: Option<String>,
    pub has_width: bool,
    pub has_height: bool,
    pub lazy: bool,
    pub srcset: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Script {
    pub url: String,
    pub in_head: bool,
    pub is_async: bool,
    pub defer: bool,
    pub module: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Iframe {
    pub url: String,
    pub has_title: bool,
    pub has_size: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Form {
    pub action: String,
    pub has_password: bool,
}

/// Everything parsed out of an HTML document.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HtmlData {
    pub titles: Vec<String>,
    pub title_outside_head: bool,
    pub meta_descriptions: Vec<String>,
    pub description_outside_head: bool,
    pub meta_keywords: Vec<String>,
    pub meta_robots: Vec<String>,
    pub meta_googlebot: Vec<String>,
    pub robots_outside_head: bool,
    pub canonicals: Vec<String>,
    pub canonicals_raw: Vec<String>,
    pub canonical_outside_head: bool,
    pub rel_next: Vec<String>,
    pub rel_prev: Vec<String>,
    pub hreflang: Vec<(String, String)>,
    pub hreflang_outside_head: bool,
    pub amphtml: Option<String>,
    pub is_amp: bool,
    pub meta_refresh: Option<String>,
    pub base_href: Option<String>,

    pub h1: Vec<String>,
    pub h2: Vec<String>,
    /// Heading levels in document order (1..=6).
    pub heading_levels: Vec<u8>,
    pub empty_headings: usize,
    pub h1_image_only: bool,

    pub word_count: usize,
    pub sentence_count: usize,
    pub flesch: f32,
    pub text_ratio: f32,
    pub content_hash: u64,
    pub simhash: u64,
    pub lorem_ipsum: bool,
    /// Visible text, whitespace-normalised and capped, used for link suggestions and offline rewrites.
    pub text: String,
    /// First substantial paragraph of body copy (outside nav, header and footer).
    pub lead: String,

    pub links: Vec<Link>,
    pub javascript_links: usize,
    pub links_with_whitespace: usize,
    pub empty_links: usize,
    pub images: Vec<Image>,
    pub scripts: Vec<Script>,
    pub inline_scripts: usize,
    pub inline_script_bytes: usize,
    pub js_redirect: bool,
    pub document_write: bool,
    pub stylesheets: Vec<String>,
    pub inline_styles_bytes: usize,
    pub font_face_without_display: bool,
    pub iframes: Vec<Iframe>,
    pub forms: Vec<Form>,
    pub inputs_without_label: usize,

    pub lang: Option<String>,
    pub charset: Option<String>,
    pub viewport: Option<String>,
    pub has_doctype: bool,
    pub favicon: bool,
    pub apple_touch_icon: bool,
    pub og: Vec<(String, String)>,
    pub twitter: Vec<(String, String)>,
    pub json_ld: Vec<serde_json::Value>,
    pub json_ld_errors: Vec<String>,
    pub microdata: bool,
    pub rdfa: bool,

    pub dom_elements: usize,
    pub head_tags: usize,
    pub body_tags: usize,
    pub invalid_in_head: Vec<String>,
    pub ids: Vec<String>,
    pub duplicate_ids: usize,
    pub positive_tabindex: usize,
    pub buttons_without_name: usize,
    pub has_main: bool,
    pub autoplay_media: bool,
    pub videos_without_captions: usize,
    pub tables_without_headers: usize,
    pub deprecated_tags: Vec<String>,
    pub uses_frames: bool,
    pub uses_plugins: bool,
    pub aria_hidden_body: bool,
    pub generic_anchor_links: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Page {
    pub id: usize,
    pub url: String,
    pub source: Option<Source>,
    pub is_internal: bool,
    pub depth: usize,
    /// 0 when no HTTP response was received.
    pub status: u16,
    pub status_text: String,
    pub error: Option<String>,
    pub blocked_by_robots: bool,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub http_version: String,
    pub response_ms: u64,
    /// Bytes on the wire (compressed if the server compressed).
    pub transfer_size: usize,
    /// Bytes after decompression.
    pub size: usize,
    pub content_encoding: String,
    pub redirect_to: Option<String>,
    pub html: Option<Box<HtmlData>>,
    pub custom_search_hits: usize,
    pub custom_extract: Vec<String>,
}

impl Page {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn headers_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_html(&self) -> bool {
        self.html.is_some()
    }

    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status)
    }

    pub fn is_https(&self) -> bool {
        self.url.starts_with("https://")
    }

    pub fn kind(&self) -> &'static str {
        let ct = self.content_type.as_bytes();
        let has = |needle: &str| ct.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle.as_bytes()));
        let starts = |prefix: &str| ct.len() >= prefix.len() && ct[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes());
        if self.html.is_some() || has("html") {
            "HTML"
        } else if has("javascript") || has("ecmascript") {
            "JavaScript"
        } else if has("css") {
            "CSS"
        } else if starts("image/") {
            "Image"
        } else if has("pdf") {
            "PDF"
        } else if has("xml") {
            "XML"
        } else if starts("text/plain") {
            "Text"
        } else if self.status == 0 {
            "Unknown"
        } else {
            "Other"
        }
    }

    pub fn title(&self) -> &str {
        self.html.as_ref().and_then(|h| h.titles.first()).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn description(&self) -> &str {
        self.html.as_ref().and_then(|h| h.meta_descriptions.first()).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn h1(&self) -> &str {
        self.html.as_ref().and_then(|h| h.h1.first()).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn canonical(&self) -> &str {
        self.html.as_ref().and_then(|h| h.canonicals.first()).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn word_count(&self) -> usize {
        self.html.as_ref().map(|h| h.word_count).unwrap_or(0)
    }

    /// Meta robots + googlebot + X-Robots-Tag, lower-cased and joined.
    pub fn robots_directives(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(h) = &self.html {
            parts.extend(h.meta_robots.iter().cloned());
            parts.extend(h.meta_googlebot.iter().cloned());
        }
        parts.extend(self.headers_all("x-robots-tag").map(|s| s.to_string()));
        parts.join(", ").to_ascii_lowercase()
    }

    /// Directive names, with any user-agent prefix ("googlebot: noindex") removed and
    /// valued directives ("max-image-preview:none") reduced to their name.
    pub fn directive_names(&self) -> Vec<String> {
        self.robots_directives().split([',', ';']).map(directive_name).collect()
    }

    /// `d` must be lower-case. Allocation-free: this runs for every page in many checks.
    pub fn has_directive(&self, d: &str) -> bool {
        let meta = self.html.iter().flat_map(|h| h.meta_robots.iter().chain(&h.meta_googlebot).map(|s| s.as_str()));
        meta.chain(self.headers_all("x-robots-tag"))
            .any(|part| part.split([',', ';']).any(|t| directive_key(t).eq_ignore_ascii_case(d)))
    }

    pub fn is_noindex(&self) -> bool {
        self.has_directive("noindex") || self.has_directive("none")
    }

    pub fn is_nofollow_page(&self) -> bool {
        self.has_directive("nofollow") || self.has_directive("none")
    }
}

/// Name of one lower-case robots directive token: strips a user-agent prefix
/// ("googlebot: noindex") and the value of valued directives ("max-image-preview:none").
pub fn directive_name(token: &str) -> String {
    directive_key(token).to_ascii_lowercase()
}

/// Borrowing form of [`directive_name`] (case preserved).
pub fn directive_key(token: &str) -> &str {
    const VALUED: &[&str] = &["max-snippet", "max-image-preview", "max-video-preview", "unavailable_after"];
    let valued = |k: &str| VALUED.iter().any(|v| v.eq_ignore_ascii_case(k.trim()));
    match token.split_once(':') {
        Some((k, _)) if valued(k) => k.trim(),
        Some((_, v)) => match v.split_once(':') {
            Some((k, _)) if valued(k) => k.trim(),
            _ => v.trim(),
        },
        None => token.trim(),
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SitemapInfo {
    pub url: String,
    pub status: u16,
    pub valid_xml: bool,
    pub is_index: bool,
    pub url_count: usize,
    pub size: usize,
    pub in_robots: bool,
    pub invalid_lastmod: usize,
    pub other_host_urls: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RobotsInfo {
    pub url: String,
    pub status: u16,
    pub size: usize,
    pub redirected: bool,
    pub disallow_all: bool,
    pub crawl_delay: Option<f32>,
    pub sitemap_directives: Vec<String>,
    pub invalid_lines: usize,
    pub noindex_lines: usize,
}

/// Site-wide facts gathered outside normal page fetching.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SiteInfo {
    pub robots: Option<RobotsInfo>,
    pub sitemaps: Vec<SitemapInfo>,
    pub sitemap_urls: Vec<String>,
    /// False in list mode, where sitemaps are not looked for.
    pub sitemaps_checked: bool,
    /// Not every listed sitemap was read (crawl limit or sitemap cap reached).
    #[serde(default)]
    pub sitemaps_partial: bool,
    /// http:// version of the start URL redirects to https://.
    pub http_redirects_to_https: Option<bool>,
    /// The www / non-www alternate host redirects to the start host.
    pub alt_host_redirects: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Crawl {
    pub start_url: String,
    pub started: String,
    pub finished: String,
    pub pages: Vec<Page>,
    pub site: SiteInfo,
}

impl Crawl {
    /// Gzip-compressed JSON (a 50k-URL crawl is GBs of raw JSON). Pages are serialized and
    /// compressed in parallel chunks, each written as its own gzip member: a multi-member file
    /// is one valid gzip stream. Members are `{…,"pages":[`, page chunks, then `]}`.
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::io::Write;
        #[derive(Serialize)]
        struct Head<'a> {
            start_url: &'a str,
            started: &'a str,
            finished: &'a str,
            site: &'a SiteInfo,
        }
        // exhaustive on purpose: a new Crawl field must be added to Head too
        let Crawl { start_url, started, finished, pages, site } = self;
        let gz = gzip_member;
        let write = || -> std::io::Result<()> {
            let mut file = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
            let mut head = serde_json::to_vec(&Head { start_url, started, finished, site })?;
            head.pop(); // closing brace: the pages follow
            head.extend_from_slice(br#","pages":["#);
            file.write_all(&gz(&head)?)?;
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
            let chunks: Vec<&[Page]> = pages.chunks(256).collect();
            for (w, wave) in chunks.chunks(threads).enumerate() {
                let parts: Vec<std::io::Result<Vec<u8>>> = std::thread::scope(|s| {
                    let running: Vec<_> = wave
                        .iter()
                        .enumerate()
                        .map(|(k, chunk)| {
                            s.spawn(move || {
                                let mut json = Vec::new();
                                for (j, p) in chunk.iter().enumerate() {
                                    if w + k + j > 0 {
                                        json.push(b',');
                                    }
                                    serde_json::to_writer(&mut json, p)?;
                                }
                                gz(&json)
                            })
                        })
                        .collect();
                    running.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(std::io::Error::other("serializer crashed")))).collect()
                });
                for part in parts {
                    file.write_all(&part?)?;
                }
            }
            file.write_all(&gz(b"]}")?)?;
            file.flush()
        };
        write().map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    /// Reads gzip-compressed or plain JSON crawl files. Files written by [`Crawl::save`] are
    /// inflated and parsed in parallel chunks.
    pub fn load(path: &std::path::Path) -> Result<Crawl, String> {
        use std::io::Read;
        let damaged = |e: &dyn std::fmt::Display| format!("{} is damaged: {e}", path.display());
        let not_crawl = |e: serde_json::Error| format!("{} is not a ShoutingToad crawl file: {e}", path.display());
        let mut data = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        if let Some(members) = split_members(&data).filter(|m| m.len() >= 2) {
            let inflate = |m: &[u8]| -> Result<Vec<u8>, String> {
                let mut out = Vec::new();
                flate2::read::GzDecoder::new(m).read_to_end(&mut out).map_err(|e| damaged(&e))?;
                Ok(out)
            };
            let (head, rest) = members.split_first().unwrap_or((&members[0], &[]));
            let (tail, chunks) = rest.split_last().unwrap_or((&members[0], &[]));
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
            let parsed: Vec<Result<Vec<Page>, String>> = std::thread::scope(|s| {
                let running: Vec<_> = chunks
                    .chunks(chunks.len().div_ceil(threads).max(1))
                    .map(|group| {
                        s.spawn(move || {
                            let mut pages = Vec::new();
                            for m in group {
                                // ",{page},{page}" (no comma on the first chunk) -> "[{page},{page}]"
                                let mut json = inflate(m)?;
                                match json.first() {
                                    Some(b',') => json[0] = b'[',
                                    _ => json.insert(0, b'['),
                                }
                                json.push(b']');
                                pages.extend(serde_json::from_slice::<Vec<Page>>(&json).map_err(not_crawl)?);
                            }
                            Ok(pages)
                        })
                    })
                    .collect();
                running.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(damaged(&"parser crashed")))).collect()
            });
            let mut json = inflate(head)?;
            json.extend(inflate(tail)?);
            let mut crawl: Crawl = serde_json::from_slice(&json).map_err(not_crawl)?;
            for chunk in parsed {
                crawl.pages.extend(chunk?);
            }
            return Ok(crawl);
        }
        if data.starts_with(&[0x1f, 0x8b]) {
            let mut json = Vec::with_capacity(data.len() * 6);
            flate2::read::MultiGzDecoder::new(&data[..]).read_to_end(&mut json).map_err(|e| damaged(&e))?;
            data = json;
        }
        serde_json::from_slice(&data).map_err(not_crawl)
    }
}

/// Gzip member whose header carries its own total length in an extra subfield ("ST"), so the
/// loader can find every member without inflating. Members over 4 GiB get a plain header.
fn gzip_member(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::with_capacity(data.len() / 3), flate2::Compression::fast());
    deflate.write_all(data)?;
    let body = deflate.finish()?;
    let mut crc = flate2::Crc::new();
    crc.update(data);
    let total = 20 + body.len() + 8;
    let mut out = Vec::with_capacity(total);
    match u32::try_from(total) {
        // ID1 ID2 CM=deflate FLG=FEXTRA MTIME(4) XFL OS=unknown, XLEN=8, "ST" LEN=4 total
        Ok(t) => {
            out.extend_from_slice(&[0x1f, 0x8b, 8, 4, 0, 0, 0, 0, 0, 0xff, 8, 0, b'S', b'T', 4, 0]);
            out.extend_from_slice(&t.to_le_bytes());
        }
        Err(_) => out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff]),
    }
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.sum().to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // ISIZE is the length mod 2^32
    Ok(out)
}

/// Members of a file written by [`gzip_member`]; `None` for any other file.
fn split_members(data: &[u8]) -> Option<Vec<&[u8]>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let h = data.get(pos..pos + 20)?;
        if h[..4] != [0x1f, 0x8b, 8, 4] || h[10..16] != [8, 0, b'S', b'T', 4, 0] {
            return None;
        }
        let len = u32::from_le_bytes([h[16], h[17], h[18], h[19]]) as usize;
        if len < 28 {
            return None;
        }
        out.push(data.get(pos..pos + len)?);
        pos += len;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip_and_plain_json() {
        let link = Link { url: "https://a.test/x".into(), nofollow: true, ..Default::default() };
        let page = Page { url: "https://a.test/".into(), html: Some(Box::new(HtmlData { links: vec![link], ..Default::default() })), ..Default::default() };
        let mut pages = vec![page];
        pages.extend((1..5000).map(|id| Page { id, url: format!("https://a.test/{id}"), ..Default::default() }));
        let crawl = Crawl { start_url: "https://a.test/".into(), pages, ..Default::default() };
        let dir = std::env::temp_dir().join(format!("st-model-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (gz, plain) = (dir.join("c.shoutingtoad"), dir.join("old.shoutingtoad"));
        crawl.save(&gz).unwrap();
        std::fs::write(&plain, serde_json::to_vec(&crawl).unwrap()).unwrap();
        for path in [&gz, &plain] {
            let back = Crawl::load(path).unwrap();
            let l = &back.pages[0].html.as_ref().unwrap().links[0];
            assert!(l.nofollow && !l.sponsored && l.fragment.is_none());
            assert_eq!(back.start_url, crawl.start_url);
            assert!(back.pages.iter().enumerate().all(|(i, p)| p.id == i) && back.pages.len() == 5000);
        }
        // a truncated or foreign file is an error, not a panic
        let bytes = std::fs::read(&gz).unwrap();
        std::fs::write(&plain, &bytes[..bytes.len() / 2]).unwrap();
        assert!(Crawl::load(&plain).is_err());
        std::fs::write(&plain, [&[0x1f, 0x8b, 8, 4][..], b"garbage"].concat()).unwrap();
        assert!(Crawl::load(&plain).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod directive_tests {
    use super::*;

    fn page(robots: &str) -> Page {
        Page { html: Some(Box::new(HtmlData { meta_robots: vec![robots.into()], ..Default::default() })), ..Default::default() }
    }

    #[test]
    fn valued_directives_are_not_none_or_noindex() {
        let p = page("max-image-preview:none, max-snippet:-1");
        assert!(!p.is_noindex() && !p.has_directive("none"));
        assert!(p.has_directive("max-image-preview"));
        assert!(page("googlebot: noindex").is_noindex());
        assert!(page("googlebot: max-image-preview:none").has_directive("max-image-preview"));
        assert!(page("unavailable_after: 2026-01-01T00:00:00Z").has_directive("unavailable_after"));
    }
}
