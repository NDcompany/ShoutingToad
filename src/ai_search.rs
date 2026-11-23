//! AI search readiness: which AI crawlers can really fetch the site (robots.txt rules *and* what
//! the server answers to each crawler's user agent), what they see without JavaScript, and how
//! quotable each page's sections are.

use crate::model::{Crawl, HtmlData, Page};
use crate::robots::Robots;
use scraper::ElementRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

// ------------------------------------------------------------------ bot registry

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Purpose {
    /// Collects content to train AI models.
    Training,
    /// Indexes pages for an AI search or answer engine.
    Search,
    /// Fetches a page when a person asks the assistant about it.
    User,
}

impl Purpose {
    pub const ALL: [Purpose; 3] = [Purpose::Search, Purpose::User, Purpose::Training];

    pub fn label(self) -> &'static str {
        match self {
            Purpose::Training => "Training",
            Purpose::Search => "Search / answers",
            Purpose::User => "User-triggered",
        }
    }
}

pub struct Bot {
    /// robots.txt product token, as the vendor writes it.
    pub name: &'static str,
    pub operator: &'static str,
    pub purpose: Purpose,
    /// Full user agent. `None` for robots.txt-only control tokens: the vendor fetches with
    /// another crawler, so there is nothing to fetch with.
    pub ua: Option<&'static str>,
    /// The vendor says robots.txt may not apply because a person asked for the page.
    pub may_ignore_robots: bool,
    /// Where the user agent comes from.
    pub source: &'static str,
}

/// AI crawlers, checked against each vendor's own documentation in November 2026. Vendors bump
/// version numbers in their user agents; sites match on the product token, so an old version
/// number still tests the same rules. Keep this the only place bot details live.
pub const BOTS: &[Bot] = &[
    // ---- search / answer engines
    Bot {
        name: "OAI-SearchBot",
        operator: "OpenAI",
        purpose: Purpose::Search,
        ua: Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36; compatible; OAI-SearchBot/1.4; +https://openai.com/searchbot"),
        may_ignore_robots: false,
        source: "https://developers.openai.com/api/docs/bots",
    },
    Bot {
        name: "Claude-SearchBot",
        operator: "Anthropic",
        purpose: Purpose::Search,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Claude-SearchBot/1.0; +https://www.anthropic.com)"),
        may_ignore_robots: false,
        source: "Anthropic publishes the token only; full string follows the ClaudeBot format",
    },
    Bot {
        name: "PerplexityBot",
        operator: "Perplexity",
        purpose: Purpose::Search,
        ua: Some("Mozilla/5.0 (compatible; PerplexityBot/1.0; +https://perplexity.ai/perplexitybot)"),
        may_ignore_robots: false,
        source: "https://docs.perplexity.ai/guides/bots",
    },
    Bot {
        name: "Applebot",
        operator: "Apple",
        purpose: Purpose::Search,
        ua: Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 (Applebot/0.1; +http://www.apple.com/go/applebot)"),
        may_ignore_robots: false,
        source: "https://support.apple.com/en-us/119829",
    },
    Bot {
        name: "DuckAssistBot",
        operator: "DuckDuckGo",
        purpose: Purpose::Search,
        ua: Some("DuckAssistBot/1.2; (+http://duckduckgo.com/duckassistbot.html)"),
        may_ignore_robots: false,
        source: "https://duckduckgo.com/duckduckgo-help-pages/results/duckassistbot",
    },
    // ---- fetchers a person triggers
    Bot {
        name: "ChatGPT-User",
        operator: "OpenAI",
        purpose: Purpose::User,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; ChatGPT-User/1.0; +https://openai.com/bot"),
        may_ignore_robots: true,
        source: "https://developers.openai.com/api/docs/bots",
    },
    Bot {
        name: "Claude-User",
        operator: "Anthropic",
        purpose: Purpose::User,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Claude-User/1.0; +https://www.anthropic.com)"),
        may_ignore_robots: false,
        source: "Anthropic publishes the token only; full string follows the ClaudeBot format",
    },
    Bot {
        name: "Perplexity-User",
        operator: "Perplexity",
        purpose: Purpose::User,
        ua: Some("Mozilla/5.0 (compatible; Perplexity-User/1.0; +https://perplexity.ai/perplexity-user)"),
        may_ignore_robots: true,
        source: "https://docs.perplexity.ai/guides/bots",
    },
    Bot {
        name: "Meta-ExternalFetcher",
        operator: "Meta",
        purpose: Purpose::User,
        ua: Some("meta-externalfetcher/1.1 (+https://developers.facebook.com/docs/sharing/webmasters/crawler)"),
        may_ignore_robots: true,
        source: "https://developers.facebook.com/docs/sharing/webmasters/web-crawlers",
    },
    Bot {
        name: "MistralAI-User",
        operator: "Mistral AI",
        purpose: Purpose::User,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; MistralAI-User/1.0; +https://docs.mistral.ai/robots)"),
        may_ignore_robots: false,
        source: "https://docs.mistral.ai/robots",
    },
    // ---- model training
    Bot {
        name: "GPTBot",
        operator: "OpenAI",
        purpose: Purpose::Training,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; GPTBot/1.4; +https://openai.com/gptbot"),
        may_ignore_robots: false,
        source: "https://developers.openai.com/api/docs/bots",
    },
    Bot {
        name: "ClaudeBot",
        operator: "Anthropic",
        purpose: Purpose::Training,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaudeBot/1.0; +claudebot@anthropic.com)"),
        may_ignore_robots: false,
        source: "Anthropic publishes the token only; full string as seen in server logs",
    },
    Bot {
        name: "Google-Extended",
        operator: "Google",
        purpose: Purpose::Training,
        ua: None,
        may_ignore_robots: false,
        source: "https://developers.google.com/search/docs/crawling-indexing/google-common-crawlers (robots.txt token only; Google fetches with its usual crawlers)",
    },
    Bot {
        name: "Applebot-Extended",
        operator: "Apple",
        purpose: Purpose::Training,
        ua: None,
        may_ignore_robots: false,
        source: "https://support.apple.com/en-us/119829 (robots.txt token only; Apple fetches with Applebot)",
    },
    Bot {
        name: "CCBot",
        operator: "Common Crawl",
        purpose: Purpose::Training,
        ua: Some("CCBot/2.0 (https://commoncrawl.org/faq/)"),
        may_ignore_robots: false,
        source: "https://commoncrawl.org/ccbot",
    },
    Bot {
        name: "Meta-ExternalAgent",
        operator: "Meta",
        purpose: Purpose::Training,
        ua: Some("meta-externalagent/1.1 (+https://developers.facebook.com/docs/sharing/webmasters/crawler)"),
        may_ignore_robots: false,
        source: "https://developers.facebook.com/docs/sharing/webmasters/web-crawlers",
    },
    Bot {
        name: "Bytespider",
        operator: "ByteDance",
        purpose: Purpose::Training,
        ua: Some("Mozilla/5.0 (Linux; Android 5.0) AppleWebKit/537.36 (KHTML, like Gecko) Mobile Safari/537.36 (compatible; Bytespider; spider-feedback@bytedance.com)"),
        may_ignore_robots: false,
        source: "Not documented by ByteDance; string as seen in server logs",
    },
    Bot {
        name: "Amazonbot",
        operator: "Amazon",
        purpose: Purpose::Training,
        ua: Some("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot) Chrome/119.0.6045.214 Safari/537.36"),
        may_ignore_robots: false,
        source: "https://developer.amazon.com/amazonbot (also used for Alexa answers)",
    },
];

