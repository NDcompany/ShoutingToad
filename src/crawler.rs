//! Multi-threaded spider. Fetches URLs, parses HTML and streams [`Page`]s out.

use crate::model::{Page, RobotsInfo, SiteInfo, SitemapInfo, Source};
use crate::parse::{decode_body, parse_html};
use crate::robots::Robots;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

pub const DEFAULT_UA: &str = concat!(
    "Mozilla/5.0 (compatible; ShoutingToad/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/NDcompany/ShoutingToad)"
);

const HTML_LIMIT: usize = 15 * 1024 * 1024;
const RESOURCE_LIMIT: usize = 25 * 1024 * 1024;
const MAX_SITEMAPS: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_depth: usize,
    pub max_urls: usize,
    pub threads: usize,
    pub delay_ms: u64,
    pub timeout_secs: u64,
    pub user_agent: String,
    pub respect_robots: bool,
    pub crawl_subdomains: bool,
    pub check_external: bool,
    pub check_resources: bool,
    pub follow_nofollow: bool,
    pub crawl_sitemaps: bool,
    pub include: String,
    pub exclude: String,
    pub custom_search: String,
    pub custom_extract: String,
    /// Render internal HTML pages with the installed Chrome/Edge (see [`crate::render`]).
    pub render: bool,
    /// Browser executable; empty = detect.
    pub browser_path: String,
    /// Browser tabs rendering at once (never more than `threads`).
    pub render_tabs: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_depth: 10,
            max_urls: 100_000,
            threads: 6,
            delay_ms: 0,
            timeout_secs: 20,
            user_agent: DEFAULT_UA.to_string(),
            respect_robots: true,
            crawl_subdomains: false,
            check_external: true,
            check_resources: true,
            follow_nofollow: false,
            crawl_sitemaps: true,
            include: String::new(),
            exclude: String::new(),
            custom_search: String::new(),
            custom_extract: String::new(),
            render: false,
            browser_path: String::new(),
            render_tabs: 4,
        }
    }
}

impl Config {
    /// Lower-case product token used to pick a robots.txt group.
    pub fn robots_agent(&self) -> String {
        let ua = self.user_agent.to_ascii_lowercase();
        for bot in ["googlebot", "bingbot", "shoutingtoad"] {
            if ua.contains(bot) {
                return bot.to_string();
            }
        }
        ua.split(['/', ' ', ';', '('])
            .find(|s| !s.is_empty())
            .unwrap_or("*")
            .to_string()
    }
}

pub enum Event {
    Page(Box<Page>),
    Progress {
        crawled: usize,
        queued: usize,
        active: usize,
    },
    Site(Box<SiteInfo>),
    Log(String),
    Finished,
}

pub struct Handle {
    pub rx: Receiver<Event>,
    pub stop: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

pub fn normalize_url(raw: &str) -> Result<Url, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("Enter a URL to crawl".into());
    }
    let full = if t.contains("://") {
        t.to_string()
    } else {
        format!("https://{t}")
    };
    let mut u = Url::parse(&full).map_err(|e| format!("'{raw}' is not a valid URL: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(format!("'{raw}' must be an http or https URL"));
    }
    if u.host_str().is_none() {
        return Err(format!("'{raw}' has no host name"));
    }
    u.set_fragment(None);
    Ok(u)
}

struct Job {
    url: String,
    depth: usize,
    source: Source,
}

struct Queue {
    jobs: VecDeque<Job>,
    seen: HashSet<String>,
    /// robots.txt and sitemap files in `seen`; they do not count against max_urls.
    meta: usize,
    active: usize,
}

impl Queue {
    fn full(&self, max: usize) -> bool {
        self.seen.len() - self.meta >= max
    }
}

struct Shared {
    cfg: Config,
    list_mode: bool,
    client: reqwest::blocking::Client,
    queue: Mutex<Queue>,
    cv: Condvar,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    next_slot: Mutex<Instant>,
    delay: Duration,
    ids: AtomicUsize,
    crawled: AtomicUsize,
    robots: Mutex<HashMap<String, Option<Arc<Robots>>>>,
    /// Grows when the start URL redirects to its www / non-www twin.
    internal_hosts: std::sync::RwLock<Vec<String>>,
    root_domain: String,
    include: Option<Regex>,
    exclude: Option<Regex>,
    custom_search: Option<Regex>,
    custom_extract: Option<Regex>,
    renderer: std::sync::OnceLock<crate::render::Pool>,
    tx: Sender<Event>,
}

fn opt_regex(pattern: &str, what: &str) -> Result<Option<Regex>, String> {
    if pattern.trim().is_empty() {
        return Ok(None);
    }
    Regex::new(pattern.trim())
        .map(Some)
        .map_err(|e| format!("{what} pattern is not a valid regex: {e}"))
}

