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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Link {
    pub url: String,
    pub raw_href: String,
    pub anchor: String,
    /// Anchor came from an image alt attribute (image link).
    pub image_link: bool,
    pub nofollow: bool,
    pub sponsored: bool,
    pub ugc: bool,
    pub target_blank: bool,
    pub noopener: bool,
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
        let ct = self.content_type.to_ascii_lowercase();
        if self.html.is_some() || ct.contains("html") {
            "HTML"
        } else if ct.contains("javascript") || ct.contains("ecmascript") {
            "JavaScript"
        } else if ct.contains("css") {
            "CSS"
        } else if ct.starts_with("image/") {
            "Image"
        } else if ct.contains("pdf") {
            "PDF"
        } else if ct.contains("xml") {
            "XML"
        } else if ct.starts_with("text/plain") {
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

    pub fn has_directive(&self, d: &str) -> bool {
        self.robots_directives()
            .split([',', ';'])
            .map(|t| t.trim())
            .map(|t| t.rsplit(':').next().unwrap_or(t).trim())
            .any(|t| t == d)
    }

    pub fn is_noindex(&self) -> bool {
        self.has_directive("noindex") || self.has_directive("none")
    }

    pub fn is_nofollow_page(&self) -> bool {
        self.has_directive("nofollow") || self.has_directive("none")
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
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Crawl, String> {
        let data = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        serde_json::from_slice(&data).map_err(|e| format!("{} is not a ShoutingToad crawl file: {e}", path.display()))
    }
}