/// Registry entry for a robots.txt token or meta name (case-insensitive).
pub fn bot(name: &str) -> Option<&'static Bot> {
    BOTS.iter()
        .find(|b| b.name.eq_ignore_ascii_case(name.trim()))
}

// ------------------------------------------------------------------ stored results

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// Robots-only token, or robots.txt disallows the page for a bot that obeys it.
    #[default]
    NotFetched,
    /// Same answer and content as a normal request.
    Ok,
    /// A redirect where a normal request got the page.
    Redirected,
    /// 200, but less than half the words a normal request got.
    LessContent,
    /// 401/403/406/451 or another 4xx where a normal request got 200.
    Blocked,
    /// A bot challenge or captcha page (Cloudflare, DataDome, Imperva…).
    Challenged,
    /// 429 Too Many Requests: may be this test's own pace, so not reported as a block.
    RateLimited,
    /// 5xx or no response where a normal request worked.
    Error,
    /// The normal request failed too, so there is nothing to compare with.
    Inconclusive,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::NotFetched => "Not fetched",
            Outcome::Ok => "OK",
            Outcome::Redirected => "Redirected",
            Outcome::LessContent => "Less content",
            Outcome::Blocked => "Blocked",
            Outcome::Challenged => "Challenge",
            Outcome::RateLimited => "Rate limited",
            Outcome::Error => "Error",
            Outcome::Inconclusive => "Inconclusive",
        }
    }

    /// The server or CDN refused this bot while a normal request worked.
    pub fn refused(self) -> bool {
        matches!(
            self,
            Outcome::Blocked | Outcome::Challenged | Outcome::Error
        )
    }
}

/// One tested page and what a normal request (the crawl's user agent) got.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TestedPage {
    pub url: String,
    pub status: u16,
    /// Words in the raw HTML: what a crawler that does not run JavaScript sees.
    pub words: usize,
    /// Challenge / error text when the normal request was refused too.
    pub problem: String,
    pub noindex: bool,
}

/// One bot on one tested page.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BotResult {
    pub bot: String,
    /// Index into [`AiBotReport::pages`].
    pub page: usize,
    pub robots_allowed: bool,
    pub outcome: Outcome,
    pub status: u16,
    pub words: usize,
    /// What refused or changed the request: CDN / firewall, challenge type, redirect target, error.
    pub detail: String,
    /// The response served to this bot carries noindex (meta or header) the normal one does not.
    pub noindex: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiBotReport {
    pub pages: Vec<TestedPage>,
    pub rows: Vec<BotResult>,
    /// robots.txt was found and parsed.
    pub robots_found: bool,
    /// HTTP status of /llms.txt (0 = no response).
    pub llms_txt_status: u16,
    /// /llms.txt answered 200 with plain text (not an HTML error page).
    pub llms_txt_ok: bool,
}

impl AiBotReport {
    pub fn page_index(&self, url: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.url == url)
    }

    /// Rows for one tested page, with their registry entry.
    pub fn rows_for(&self, page: usize) -> impl Iterator<Item = (&'static Bot, &BotResult)> {
        self.rows
            .iter()
            .filter(move |r| r.page == page)
            .filter_map(|r| bot(&r.bot).map(|b| (b, r)))
    }

    /// A bot cannot get the page: robots.txt it obeys disallows it, or the server refuses it.
    pub fn blocked(b: &Bot, r: &BotResult) -> bool {
        (!r.robots_allowed && !b.may_ignore_robots) || r.outcome.refused()
    }
}