/// Start a crawl. `list_mode` audits exactly the given URLs without following links.
pub fn start(seeds: Vec<String>, list_mode: bool, cfg: Config) -> Result<Handle, String> {
    let mut urls = Vec::new();
    for s in seeds.iter().filter(|s| !s.trim().is_empty()) {
        urls.push(normalize_url(s)?);
    }
    if urls.is_empty() {
        return Err("Enter at least one URL to crawl".into());
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(cfg.user_agent.clone())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .connect_timeout(Duration::from_secs(cfg.timeout_secs.clamp(1, 15)))
        .pool_max_idle_per_host(cfg.threads.max(1))
        .build()
        .map_err(|e| format!("Could not create HTTP client: {e}"))?;

    let mut internal_hosts: Vec<String> = urls
        .iter()
        .filter_map(|u| u.host_str().map(|h| h.to_string()))
        .collect();
    internal_hosts.sort();
    internal_hosts.dedup();
    let root_domain = internal_hosts[0].trim_start_matches("www.").to_string();

    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let pause = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        include: opt_regex(&cfg.include, "Include")?,
        exclude: opt_regex(&cfg.exclude, "Exclude")?,
        custom_search: opt_regex(&cfg.custom_search, "Custom search")?,
        custom_extract: opt_regex(&cfg.custom_extract, "Custom extraction")?,
        delay: Duration::from_millis(cfg.delay_ms),
        cfg,
        list_mode,
        client,
        queue: Mutex::new(Queue {
            jobs: VecDeque::new(),
            seen: HashSet::new(),
            meta: 0,
            active: 0,
        }),
        cv: Condvar::new(),
        stop: stop.clone(),
        pause: pause.clone(),
        next_slot: Mutex::new(Instant::now()),
        ids: AtomicUsize::new(1),
        crawled: AtomicUsize::new(0),
        robots: Mutex::new(HashMap::new()),
        internal_hosts: std::sync::RwLock::new(internal_hosts),
        root_domain,
        renderer: std::sync::OnceLock::new(),
        tx,
    });

    thread::spawn(move || run(shared, urls));
    Ok(Handle { rx, stop, pause })
}

/// Closes the browser however the crawl ends (finished, stopped or a panic).
struct CloseBrowser(Arc<Shared>);

impl Drop for CloseBrowser {
    fn drop(&mut self) {
        if let Some(p) = self.0.renderer.get() {
            p.shutdown();
        }
    }
}

fn run(sh: Arc<Shared>, seeds: Vec<Url>) {
    let _close = CloseBrowser(sh.clone());
    // robots.txt + sitemaps for the primary host, before any page is fetched
    let launch = sh.cfg.render.then(|| {
        let sh = sh.clone();
        thread::spawn(move || {
            let c = &sh.cfg;
            let tabs = c.render_tabs.clamp(1, c.threads.max(1));
            match crate::render::Pool::launch(
                &c.browser_path,
                tabs,
                &c.user_agent,
                c.timeout_secs,
                &sh.stop,
            ) {
                Ok(p) => {
                    let _ = sh.tx.send(Event::Log(format!(
                        "Rendering JavaScript with {} ({tabs} tabs)",
                        p.browser
                    )));
                    let _ = sh.renderer.set(p);
                    if sh.stop.load(Ordering::Relaxed) {
                        if let Some(p) = sh.renderer.get() {
                            p.shutdown();
                        }
                    }
                }
                Err(e) => {
                    let _ = sh.tx.send(Event::Log(e));
                }
            }
        })
    });
    let setup = {
        let (sh, start) = (sh.clone(), seeds[0].clone());
        thread::spawn(move || site_setup(&sh, &start))
    };
    let setup = if wait_or_stop(&sh, std::slice::from_ref(&setup)) {
        setup.join().ok()
    } else {
        None
    };
    let launched = launch.is_none_or(|l| wait_or_stop(&sh, std::slice::from_ref(&l)));
    let Some((site, sitemap_urls)) = setup.filter(|_| launched) else {
        let _ = sh.tx.send(Event::Finished);
        return;
    };
    let _ = sh.tx.send(Event::Site(Box::new(site)));

    {
        let mut q = sh.queue.lock().unwrap();
        for u in &seeds {
            if q.seen.insert(u.to_string()) {
                q.jobs.push_back(Job {
                    url: u.to_string(),
                    depth: 0,
                    source: Source::Seed,
                });
            }
        }
    }
    run_workers(&sh);

    // Phase 2: sitemap URLs the link crawl never reached (orphan candidates).
    if sh.cfg.crawl_sitemaps && !sh.stop.load(Ordering::Relaxed) {
        let mut q = sh.queue.lock().unwrap();
        for u in &sitemap_urls {
            if q.full(sh.cfg.max_urls) {
                break;
            }
            if is_internal(&sh, u) && wanted(&sh, u) && q.seen.insert(u.clone()) {
                q.jobs.push_back(Job {
                    url: u.clone(),
                    depth: 1,
                    source: Source::Sitemap,
                });
            }
        }
        drop(q);
        run_workers(&sh);
    }
    drop(_close);
    let _ = sh.tx.send(Event::Finished);
}

fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    }
}