/// Per-page AI data kept in [`HtmlData`]: small on purpose (it is stored for every page).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiData {
    /// Robots meta tags that name an AI crawler: (meta name, content).
    pub bot_meta: Vec<(String, String)>,
    /// Main-content sections (heading + at least 10 words).
    pub sections: u16,
    /// Sections with no quotability problem.
    pub quotable: u16,
    /// Every problem found on any section (see [`SHORT`] …).
    pub flags: u8,
    /// Heading of the first section with a problem, and its problems.
    pub weakest: String,
    pub weakest_flags: u8,
}

// ------------------------------------------------------------------ choosing pages to test

/// Collects, while the crawl runs, how often each internal URL is linked and which are
/// indexable, so the access test can pick the most-linked pages without keeping pages around.
#[derive(Default)]
pub struct Sampler(Mutex<SamplerState>);

#[derive(Default)]
struct SamplerState {
    /// fnv64(url) -> (pages linking to it, indexable)
    urls: HashMap<u64, (u32, bool)>,
    /// Lowest-id depth-0 HTML page: the start page after redirects.
    home: Option<(usize, String)>,
}

/// "scheme://host[:port]/" of a URL, without parsing it.
fn origin_prefix(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    match url[after..].find('/') {
        Some(i) => &url[..after + i + 1],
        None => url,
    }
}

impl Sampler {
    pub fn note(&self, p: &Page) {
        let Some(h) = p.html.as_deref() else { return };
        if !p.is_internal || p.status != 200 {
            return;
        }
        let prefix = origin_prefix(&p.url);
        let mut targets: Vec<u64> = h
            .links
            .iter()
            .filter(|l| l.url.starts_with(prefix) && l.url != p.url)
            .map(|l| crate::parse::fnv64(l.url.as_bytes()))
            .collect();
        targets.sort_unstable();
        targets.dedup();
        let ok = crate::audit::indexable(p);
        let mut s = self.0.lock().unwrap();
        for t in targets {
            s.urls.entry(t).or_default().0 += 1;
        }
        if ok {
            s.urls
                .entry(crate::parse::fnv64(p.url.as_bytes()))
                .or_default()
                .1 = true;
        }
        if p.depth == 0 && s.home.as_ref().is_none_or(|(id, _)| p.id < *id) {
            s.home = Some((p.id, p.url.clone()));
        }
    }

    /// The start page, then the most-linked indexable pages: `n` URLs at most.
    pub fn pick<'a>(&self, seen: impl Iterator<Item = &'a String>, n: usize) -> Vec<String> {
        let s = self.0.lock().unwrap();
        let Some((_, home)) = s.home.clone() else {
            return Vec::new();
        };
        let mut ranked: Vec<(u32, &String)> = seen
            .filter(|u| **u != home)
            .filter_map(|u| match s.urls.get(&crate::parse::fnv64(u.as_bytes())) {
                Some(&(links, true)) => Some((links, u)),
                _ => None,
            })
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        std::iter::once(home.clone())
            .chain(ranked.into_iter().map(|(_, u)| u.clone()))
            .take(n)
            .collect()
    }
}

// ------------------------------------------------------------------ the access test

pub struct Fetched {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Decoded body.
    pub body: String,
}

/// Fetches a URL, with the given user agent or (None) the crawl's own.
pub type FetchFn<'a> = dyn Fn(&str, Option<&str>) -> Result<Fetched, String> + Sync + 'a;

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn path_and_query(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        },
        Err(_) => "/".into(),
    }
}

/// CDN / firewall named by the response headers.
fn vendor(headers: &[(String, String)]) -> &'static str {
    let has = |n: &str| header(headers, n).is_some();
    let server = header(headers, "server").unwrap_or("").to_ascii_lowercase();
    if has("cf-ray") || server.contains("cloudflare") {
        "Cloudflare"
    } else if has("x-datadome") || server.contains("datadome") {
        "DataDome"
    } else if server.contains("akamai") {
        "Akamai"
    } else if has("x-iinfo") || header(headers, "x-cdn").is_some_and(|v| v.contains("Imperva")) {
        "Imperva"
    } else if has("x-sucuri-id") {
        "Sucuri"
    } else if has("x-vercel-id") || has("x-vercel-mitigated") {
        "Vercel"
    } else if has("x-amz-cf-id") {
        "Amazon CloudFront"
    } else if has("x-served-by")
        && header(headers, "x-served-by").is_some_and(|v| v.contains("cache-"))
    {
        "Fastly"
    } else {
        "the server"
    }
}

/// A bot challenge or captcha page. Body markers only count on error statuses or near-empty
/// pages: Cloudflare injects its challenge-platform script into ordinary pages too.
fn challenge(f: &Fetched, words: usize) -> Option<String> {
    if let Some(v) = header(&f.headers, "cf-mitigated") {
        return Some(format!("Cloudflare challenge (cf-mitigated: {})", v.trim()));
    }
    if let Some(v) = header(&f.headers, "x-vercel-mitigated") {
        return Some(format!(
            "Vercel challenge (x-vercel-mitigated: {})",
            v.trim()
        ));
    }
    if f.status < 400 && words >= 150 {
        return None;
    }
    let b = f.body.as_str();
    let markers: [(&str, &str); 9] = [
        ("cf_chl_opt", "Cloudflare challenge"),
        ("challenges.cloudflare.com", "Cloudflare challenge"),
        ("/cdn-cgi/challenge-platform", "Cloudflare challenge"),
        ("captcha-delivery.com", "DataDome captcha"),
        ("_Incapsula_Resource", "Imperva challenge"),
        ("px-captcha", "HUMAN (PerimeterX) captcha"),
        ("g-recaptcha", "reCAPTCHA"),
        ("h-captcha", "hCaptcha"),
        ("sgcaptcha", "SiteGround captcha"),
    ];
    markers
        .iter()
        .find(|(m, _)| b.contains(m))
        .map(|(_, what)| what.to_string())
}

/// `directives` (a robots meta content or one X-Robots-Tag value) carries noindex/none for
/// `token`: unprefixed directives apply to every crawler, "gptbot: noindex" only to GPTBot.
pub fn noindex_for(directives: &str, token: Option<&str>) -> bool {
    let (target, rest) = match directives.split_once(':') {
        Some((k, v))
            if !k.trim().is_empty()
                && !k.trim().contains(' ')
                && !k.contains(',')
                && !matches!(
                    k.trim().to_ascii_lowercase().as_str(),
                    "max-snippet" | "max-image-preview" | "max-video-preview" | "unavailable_after"
                ) =>
        {
            (Some(k.trim()), v)
        }
        _ => (None, directives),
    };
    let applies = match (target, token) {
        (None, _) => true,
        (Some(t), Some(tok)) => t.eq_ignore_ascii_case(tok),
        (Some(_), None) => false,
    };
    applies
        && rest.split([',', ';']).any(|d| {
            let d = d.trim();
            d.eq_ignore_ascii_case("noindex") || d.eq_ignore_ascii_case("none")
        })
}

/// noindex for `token` in a response: robots meta, a meta tag naming the bot, or X-Robots-Tag.
fn response_noindex(h: &HtmlData, headers: &[(String, String)], token: Option<&str>) -> bool {
    h.meta_robots.iter().any(|m| noindex_for(m, None))
        || h.ai.bot_meta.iter().any(|(name, content)| {
            token.is_some_and(|t| name.eq_ignore_ascii_case(t)) && noindex_for(content, None)
        })
        || headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("x-robots-tag"))
            .any(|(_, v)| noindex_for(v, token))
}

/// Status, word count, refusal text and noindex of one response.
fn inspect(url: &str, f: &Fetched, token: Option<&str>) -> (usize, Option<String>, bool) {
    let base = url::Url::parse(url).unwrap_or_else(|_| url::Url::parse("http://invalid/").unwrap());
    let h = crate::parse::parse_html(&f.body, &base);
    let words = h.word_count;
    (
        words,
        challenge(f, words),
        response_noindex(&h, &f.headers, token),
    )
}

fn baseline(url: &str, r: Result<Fetched, String>) -> TestedPage {
    let mut t = TestedPage {
        url: url.to_string(),
        ..Default::default()
    };
    match r {
        Ok(f) => {
            let (words, ch, noindex) = inspect(url, &f, None);
            t.status = f.status;
            t.words = words;
            t.noindex = noindex;
            if let Some(c) = ch {
                t.problem = c;
            } else if f.status != 200 {
                t.problem = format!("HTTP {}", f.status);
            }
        }
        Err(e) => t.problem = e,
    }
    t
}

/// Compare a bot's response with the normal one.
fn classify(base: &TestedPage, url: &str, bot: &Bot, r: Result<Fetched, String>) -> BotResult {
    let mut out = BotResult {
        bot: bot.name.to_string(),
        robots_allowed: true,
        ..Default::default()
    };
    let base_ok = base.status == 200 && base.problem.is_empty();
    let f = match r {
        Ok(f) => f,
        Err(e) => {
            out.outcome = if base_ok {
                Outcome::Error
            } else {
                Outcome::Inconclusive
            };
            out.detail = e;
            return out;
        }
    };
    let (words, ch, noindex) = inspect(url, &f, Some(bot.name));
    out.status = f.status;
    out.words = words;
    out.noindex = noindex && !base.noindex;
    let refused_by = || format!("{} (HTTP {})", vendor(&f.headers), f.status);
    (out.outcome, out.detail) = if !base_ok {
        (Outcome::Inconclusive, base.problem.clone())
    } else if let Some(c) = ch {
        (Outcome::Challenged, c)
    } else if f.status == 429 {
        (Outcome::RateLimited, refused_by())
    } else if (300..400).contains(&f.status) {
        let to = header(&f.headers, "location")
            .unwrap_or("")
            .trim()
            .to_string();
        (Outcome::Redirected, format!("to {to}"))
    } else if (400..500).contains(&f.status) {
        (Outcome::Blocked, refused_by())
    } else if f.status >= 500 || f.status < 200 {
        (Outcome::Error, refused_by())
    } else if base.words >= 50 && words * 2 < base.words {
        (
            Outcome::LessContent,
            format!("{words} words vs {} for a normal request", base.words),
        )
    } else {
        (Outcome::Ok, String::new())
    };
    out
}

/// Run `work(i)` for `0..n` on up to `threads` threads, honouring stop and pause.
fn parallel(
    n: usize,
    threads: usize,
    stop: &AtomicBool,
    pause: &AtomicBool,
    work: impl Fn(usize) + Sync,
) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, 4).min(n.max(1)) {
            s.spawn(|| loop {
                while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(150));
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n || stop.load(Ordering::Relaxed) {
                    break;
                }
                work(i);
            });
        }
    });
}