/// Follow the start URL's redirects. When it lands on the same site (http -> https,
/// bare -> www or the reverse), that host is internal too and site-wide checks
/// (robots.txt, sitemaps, host variants) run against it.
fn resolve_start(sh: &Shared, start: &Url) -> Option<Url> {
    let bare = |u: &Url| {
        u.host_str()
            .unwrap_or("")
            .trim_start_matches("www.")
            .to_string()
    };
    let mut cur = start.clone();
    for _ in 0..5 {
        let r = fetch(sh, cur.as_str(), false, 64 * 1024).ok()?;
        if !(300..400).contains(&r.status) {
            break;
        }
        let next = cur.join(header(&r.headers, "location")?.trim()).ok()?;
        if bare(&next) != bare(start) {
            return None;
        }
        cur = next;
    }
    if cur == *start {
        return None;
    }
    if let Some(h) = cur.host_str() {
        let mut hosts = sh.internal_hosts.write().unwrap();
        if !hosts.iter().any(|x| x == h) {
            hosts.push(h.to_string());
        }
    }
    Some(cur)
}

fn site_setup(sh: &Shared, start: &Url) -> (SiteInfo, Vec<String>) {
    let mut site = SiteInfo::default();
    // Follow a start-URL redirect to its www/non-www twin, but only if robots.txt lets us fetch it.
    let mut start = start.clone();
    let start_allowed = !sh.cfg.respect_robots
        || robots_for(sh, &start)
            .is_none_or(|r| r.is_allowed(&path_and_query(&start), &sh.cfg.robots_agent()));
    if !sh.list_mode && start_allowed {
        if let Some(fin) = resolve_start(sh, &start) {
            start = fin;
        }
    }
    let start = &start;
    let robots = fetch_robots(sh, start, true, &mut site);
    let mut sitemap_urls: Vec<String> = Vec::new();
    if !sh.list_mode {
        sitemap_urls = fetch_sitemaps(sh, start, robots.as_deref(), &mut site);
        site.sitemaps_checked = true;
        if !sh.stop.load(Ordering::Relaxed) {
            check_host_variants(sh, start, &mut site);
        }
    }
    site.sitemap_urls = sitemap_urls.clone();
    (site, sitemap_urls)
}

/// Wait for `handles`, but return false as soon as the crawl is stopped: a thread can sit in a
/// request to a slow server until the timeout. It exits after that request; its result is dropped.
fn wait_or_stop<T>(sh: &Shared, handles: &[thread::JoinHandle<T>]) -> bool {
    while !handles.iter().all(|h| h.is_finished()) {
        if sh.stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    true
}

fn run_workers(sh: &Arc<Shared>) {
    let handles: Vec<_> = (0..sh.cfg.threads.clamp(1, 64))
        .map(|_| {
            let sh = sh.clone();
            thread::spawn(move || worker(sh))
        })
        .collect();
    wait_or_stop(sh, &handles);
}

fn worker(sh: Arc<Shared>) {
    loop {
        while sh.pause.load(Ordering::Relaxed) && !sh.stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(150));
        }
        let job = {
            let mut q = sh.queue.lock().unwrap();
            loop {
                if sh.stop.load(Ordering::Relaxed) {
                    break None;
                }
                if let Some(j) = q.jobs.pop_front() {
                    q.active += 1;
                    break Some(j);
                }
                if q.active == 0 {
                    break None;
                }
                q = sh.cv.wait_timeout(q, Duration::from_millis(200)).unwrap().0;
            }
        };
        let Some(job) = job else {
            sh.cv.notify_all();
            return;
        };

        // A panic on hostile input must not kill the worker: it would leave `active` raised and
        // the crawl waiting forever.
        let (page, discovered) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(&sh, &job)))
                .unwrap_or_else(|_| {
                    let mut page = base_page(
                        &sh,
                        &job.url,
                        job.depth,
                        job.source,
                        is_internal(&sh, &job.url),
                    );
                    page.error = Some("Internal error while processing this URL".into());
                    page.status_text = "Internal error while processing this URL".into();
                    (page, Vec::new())
                });
        let crawled = sh.crawled.fetch_add(1, Ordering::Relaxed) + 1;
        let (queued, active) = {
            let mut q = sh.queue.lock().unwrap();
            for d in discovered {
                if q.full(sh.cfg.max_urls) {
                    break;
                }
                if q.seen.insert(d.url.clone()) {
                    q.jobs.push_back(d);
                }
            }
            q.active -= 1;
            (q.jobs.len(), q.active)
        };
        sh.cv.notify_all();
        let _ = sh.tx.send(Event::Page(Box::new(page)));
        let _ = sh.tx.send(Event::Progress {
            crawled,
            queued,
            active,
        });
    }
}

fn is_internal(sh: &Shared, url: &str) -> bool {
    let Some(host) = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
    else {
        return false;
    };
    if sh
        .internal_hosts
        .read()
        .unwrap()
        .iter()
        .any(|h| same_site(h, &host))
    {
        return true;
    }
    sh.cfg.crawl_subdomains
        && (host == sh.root_domain || host.ends_with(&format!(".{}", sh.root_domain)))
}