/// Test every bot in [`BOTS`] on `urls` (the start page first). Robots-only tokens and bots
/// that robots.txt keeps out are evaluated but not fetched. Roughly
/// `urls × fetching bots + urls + 1` requests, made through `fetch` (which applies the crawl delay).
pub fn test_bots(
    urls: &[String],
    robots: Option<&Robots>,
    fetch: &FetchFn,
    stop: &AtomicBool,
    pause: &AtomicBool,
    threads: usize,
) -> AiBotReport {
    let mut report = AiBotReport {
        robots_found: robots.is_some(),
        ..Default::default()
    };
    let Some(first) = urls.first() else {
        return report;
    };
    let pages: Vec<Mutex<TestedPage>> = urls.iter().map(|_| Mutex::default()).collect();
    parallel(urls.len(), threads, stop, pause, |i| {
        *pages[i].lock().unwrap() = baseline(&urls[i], fetch(&urls[i], None));
    });
    report.pages = pages.into_iter().map(|m| m.into_inner().unwrap()).collect();

    let mut rows = Vec::new();
    let mut jobs = Vec::new();
    for (pi, url) in urls.iter().enumerate() {
        let path = path_and_query(url);
        for b in BOTS {
            let allowed = robots.is_none_or(|r| r.is_allowed_token(&path, b.name));
            if b.ua.is_some() && (allowed || b.may_ignore_robots) {
                jobs.push(rows.len());
            }
            rows.push(BotResult {
                bot: b.name.to_string(),
                page: pi,
                robots_allowed: allowed,
                ..Default::default()
            });
        }
    }
    let rows_m: Vec<Mutex<BotResult>> = rows.into_iter().map(Mutex::new).collect();
    parallel(jobs.len(), threads, stop, pause, |j| {
        let mut row = rows_m[jobs[j]].lock().unwrap();
        let Some(b) = bot(&row.bot) else { return };
        let url = &urls[row.page];
        let r = fetch(url, b.ua);
        let base = &report.pages[row.page];
        let res = classify(base, url, b, r);
        *row = BotResult {
            page: row.page,
            robots_allowed: row.robots_allowed,
            ..res
        };
    });
    report.rows = rows_m
        .into_iter()
        .map(|m| m.into_inner().unwrap())
        .collect();

    if !stop.load(Ordering::Relaxed) {
        let llms = format!("{}llms.txt", origin_prefix(first));
        if let Ok(f) = fetch(&llms, None) {
            report.llms_txt_status = f.status;
            report.llms_txt_ok = f.status == 200
                && !header(&f.headers, "content-type").is_some_and(|c| c.contains("html"))
                && !f.body.trim_start().starts_with('<')
                && !f.body.trim().is_empty();
        }
    }
    report
}

// ------------------------------------------------------------------ quotability

/// Fewer than 40 words: too thin to answer anything on its own.
pub const SHORT: u8 = 1;
/// Over 300 words: an answer engine has to cut it, so split it with subheadings.
pub const LONG: u8 = 2;
/// Opens with "This", "It", "They"…: quoted alone, the reader does not know what it refers to.
pub const DANGLING: u8 = 4;
/// The first sentence does not name the heading's topic (or runs past 35 words).
pub const OFF_TOPIC: u8 = 8;
/// No number, date or name anywhere in the section.
pub const NO_FACTS: u8 = 16;

pub fn flag_labels(flags: u8) -> Vec<&'static str> {
    [
        (SHORT, "under 40 words"),
        (LONG, "over 300 words"),
        (DANGLING, "opens with this/it/they"),
        (OFF_TOPIC, "first sentence does not answer the heading"),
        (NO_FACTS, "no numbers or names"),
    ]
    .iter()
    .filter(|(f, _)| flags & f != 0)
    .map(|(_, l)| *l)
    .collect()
}

const DANGLING_WORDS: &[&str] = &[
    "this",
    "that",
    "these",
    "those",
    "it",
    "it's",
    "its",
    "they",
    "their",
    "them",
    "he",
    "she",
    "such",
    "also",
    "additionally",
    "however",
    "furthermore",
    "moreover",
];

const STOP: &[&str] = &[
    "what", "when", "where", "which", "while", "why", "how", "does", "your", "with", "from",
    "that", "this", "about", "into", "have", "will", "should", "could", "would", "there", "their",
    "they", "them", "then", "than", "more", "most", "some", "each", "other", "best", "make",
];

fn clean(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase()
}

/// Text up to the first sentence end in the first non-empty block.
fn first_sentence(text: &str) -> &str {
    let block = text
        .split('\n')
        .find(|b| b.chars().any(|c| c.is_alphanumeric()))
        .unwrap_or("")
        .trim();
    let mut prev_space_after = None;
    for (i, c) in block.char_indices() {
        if matches!(c, '.' | '!' | '?') {
            let next = block[i + c.len_utf8()..].chars().next();
            if next.is_none_or(|n| n.is_whitespace()) {
                prev_space_after = Some(i + c.len_utf8());
                break;
            }
        }
    }
    &block[..prev_space_after.unwrap_or(block.len())]
}

/// The first sentence shares a significant word (4-letter stem) with the heading.
fn names_topic(heading: &str, sentence: &str) -> bool {
    let stems = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(clean)
            .filter(|w| w.chars().count() >= 4 && !STOP.contains(&w.as_str()))
            .map(|w| w.chars().take(4).collect())
            .collect()
    };
    let h = stems(heading);
    if h.is_empty() {
        return true;
    }
    let s = stems(sentence);
    h.iter().any(|w| s.contains(w))
}

/// A digit, or a capitalised word that does not start a sentence (a name).
fn has_facts(text: &str) -> bool {
    if text.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    for line in text.split('\n') {
        let mut sentence_start = true;
        for w in line.split_whitespace() {
            let first = w.chars().find(|c| c.is_alphabetic());
            if !sentence_start && w != "I" && first.is_some_and(|c| c.is_uppercase()) {
                return true;
            }
            if first.is_some() {
                sentence_start = w.ends_with(['.', '!', '?', ':']);
            }
        }
    }
    false
}

/// Problems of one section; `None` when it has under 10 words (labels, cards, widgets).
pub fn section_flags(heading: &str, text: &str) -> Option<u8> {
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .count();
    if words < 10 {
        return None;
    }
    let mut f = 0;
    if words < 40 {
        f |= SHORT;
    }
    if words > 300 {
        f |= LONG;
    }
    let first = first_sentence(text);
    if first
        .split_whitespace()
        .next()
        .is_some_and(|w| DANGLING_WORDS.contains(&clean(w).as_str()))
    {
        f |= DANGLING;
    }
    if first.split_whitespace().count() > 35 || !names_topic(heading, first) {
        f |= OFF_TOPIC;
    }
    if !has_facts(text) {
        f |= NO_FACTS;
    }
    Some(f)
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split the main content (`<main>`, else `<article>`, else the body without its header) at
/// headings and score each section. Navigation, footers, asides and forms are skipped.
pub fn quotability(body: ElementRef, out: &mut AiData) {
    let find = |name: &str| {
        body.descendants().filter_map(ElementRef::wrap).find(|e| {
            e.value().name() == name
                || (name == "main"
                    && e.value()
                        .attr("role")
                        .is_some_and(|r| r.eq_ignore_ascii_case("main")))
        })
    };
    let (root, skip_header) = match find("main").or_else(|| find("article")) {
        Some(r) => (r, false),
        None => (body, true),
    };
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut stack = vec![root.children()];
    while let Some(children) = stack.last_mut() {
        let Some(child) = children.next() else {
            stack.pop();
            if let Some((_, t)) = sections.last_mut() {
                t.push('\n');
            }
            continue;
        };
        if let Some(t) = child.value().as_text() {
            if let Some((_, s)) = sections.last_mut() {
                s.push_str(t);
                s.push(' ');
            }
        } else if let Some(el) = ElementRef::wrap(child) {
            match el.value().name() {
                "script" | "style" | "noscript" | "template" | "svg" | "iframe" | "object"
                | "nav" | "footer" | "aside" | "form" | "button" | "select" | "dialog" => {}
                "header" if skip_header => {}
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let h = norm(&el.text().collect::<Vec<_>>().join(" "));
                    sections.push((h, String::new()));
                }
                _ => {
                    if let Some((_, t)) = sections.last_mut() {
                        t.push('\n');
                    }
                    stack.push(el.children());
                }
            }
        }
    }
    for (heading, text) in &sections {
        if heading.is_empty() {
            continue;
        }
        let Some(f) = section_flags(heading, text) else {
            continue;
        };
        out.sections = out.sections.saturating_add(1);
        out.flags |= f;
        if f == 0 {
            out.quotable = out.quotable.saturating_add(1);
        } else if out.weakest.is_empty() {
            out.weakest = heading.chars().take(80).collect();
            out.weakest_flags = f;
        }
    }
}

// ------------------------------------------------------------------ presentation

/// Short text for one matrix cell.
pub fn cell(b: &Bot, r: &BotResult) -> String {
    let robots = if r.robots_allowed {
        ""
    } else {
        "robots.txt disallows · "
    };
    match r.outcome {
        Outcome::NotFetched if b.ua.is_none() => format!(
            "{}robots.txt token only",
            if r.robots_allowed {
                "Allowed · "
            } else {
                "Disallowed · "
            }
        ),
        Outcome::NotFetched if !r.robots_allowed => "Disallowed by robots.txt".into(),
        Outcome::NotFetched => "Not tested".into(),
        Outcome::Ok => format!("{robots}OK {} · {} words", r.status, r.words),
        o => {
            let mut s = format!("{robots}{}", o.label());
            if r.status > 0 && !matches!(o, Outcome::Inconclusive) {
                let _ = write!(s, " {}", r.status);
            }
            if !r.detail.is_empty() {
                let _ = write!(s, " · {}", r.detail);
            }
            if r.noindex {
                s.push_str(" · noindex for this bot");
            }
            s
        }
    }
}

/// One-line summary for the CLI.
pub fn summary(r: &AiBotReport) -> String {
    let mut can = 0;
    let mut total = 0;
    let mut blocked = Vec::new();
    for (b, row) in r.rows_for(0) {
        if b.purpose == Purpose::Training {
            continue;
        }
        total += 1;
        if AiBotReport::blocked(b, row) {
            let why = if row.outcome.refused() {
                format!("{} {}", row.outcome.label().to_lowercase(), row.status)
            } else {
                "robots.txt".into()
            };
            blocked.push(format!("{} ({why})", b.name));
        } else {
            can += 1;
        }
    }
    let mut s = format!(
        "AI crawlers: {can} of {total} search and user-triggered bots can fetch the start page"
    );
    if !blocked.is_empty() {
        let _ = write!(s, "; blocked: {}", blocked.join(", "));
    }
    let training_blocked = r
        .rows_for(0)
        .filter(|(b, row)| b.purpose == Purpose::Training && AiBotReport::blocked(b, row))
        .count();
    let training = BOTS
        .iter()
        .filter(|b| b.purpose == Purpose::Training)
        .count();
    let _ = write!(
        s,
        ". Training bots blocked: {training_blocked} of {training}"
    );
    s
}