/// www and non-www are one site, as the checks treat them, so a start URL that
/// redirects to its www (or bare) host keeps crawling.
fn same_site(a: &str, b: &str) -> bool {
    a.trim_start_matches("www.") == b.trim_start_matches("www.")
}

/// Internal URL passes the include and exclude patterns.
fn wanted(sh: &Shared, u: &str) -> bool {
    sh.include.as_ref().is_none_or(|re| re.is_match(u))
        && sh.exclude.as_ref().is_none_or(|re| !re.is_match(u))
}

fn throttle(sh: &Shared) {
    let wait = {
        let mut next = sh.next_slot.lock().unwrap();
        let now = Instant::now();
        let slot = (*next).max(now);
        *next = slot + sh.delay;
        slot - now
    };
    thread::sleep(wait);
}

struct Response {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    version: String,
    elapsed_ms: u64,
    body: Vec<u8>,
    transfer: usize,
    encoding: String,
}

const ACCEPT_HTML: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
/// Browser image Accept header: CDNs (Shopify, Cloudflare, imgix…) negotiate
/// WebP/AVIF from it, so sizes and formats match what visitors receive.
const ACCEPT_IMAGE: &str = "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8";

fn fetch(sh: &Shared, url: &str, head: bool, limit: usize) -> Result<Response, String> {
    fetch_as(sh, url, head, limit, ACCEPT_HTML)
}

fn fetch_as(
    sh: &Shared,
    url: &str,
    head: bool,
    limit: usize,
    accept: &str,
) -> Result<Response, String> {
    // 429 means this crawl is being rate limited: back off (honouring Retry-After) and retry.
    for _ in 0..2 {
        let r = fetch_once(sh, url, head, limit, accept)?;
        if r.status != 429 || sh.stop.load(Ordering::Relaxed) {
            return Ok(r);
        }
        let secs = header(&r.headers, "retry-after")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(5)
            .clamp(1, 30);
        let mut next = sh.next_slot.lock().unwrap();
        *next = (*next).max(Instant::now() + Duration::from_secs(secs));
    }
    fetch_once(sh, url, head, limit, accept)
}

fn fetch_once(
    sh: &Shared,
    url: &str,
    head: bool,
    limit: usize,
    accept: &str,
) -> Result<Response, String> {
    throttle(sh);
    let t0 = Instant::now();
    let req = if head {
        sh.client.head(url)
    } else {
        sh.client.get(url)
    };
    let resp = req
        .header("Accept", accept)
        .header("Accept-Encoding", "gzip, deflate, br")
        .send()
        .map_err(describe_error)?;
    let status = resp.status();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let version = format!("{:?}", resp.version());
    let encoding = resp
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mut raw = Vec::new();
    if !head {
        // reqwest's blocking timeout is per read, so a server dripping a byte every few seconds
        // would hold this worker forever: enforce the timeout over the whole body, and stop.
        let deadline = t0 + Duration::from_secs(sh.cfg.timeout_secs.max(1));
        let mut body = resp.take(limit as u64);
        let mut buf = [0u8; 16 * 1024];
        loop {
            if sh.stop.load(Ordering::Relaxed) {
                return Err("Stopped".into());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "Timeout: the body took longer than {}s to download",
                    sh.cfg.timeout_secs.max(1)
                ));
            }
            match body.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
        }
    }
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    let transfer = raw.len();
    let body = decompress(&encoding, raw, limit);
    Ok(Response {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        headers,
        version,
        elapsed_ms,
        body,
        transfer,
        encoding,
    })
}

fn decompress(encoding: &str, raw: Vec<u8>, limit: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let ok = match encoding {
        "gzip" | "x-gzip" => flate2::read::GzDecoder::new(&raw[..])
            .take(limit as u64)
            .read_to_end(&mut out)
            .is_ok(),
        "deflate" => {
            flate2::read::ZlibDecoder::new(&raw[..])
                .take(limit as u64)
                .read_to_end(&mut out)
                .is_ok()
                || {
                    out.clear();
                    flate2::read::DeflateDecoder::new(&raw[..])
                        .take(limit as u64)
                        .read_to_end(&mut out)
                        .is_ok()
                }
        }
        "br" => brotli_decompressor::Decompressor::new(&raw[..], 4096)
            .take(limit as u64)
            .read_to_end(&mut out)
            .is_ok(),
        _ => return raw,
    };
    if ok {
        out
    } else {
        raw
    }
}

fn describe_error(e: reqwest::Error) -> String {
    use std::error::Error;
    // rustls reports handshake failures as io::ErrorKind::InvalidData inside the connect error.
    // The io errors nest through get_ref(), not source().
    let tls = std::iter::successors(e.source(), |&s| s.source()).any(|s| {
        let mut io = s.downcast_ref::<std::io::Error>();
        while let Some(i) = io {
            if i.kind() == std::io::ErrorKind::InvalidData {
                return true;
            }
            io = i.get_ref().and_then(|r| r.downcast_ref::<std::io::Error>());
        }
        false
    });
    let mut msg = if e.is_timeout() {
        "Timeout".to_string()
    } else if e.is_connect() && tls {
        "SSL/TLS error".to_string()
    } else if e.is_connect() {
        "Connection failed".to_string()
    } else {
        "Request failed".to_string()
    };
    let mut src = e.source();
    let mut detail = String::new();
    while let Some(s) = src {
        detail = s.to_string();
        src = s.source();
    }
    if !detail.is_empty() {
        msg.push_str(": ");
        msg.push_str(&detail);
    }
    msg
}

fn robots_for(sh: &Shared, url: &Url) -> Option<Arc<Robots>> {
    let key = url.origin().ascii_serialization(); // keeps a non-default port
    if let Some(r) = sh.robots.lock().unwrap().get(&key) {
        return r.clone();
    }
    let mut ignored = SiteInfo::default();
    fetch_robots(sh, url, false, &mut ignored)
}

/// Fetch robots.txt for the URL's host, emit it as a page, and cache the rules.
fn fetch_robots(sh: &Shared, url: &Url, primary: bool, site: &mut SiteInfo) -> Option<Arc<Robots>> {
    let key = url.origin().ascii_serialization(); // keeps a non-default port
    let robots_url = format!("{key}/robots.txt");
    let mut current = robots_url.clone();
    let mut redirected = false;
    let mut result = None;
    for _ in 0..5 {
        match fetch(sh, &current, false, 1024 * 1024) {
            Ok(r) if (300..400).contains(&r.status) => {
                let loc = header(&r.headers, "location")
                    .and_then(|l| Url::parse(&current).ok()?.join(&l).ok());
                match loc {
                    Some(l) => {
                        redirected = true;
                        current = l.to_string();
                    }
                    None => {
                        result = Some(r);
                        break;
                    }
                }
            }
            Ok(r) => {
                result = Some(r);
                break;
            }
            Err(e) => {
                let _ = sh.tx.send(Event::Log(format!("robots.txt: {e}")));
                break;
            }
        }
    }
    let rules = result
        .as_ref()
        .filter(|r| r.status == 200)
        .map(|r| Arc::new(Robots::parse(&String::from_utf8_lossy(&r.body))));
    if primary {
        let agent = sh.cfg.robots_agent();
        site.robots = Some(RobotsInfo {
            url: robots_url.clone(),
            status: result.as_ref().map(|r| r.status).unwrap_or(0),
            size: result.as_ref().map(|r| r.body.len()).unwrap_or(0),
            redirected,
            disallow_all: rules.as_ref().is_some_and(|r| r.disallows_all(&agent)),
            crawl_delay: rules.as_ref().and_then(|r| r.crawl_delay(&agent)),
            sitemap_directives: rules
                .as_ref()
                .map(|r| r.sitemaps.clone())
                .unwrap_or_default(),
            invalid_lines: rules.as_ref().map(|r| r.invalid_lines).unwrap_or(0),
            noindex_lines: rules.as_ref().map(|r| r.noindex_lines).unwrap_or(0),
        });
        if let Some(r) = &result {
            let page = resource_page(sh, &robots_url, 0, Source::Robots, true, r);
            let mut q = sh.queue.lock().unwrap();
            if q.seen.insert(robots_url.clone()) {
                q.meta += 1;
            }
            let _ = sh.tx.send(Event::Page(Box::new(page)));
        }
    }
    sh.robots.lock().unwrap().insert(key, rules.clone());
    rules
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn valid_lastmod(s: &str) -> bool {
    let s = s.trim();
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
        || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
        || chrono::NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d").is_ok()
        || (s.len() == 4 && s.parse::<u32>().is_ok())
        // W3C datetime allows minute precision with a Z designator (2026-11-01T10:00Z).
        || chrono::DateTime::parse_from_str(&s.replace('Z', "+00:00"), "%Y-%m-%dT%H:%M%:z").is_ok()
}

fn fetch_sitemaps(
    sh: &Shared,
    start: &Url,
    robots: Option<&Robots>,
    site: &mut SiteInfo,
) -> Vec<String> {
    let base = start.origin().ascii_serialization();
    let declared: Vec<String> = robots.map(|r| r.sitemaps.clone()).unwrap_or_default();
    let mut todo: VecDeque<(String, bool)> = declared.iter().map(|s| (s.clone(), true)).collect();
    let default = format!("{base}/sitemap.xml");
    // Only guess /sitemap.xml when robots.txt declares nothing.
    if declared.is_empty() {
        todo.push_back((default.clone(), false));
    }
    let mut done: HashSet<String> = HashSet::new();
    let mut guessed: HashSet<String> = HashSet::from([default.clone()]);
    let mut urls: Vec<String> = Vec::new();
    let mut url_set: HashSet<String> = HashSet::new();
    while let Some((sm_url, in_robots)) = todo.pop_front() {
        if done.contains(&sm_url) || sh.stop.load(Ordering::Relaxed) {
            continue;
        }
        // A sitemap index can list hundreds of files. Once the crawl budget is covered,
        // stop downloading them; checks that need the full list are skipped.
        if done.len() >= MAX_SITEMAPS || urls.len() >= sh.cfg.max_urls {
            site.sitemaps_partial = true;
            continue;
        }
        done.insert(sm_url.clone());
        let resp = match fetch(sh, &sm_url, false, 60 * 1024 * 1024) {
            Ok(r) => r,
            Err(e) => {
                let _ = sh.tx.send(Event::Log(format!("Sitemap {sm_url}: {e}")));
                continue;
            }
        };
        // Probing /sitemap.xml is a guess; a 404 there is not a finding.
        if !in_robots && guessed.contains(&sm_url) && matches!(resp.status, 404 | 410) {
            continue;
        }
        let page = resource_page(sh, &sm_url, 0, Source::Sitemap, true, &resp);
        {
            let mut q = sh.queue.lock().unwrap();
            if q.seen.insert(sm_url.clone()) {
                q.meta += 1;
            }
        }
        let _ = sh.tx.send(Event::Page(Box::new(page)));
        // Search engines follow sitemap redirects (mozilla.org: /sitemap.xml -> /en-US/sitemap.xml).
        if (300..400).contains(&resp.status) {
            if let Some(next) = header(&resp.headers, "location")
                .and_then(|l| Url::parse(&sm_url).ok()?.join(l.trim()).ok())
            {
                if guessed.contains(&sm_url) {
                    guessed.insert(next.to_string());
                }
                todo.push_front((next.to_string(), in_robots));
                continue;
            }
        }
        let mut info = SitemapInfo {
            url: sm_url.clone(),
            status: resp.status,
            in_robots,
            size: resp.body.len(),
            ..Default::default()
        };
        if resp.status == 200 {
            let mut body = resp.body.clone();
            if body.starts_with(&[0x1f, 0x8b]) {
                let mut out = Vec::new();
                if flate2::read::GzDecoder::new(&body[..])
                    .read_to_end(&mut out)
                    .is_ok()
                {
                    body = out;
                }
            }
            let text = String::from_utf8_lossy(&body);
            let mut locs: Vec<String> = Vec::new();
            if let Ok(doc) = roxmltree::Document::parse(&text) {
                info.valid_xml = true;
                let root = doc.root_element();
                let child_text = |n: roxmltree::Node, name: &str| {
                    n.children()
                        .find(|c| c.tag_name().name() == name)
                        .and_then(|c| c.text())
                        .map(|t| t.trim().to_string())
                };
                match root.tag_name().name() {
                    // Google also accepts RSS 2.0 and Atom feeds as sitemaps.
                    "feed" => {
                        for entry in root.children().filter(|n| n.tag_name().name() == "entry") {
                            let link = entry
                                .children()
                                .filter(|c| c.tag_name().name() == "link")
                                .find(|c| c.attribute("rel").is_none_or(|r| r == "alternate"))
                                .and_then(|c| c.attribute("href"));
                            if let Some(l) = link {
                                locs.push(l.trim().to_string());
                            }
                        }
                    }
                    "rss" => {
                        for item in root.descendants().filter(|n| n.tag_name().name() == "item") {
                            if let Some(l) = child_text(item, "link") {
                                locs.push(l);
                            }
                        }
                    }
                    name => {
                        info.is_index = name == "sitemapindex";
                        for entry in root.children().filter(|n| n.is_element()) {
                            let Some(loc) = child_text(entry, "loc") else {
                                continue;
                            };
                            if let Some(lm) = child_text(entry, "lastmod") {
                                if !valid_lastmod(&lm) {
                                    info.invalid_lastmod += 1;
                                }
                            }
                            locs.push(loc);
                        }
                    }
                }
            } else if !text.trim_start().starts_with('<') {
                // Plain-text sitemap: one URL per line.
                info.valid_xml = true;
                locs = text
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| l.starts_with("http"))
                    .collect();
            }
            for loc in locs {
                info.url_count += 1;
                if info.is_index {
                    todo.push_back((loc, in_robots));
                } else if let Ok(u) = normalize_url(&loc) {
                    if u.host_str() != start.host_str() {
                        info.other_host_urls += 1;
                    }
                    let s = u.to_string();
                    if url_set.insert(s.clone()) {
                        urls.push(s);
                    }
                }
            }
        }
        site.sitemaps.push(info);
    }
    urls
}