/// Honest note on llms.txt for the GUI and report.
pub fn llms_note(r: &AiBotReport) -> String {
    let state = if r.llms_txt_ok {
        "found".to_string()
    } else if r.llms_txt_status == 0 {
        "not checked or no response".to_string()
    } else {
        format!("not found (HTTP {})", r.llms_txt_status)
    };
    format!("/llms.txt: {state}. llms.txt is a proposed convention; logs studied so far show AI crawlers rarely request it, so it is listed for information only and does not affect any finding.")
}

/// Pages with the weakest sections first: (url, data), at most `n`.
pub fn quotability_rows(crawl: &Crawl, n: usize) -> Vec<(&str, &AiData)> {
    let mut v: Vec<(&str, &AiData)> = crawl
        .pages
        .iter()
        .filter_map(|p| crate::audit::ihd(p).map(|h| (p.url.as_str(), &h.ai)))
        .filter(|(_, a)| a.sections > 0)
        .collect();
    v.sort_by(|a, b| {
        let share = |d: &AiData| d.quotable as f32 / d.sections.max(1) as f32;
        share(a.1)
            .total_cmp(&share(b.1))
            .then(b.1.sections.cmp(&a.1.sections))
            .then(a.0.cmp(b.0))
    });
    v.truncate(n);
    v
}