fn check_host_variants(sh: &Shared, start: &Url, site: &mut SiteInfo) {
    let host = start.host_str().unwrap_or("").to_string();
    if start.scheme() == "https" {
        let http = format!("http://{host}/");
        if let Ok(r) = fetch(sh, &http, false, 64 * 1024) {
            site.http_redirects_to_https = Some(
                (300..400).contains(&r.status)
                    && header(&r.headers, "location").is_some_and(|l| l.starts_with("https://")),
            );
        }
    }
    if host.parse::<std::net::IpAddr>().is_ok() || host == "localhost" {
        return;
    }
    let alt = match host.strip_prefix("www.") {
        Some(bare) => bare.to_string(),
        None => format!("www.{host}"),
    };
    let alt_url = format!("{}://{alt}/", start.scheme());
    if let Ok(r) = fetch(sh, &alt_url, false, 64 * 1024) {
        site.alt_host_redirects = Some(
            (300..400).contains(&r.status)
                && header(&r.headers, "location").is_some_and(|l| l.contains(&host)),
        );
    }
}

fn base_page(sh: &Shared, url: &str, depth: usize, source: Source, internal: bool) -> Page {
    Page {
        id: sh.ids.fetch_add(1, Ordering::Relaxed),
        url: url.to_string(),
        source: Some(source),
        is_internal: internal,
        depth,
        ..Default::default()
    }
}