/// The "AI search readiness" section of the HTML report.
pub fn html_section(crawl: &Crawl) -> String {
    use crate::report::esc;
    let mut s = String::from("<h2>AI search readiness</h2>\n");
    let Some(r) = &crawl.site.ai_bots else {
        s.push_str("<p class=muted>The AI crawler access test was not run for this crawl.</p>\n");
        return s;
    };
    s.push_str("<p class=muted>Each AI crawler's robots.txt rules were evaluated, then each page was requested with the crawler's published user agent and compared with a normal request. A block seen here means the server or CDN treats that user agent differently; the real crawlers also come from known IP ranges, which some firewalls check too, so results can differ for them.</p>\n<ol class=muted>");
    for p in &r.pages {
        let _ = write!(
            s,
            "<li><a href=\"{u}\">{u}</a>: normal request HTTP {} · {} words without JavaScript{}</li>",
            p.status,
            p.words,
            if p.problem.is_empty() {
                String::new()
            } else {
                format!(" · {}", esc(&p.problem))
            },
            u = esc(&p.url)
        );
    }
    s.push_str("</ol>\n<table><thead><tr><th>Crawler</th><th>Purpose</th>");
    for i in 0..r.pages.len() {
        let _ = write!(s, "<th>Page {}</th>", i + 1);
    }
    s.push_str("</tr></thead><tbody>");
    for purpose in Purpose::ALL {
        for b in BOTS.iter().filter(|b| b.purpose == purpose) {
            let _ = write!(
                s,
                "<tr><td><strong>{}</strong> <span class=muted>{}</span></td><td>{}</td>",
                esc(b.name),
                esc(b.operator),
                purpose.label()
            );
            for pi in 0..r.pages.len() {
                match r.rows_for(pi).find(|(x, _)| x.name == b.name) {
                    Some((_, row)) => {
                        let color = if AiBotReport::blocked(b, row) {
                            "var(--issue)"
                        } else if matches!(row.outcome, Outcome::Ok) {
                            "var(--good)"
                        } else {
                            "inherit"
                        };
                        let _ =
                            write!(s, "<td style=\"color:{color}\">{}</td>", esc(&cell(b, row)));
                    }
                    None => s.push_str("<td></td>"),
                }
            }
            s.push_str("</tr>");
        }
    }
    let _ = write!(
        s,
        "</tbody></table>\n<p class=muted>{}</p>\n",
        esc(&llms_note(r))
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(status: u16, headers: &[(&str, &str)], body: &str) -> Fetched {
        Fetched {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.into(),
        }
    }

    fn page_html(words: usize) -> String {
        format!("<html><body><p>{}</p></body></html>", "word ".repeat(words))
    }

    #[test]
    fn registry_tokens_are_unique_and_robots_only_tokens_have_no_ua() {
        for (i, b) in BOTS.iter().enumerate() {
            assert!(BOTS[i + 1..]
                .iter()
                .all(|o| !o.name.eq_ignore_ascii_case(b.name)));
            if let Some(ua) = b.ua {
                assert!(
                    ua.to_ascii_lowercase()
                        .contains(&b.name.to_ascii_lowercase()),
                    "{}",
                    b.name
                );
            }
        }
        assert!(bot("google-extended").unwrap().ua.is_none());
        assert!(bot("APPLEBOT-EXTENDED").unwrap().ua.is_none());
    }

    #[test]
    fn classifies_blocks_challenges_and_thin_responses() {
        let base = baseline("https://e.com/", Ok(fetched(200, &[], &page_html(400))));
        assert_eq!(base.words, 400);
        let gpt = bot("GPTBot").unwrap();
        let c = |f| classify(&base, "https://e.com/", gpt, Ok(f));
        assert_eq!(c(fetched(200, &[], &page_html(390))).outcome, Outcome::Ok);
        assert_eq!(
            c(fetched(200, &[], &page_html(20))).outcome,
            Outcome::LessContent
        );
        let r = c(fetched(403, &[("server", "cloudflare")], "denied"));
        assert_eq!(
            (r.outcome, r.detail.as_str()),
            (Outcome::Blocked, "Cloudflare (HTTP 403)")
        );
        let r = c(fetched(
            403,
            &[("cf-mitigated", "challenge")],
            "<title>Just a moment...</title>",
        ));
        assert_eq!(r.outcome, Outcome::Challenged);
        assert_eq!(c(fetched(429, &[], "")).outcome, Outcome::RateLimited);
        assert_eq!(c(fetched(503, &[], "")).outcome, Outcome::Error);
        // a normal page that carries Cloudflare's injected script is not a challenge
        let ok = format!(
            "{}<script src=\"/cdn-cgi/challenge-platform/scripts/jsd/main.js\"></script>",
            page_html(400)
        );
        assert_eq!(c(fetched(200, &[], &ok)).outcome, Outcome::Ok);
        // bot-only noindex
        let ni = "<html><head><meta name=\"GPTBot\" content=\"noindex\"></head><body>".to_string()
            + &"word ".repeat(400);
        assert!(c(fetched(200, &[], &ni)).noindex);
        assert!(
            c(fetched(
                200,
                &[("X-Robots-Tag", "gptbot: noindex")],
                &page_html(400)
            ))
            .noindex
        );
        assert!(
            !c(fetched(
                200,
                &[("X-Robots-Tag", "ccbot: noindex")],
                &page_html(400)
            ))
            .noindex
        );
        // nothing to compare with
        let bad = baseline(
            "https://e.com/",
            Ok(fetched(403, &[("cf-mitigated", "challenge")], "")),
        );
        assert_eq!(
            classify(&bad, "https://e.com/", gpt, Ok(fetched(403, &[], ""))).outcome,
            Outcome::Inconclusive
        );
    }

    #[test]
    fn noindex_targets() {
        assert!(noindex_for("noindex, nofollow", Some("GPTBot")));
        assert!(noindex_for("GPTBot: noindex", Some("gptbot")));
        assert!(!noindex_for("GPTBot: noindex", Some("ClaudeBot")));
        assert!(!noindex_for(
            "max-snippet:-1, max-image-preview:large",
            Some("GPTBot")
        ));
        assert!(!noindex_for("googlebot: noindex", None));
    }

    #[test]
    fn section_scoring() {
        let good = "Toad food costs about 4 dollars a week for one adult toad. Feed crickets and worms three times a week, dusted with calcium powder. Young toads eat daily and need smaller insects that fit between their eyes. Fresh water in a shallow dish matters as much as the food itself.";
        assert_eq!(
            section_flags("How much does toad food cost?", good),
            Some(0)
        );
        let dangling = "It depends on many things that we will talk about below in some detail, so keep reading to learn everything there is to know about it and more.";
        let f = section_flags("How much does toad food cost?", dangling).unwrap();
        assert!(f & DANGLING != 0 && f & OFF_TOPIC != 0 && f & NO_FACTS != 0 && f & SHORT != 0);
        assert_eq!(section_flags("Menu", "Home About"), None);
        assert_eq!(
            first_sentence("Pricing is 3.5 dollars. Next one."),
            "Pricing is 3.5 dollars."
        );
    }

    #[test]
    fn quotability_splits_main_content_by_headings() {
        let html = r#"<html><body><header><h2>Site menu</h2><p>skip me entirely please because it is the header of the page</p></header>
            <main><h1>Toad care</h1><p>Toad care takes about 10 minutes a day for a healthy adult toad living indoors in a glass tank with moss and water. Clean the water daily, mist the moss, and check the tank temperature stays near 22 degrees in summer and winter.</p>
            <h2>Feeding</h2><p>This is something we cover later in the article, and it is a big topic for every new owner who wants to learn.</p>
            <nav><h2>Related</h2><a href="/a">Other pages that we wrote about toads and frogs and newts and more</a></nav></main></body></html>"#;
        let d = crate::parse::parse_html(html, &url::Url::parse("https://e.com/").unwrap());
        assert_eq!((d.ai.sections, d.ai.quotable), (2, 1), "{:?}", d.ai);
        assert_eq!(d.ai.weakest, "Feeding");
        assert!(d.ai.weakest_flags & DANGLING != 0);
    }

    #[test]
    fn sampler_picks_home_then_most_linked_indexable() {
        let s = Sampler::default();
        let page = |id, url: &str, depth, links: &[&str]| Page {
            id,
            url: url.into(),
            depth,
            is_internal: true,
            status: 200,
            html: Some(Box::new(HtmlData {
                links: links
                    .iter()
                    .map(|l| crate::model::Link {
                        url: l.to_string(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let (h, a, b, c) = (
            "https://e.com/",
            "https://e.com/a",
            "https://e.com/b",
            "https://e.com/c",
        );
        s.note(&page(1, h, 0, &[a, b, c, "https://other.com/x"]));
        s.note(&page(2, a, 1, &[b, b, h]));
        s.note(&page(3, b, 1, &[c]));
        let mut c_page = page(4, c, 1, &[b]);
        c_page.status = 404;
        s.note(&c_page);
        let seen: Vec<String> = [h, a, b, c].iter().map(|s| s.to_string()).collect();
        assert_eq!(s.pick(seen.iter(), 5), vec![h, b, a]);
        assert_eq!(s.pick(seen.iter(), 2), vec![h, b]);
    }
}