fn fill_response(page: &mut Page, r: &Response, url: &Url) {
    page.status = r.status;
    page.status_text = r.reason.clone();
    page.headers = r.headers.clone();
    page.http_version = r.version.clone();
    page.response_ms = r.elapsed_ms;
    page.transfer_size = r.transfer;
    page.size = r.body.len();
    page.content_encoding = r.encoding.clone();
    page.content_type = header(&r.headers, "content-type").unwrap_or_default();
    if (300..400).contains(&r.status) {
        page.redirect_to = header(&r.headers, "location")
            .and_then(|l| url.join(l.trim()).ok())
            .map(|mut u| {
                u.set_fragment(None);
                u.to_string()
            });
    }
    if let Some(len) =
        header(&r.headers, "content-length").and_then(|v| v.trim().parse::<usize>().ok())
    {
        if page.size == 0 {
            page.size = len;
            page.transfer_size = len;
        }
    }
}

fn resource_page(
    sh: &Shared,
    url: &str,
    depth: usize,
    source: Source,
    internal: bool,
    r: &Response,
) -> Page {
    let mut p = base_page(sh, url, depth, source, internal);
    if let Ok(u) = Url::parse(url) {
        fill_response(&mut p, r, &u);
    }
    p
}

fn process(sh: &Shared, job: &Job) -> (Page, Vec<Job>) {
    let internal = is_internal(sh, &job.url);
    let mut page = base_page(sh, &job.url, job.depth, job.source, internal);
    let mut found = Vec::new();
    let Ok(url) = Url::parse(&job.url) else {
        page.error = Some("Invalid URL".into());
        return (page, found);
    };

    if internal && sh.cfg.respect_robots {
        if let Some(rules) = robots_for(sh, &url) {
            if !rules.is_allowed(&path_and_query(&url), &sh.cfg.robots_agent()) {
                page.blocked_by_robots = true;
                page.status_text = "Blocked by robots.txt".into();
                return (page, found);
            }
        }
    }

    let is_resource = matches!(
        job.source,
        Source::Image | Source::Script | Source::Stylesheet
    );
    let full_fetch = internal && !is_resource;
    let result = if full_fetch {
        fetch(sh, &job.url, false, HTML_LIMIT)
    } else {
        // Status-only check. Some servers reject HEAD, so fall back to GET.
        // Some servers answer HEAD wrongly (404/405/403), so confirm any error with GET.
        let limit = if is_resource {
            RESOURCE_LIMIT
        } else {
            256 * 1024
        };
        let accept = match job.source {
            Source::Image => ACCEPT_IMAGE,
            Source::Stylesheet => "text/css,*/*;q=0.1",
            Source::Script => "*/*",
            _ => ACCEPT_HTML,
        };
        match fetch_as(sh, &job.url, true, 0, accept) {
            Ok(r)
                if r.status >= 400
                    || (r.status == 200
                        && is_resource
                        && header(&r.headers, "content-length").is_none()) =>
            {
                fetch_as(sh, &job.url, false, limit, accept)
            }
            Err(_) => fetch_as(sh, &job.url, false, limit, accept),
            other => other,
        }
    };
    let resp = match result {
        Ok(r) => r,
        Err(e) => {
            page.error = Some(e.clone());
            page.status_text = e;
            return (page, found);
        }
    };
    fill_response(&mut page, &resp, &url);

    let next_depth = job.depth + 1;
    if let Some(loc) = &page.redirect_to {
        let target_internal = is_internal(sh, loc);
        if target_internal || sh.cfg.check_external {
            found.push(Job {
                url: loc.clone(),
                depth: job.depth,
                source: Source::Redirect,
            });
        }
    }

    let is_html = page.content_type.to_ascii_lowercase().contains("html")
        || (page.content_type.is_empty() && resp.body.trim_ascii_start().starts_with(b"<"));
    if !full_fetch || resp.status != 200 && !(400..600).contains(&resp.status) || !is_html {
        return (page, found);
    }

    let text = decode_body(&resp.body, &page.content_type);
    if let Some(re) = &sh.custom_search {
        page.custom_search_hits = re.find_iter(&text).count();
    }
    if let Some(re) = &sh.custom_extract {
        page.custom_extract = re
            .captures_iter(&text)
            .filter_map(|c| c.get(1).or(c.get(0)).map(|m| m.as_str().trim().to_string()))
            .take(10)
            .collect();
    }
    let data = parse_html(&text, &url);

    // Error pages are parsed for reporting but their links are not followed.
    if resp.status == 200 {
        let page_nofollow = {
            page.html = Some(Box::new(data));
            page.is_nofollow_page()
        };
        // JavaScript rendering: links that only exist after rendering are followed too.
        let js_links = match sh.renderer.get() {
            Some(pool) if !sh.stop.load(Ordering::Relaxed) => {
                throttle(sh);
                let (r, links) = pool.render(&job.url, page.html.as_deref().unwrap(), &sh.stop);
                page.rendered = Some(Box::new(r));
                links
            }
            _ => Vec::new(),
        };
        let d = page.html.as_ref().unwrap();
        let follow_links = !sh.list_mode
            && next_depth <= sh.cfg.max_depth
            && (!page_nofollow || sh.cfg.follow_nofollow);
        for l in d.links.iter().chain(&js_links) {
            if l.nofollow && !sh.cfg.follow_nofollow {
                continue;
            }
            if is_internal(sh, &l.url) {
                if follow_links && wanted(sh, &l.url) {
                    found.push(Job {
                        url: l.url.clone(),
                        depth: next_depth,
                        source: Source::Link,
                    });
                }
            } else if sh.cfg.check_external {
                found.push(Job {
                    url: l.url.clone(),
                    depth: next_depth,
                    source: Source::Link,
                });
            }
        }
        if sh.cfg.check_resources {
            let res = d
                .images
                .iter()
                .map(|i| (&i.url, Source::Image))
                .chain(d.scripts.iter().map(|s| (&s.url, Source::Script)))
                .chain(d.stylesheets.iter().map(|s| (s, Source::Stylesheet)));
            for (u, src) in res {
                if is_internal(sh, u) || sh.cfg.check_external {
                    found.push(Job {
                        url: u.clone(),
                        depth: next_depth,
                        source: src,
                    });
                }
            }
        }
        if !sh.list_mode {
            let related = d
                .canonicals
                .iter()
                .map(|u| (u, Source::Canonical))
                .chain(d.hreflang.iter().map(|(_, u)| (u, Source::Hreflang)))
                .chain(
                    d.rel_next
                        .iter()
                        .chain(d.rel_prev.iter())
                        .map(|u| (u, Source::Pagination)),
                )
                .chain(d.amphtml.iter().map(|u| (u, Source::Amp)));
            for (u, src) in related {
                if is_internal(sh, u) && next_depth <= sh.cfg.max_depth && wanted(sh, u) {
                    found.push(Job {
                        url: u.clone(),
                        depth: next_depth,
                        source: src,
                    });
                }
            }
        }
    } else {
        page.html = Some(Box::new(data));
    }
    (page, found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_and_bare_host_are_one_site() {
        assert!(same_site("example.com", "www.example.com"));
        assert!(same_site("www.example.com", "example.com"));
        assert!(!same_site("example.com", "shop.example.com"));
        assert!(!same_site("example.com", "example.org"));
    }

    #[test]
    fn lastmod_formats() {
        for ok in [
            "2026",
            "2026-11",
            "2026-11-01",
            "2026-11-01T10:00Z",
            "2026-11-01T10:00+05:30",
            "2026-11-01T10:00:00Z",
        ] {
            assert!(valid_lastmod(ok), "{ok}");
        }
        assert!(!valid_lastmod("23/11/2026"));
    }

    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Answers /robots.txt with a 404 and never answers anything else.
    fn hanging_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                thread::spawn(move || {
                    let mut line = String::new();
                    let _ = BufReader::new(&s).read_line(&mut line);
                    if line.contains("/robots.txt") {
                        let _ = s.write_all(
                            b"HTTP/1.1 404 Not Found
Content-Length: 0
Connection: close

",
                        );
                    } else {
                        thread::sleep(Duration::from_secs(60));
                    }
                });
            }
        });
        port
    }

    fn time_to_finish_after_stop(list_mode: bool) -> Duration {
        let port = hanging_server();
        let seeds: Vec<String> = (0..8)
            .map(|i| format!("http://127.0.0.1:{port}/p/{i}"))
            .collect();
        let cfg = Config {
            threads: 4,
            timeout_secs: 30,
            ..Default::default()
        };
        let h = start(seeds, list_mode, cfg).unwrap();
        thread::sleep(Duration::from_millis(500));
        let t = Instant::now();
        h.stop.store(true, Ordering::Relaxed);
        loop {
            match h.rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Event::Finished) => return t.elapsed(),
                Ok(_) => {}
                Err(e) => panic!("crawl did not finish after stop: {e}"),
            }
        }
    }

    #[test]
    fn stop_is_prompt_while_requests_hang() {
        // crawl mode hangs in the sitemap fetch, list mode in the page workers
        for list_mode in [false, true] {
            let took = time_to_finish_after_stop(list_mode);
            assert!(
                took < Duration::from_secs(2),
                "list_mode={list_mode}: stop took {took:?}"
            );
        }
    }
}
