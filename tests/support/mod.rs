//! Shared helpers for the end-to-end tests: a std-only fixture HTTP server,
//! an HTML page builder and assertion helpers keyed by check id + URL.
#![allow(dead_code)]

use shouting_toad::audit::{self, Audit};
use shouting_toad::checks::{catalogue, find};
use shouting_toad::crawler::{self, Config, Event};
use shouting_toad::model::{Crawl, Page};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ routes

#[derive(Clone, Default)]
pub struct Route {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// gzip the body when the client accepts it.
    pub gzip: bool,
    pub delay_ms: u64,
    /// Close the connection without answering.
    pub drop: bool,
    /// Served instead when the User-Agent contains the needle (first match wins).
    pub by_ua: Vec<(String, Route)>,
}

pub const SECURE_HEADERS: [(&str, &str); 6] = [
    (
        "Content-Security-Policy",
        "default-src 'self'; frame-ancestors 'self'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "SAMEORIGIN"),
    ("Referrer-Policy", "strict-origin-when-cross-origin"),
    ("Permissions-Policy", "camera=(), geolocation=()"),
    ("Cross-Origin-Opener-Policy", "same-origin"),
];

impl Route {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Route {
        let mut r = Route {
            status,
            body: body.into(),
            ..Default::default()
        };
        if !content_type.is_empty() {
            r.headers.push(("Content-Type".into(), content_type.into()));
        }
        r
    }
    pub fn header(mut self, k: &str, v: &str) -> Route {
        self.headers.push((k.into(), v.into()));
        self
    }
    /// Replace (or remove, with an empty value) a header.
    pub fn set(mut self, k: &str, v: &str) -> Route {
        self.headers.retain(|(h, _)| !h.eq_ignore_ascii_case(k));
        if v.is_empty() {
            self
        } else {
            self.header(k, v)
        }
    }
    pub fn gzip(mut self, on: bool) -> Route {
        self.gzip = on;
        self
    }
    pub fn delay(mut self, ms: u64) -> Route {
        self.delay_ms = ms;
        self
    }
    /// Answer `route` to user agents containing `needle`.
    pub fn for_ua(mut self, needle: &str, route: Route) -> Route {
        self.by_ua.push((needle.into(), route));
        self
    }
}

/// 200 HTML page with good security headers, gzipped.
pub fn html(body: impl Into<String>) -> Route {
    let mut r = Route::new(200, "text/html; charset=utf-8", body.into()).gzip(true);
    for (k, v) in SECURE_HEADERS {
        r = r.header(k, v);
    }
    r
}

pub fn redirect(status: u16, location: &str) -> Route {
    Route::new(status, "text/html; charset=utf-8", "").header("Location", location)
}

/// Error page. Its body links to /orphan: links on error pages must not count as inlinks.
pub fn error(status: u16) -> Route {
    Route::new(
        status,
        "text/html; charset=utf-8",
        format!("<!DOCTYPE html><html lang=\"en\"><head><title>Error {status}</title></head><body><h1>Error {status}</h1><a href=\"/orphan\">Orphan toad page</a></body></html>"),
    )
}

/// Static file with long-lived cache headers and a validator.
pub fn file(content_type: &str, body: impl Into<Vec<u8>>) -> Route {
    Route::new(200, content_type, body)
        .header("Cache-Control", "public, max-age=31536000")
        .header("ETag", "\"v1\"")
}

pub type Site = HashMap<String, Route>;

pub fn gz(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

// ------------------------------------------------------------------ server

/// (Host header, method, request target, user agent) for every request received.
type Log = Arc<Mutex<Vec<(String, String, String, String)>>>;

pub struct Server {
    pub port: u16,
    /// Internal origin, e.g. http://127.0.0.1:1234
    pub base: String,
    /// Same server under another host name, so the crawler treats it as external.
    pub ext: String,
    log: Log,
}

impl Server {
    pub fn start(build: impl FnOnce(&str, &str, u16) -> Site) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let ext = format!("http://localhost:{port}");
        let routes = Arc::new(build(&base, &ext, port));
        let log: Log = Arc::default();
        let mut listeners = vec![listener];
        // "localhost" may resolve to ::1 first; answer there too when the port is free.
        if let Ok(l) = TcpListener::bind(("::1", port)) {
            listeners.push(l);
        }
        for l in listeners {
            let (routes, log) = (routes.clone(), log.clone());
            thread::spawn(move || {
                for s in l.incoming().flatten() {
                    let (routes, log) = (routes.clone(), log.clone());
                    thread::spawn(move || serve(s, &routes, &log));
                }
            });
        }
        Server {
            port,
            base,
            ext,
            log,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// True when any request (any host, any method) asked for this target.
    pub fn requested(&self, target: &str) -> bool {
        self.log
            .lock()
            .unwrap()
            .iter()
            .any(|(_, _, t, _)| t == target)
    }

    /// Requests whose User-Agent contains `needle`.
    pub fn requests_by(&self, needle: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, _, ua)| ua.contains(needle))
            .count()
    }

    pub fn request_count(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

/// A port with nothing listening on it.
pub fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn serve(mut s: TcpStream, routes: &Site, log: &Mutex<Vec<(String, String, String, String)>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.first() == Some(&0x16) {
            // TLS ClientHello on a plain-HTTP port: answer in plain text so the handshake fails fast.
            let _ = s.write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = text.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_string();
    let target = first.next().unwrap_or("/").to_string();
    let header = |name: &str| {
        lines
            .clone()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.trim().to_string())
            })
            .unwrap_or_default()
    };
    let host = header("host");
    let accepts_gzip = header("accept-encoding").contains("gzip");
    let ua = header("user-agent");
    log.lock()
        .unwrap()
        .push((host, method.clone(), target.clone(), ua.clone()));

    let fallback = error(404);
    let path = target.split('?').next().unwrap_or("");
    let route = routes
        .get(&target)
        .or_else(|| routes.get(path))
        .unwrap_or(&fallback);
    let route = route
        .by_ua
        .iter()
        .find(|(needle, _)| ua.contains(needle.as_str()))
        .map_or(route, |(_, r)| r);
    if route.delay_ms > 0 {
        thread::sleep(Duration::from_millis(route.delay_ms));
    }
    if route.drop {
        return;
    }
    let mut head = format!("HTTP/1.1 {} Fixture\r\n", route.status);
    for (k, v) in &route.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    let body = if route.gzip && accepts_gzip {
        head.push_str("Content-Encoding: gzip\r\n");
        gz(&route.body)
    } else {
        route.body.clone()
    };
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let _ = s.write_all(head.as_bytes());
    if method != "HEAD" {
        let _ = s.write_all(&body);
    }
    let _ = s.flush();
}

// ------------------------------------------------------------------ HTML builder

const WORDS: &str = "toad frog pond lily green jump swim leaf rain mud bug fly hop log reed moss stone bank creek shade sun warm cool night day song call egg tail home rest wait look find eat grow live small big quick slow calm wild soft loud clear deep wet dry old new good kind brave shy bright dark gold brown spot skin eye leg foot toe web back side road path field farm barn hill wood tree root branch nest bird fish snail worm bee ant moth wing sky cloud wind storm drop pool lake shore sand rock cave shell seed plant bloom fruit berry nut corn wheat grass hay rope boat dock pier net hook bait catch keep give take make help play read walk talk sing hear see feel know learn show tell ask turn move stay start stop";

/// Deterministic, easy-to-read filler text, different for every seed.
pub fn prose(seed: &str, words: usize) -> String {
    let vocab: Vec<&str> = WORDS.split(' ').collect();
    let mut x = shouting_toad::parse::fnv64(seed.as_bytes());
    let mut out = String::new();
    for i in 0..words {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let w = vocab[(x >> 33) as usize % vocab.len()];
        if i % 8 == 0 {
            let mut c = w.chars();
            out.push_str(&(c.next().unwrap().to_ascii_uppercase().to_string() + c.as_str()));
        } else {
            out.push_str(w);
        }
        out.push_str(if i % 8 == 7 || i + 1 == words {
            ". "
        } else {
            " "
        });
    }
    out.trim_end().to_string()
}

#[derive(Clone)]
pub struct Doc {
    pub doctype: bool,
    /// Attributes of <html>, e.g. ` lang="en"`.
    pub html_attrs: String,
    pub body_attrs: String,
    pub head: Vec<String>,
    pub body: Vec<String>,
    /// Wrap the body in <main>.
    pub main: bool,
}

impl Doc {
    /// A page that should pass every check except those caused by the fixture being HTTP/1.1.
    pub fn good(base: &str, path: &str, topic: &str) -> Doc {
        let url = format!("{base}{path}");
        let title = format!("{topic} - Toad Shop Fixture Guide");
        let desc = format!("Learn all about {topic} at the Toad Shop, with plain tips, clear examples and honest answers for every curious reader.");
        Doc {
            doctype: true,
            html_attrs: " lang=\"en\"".into(),
            body_attrs: String::new(),
            head: vec![
                r#"<meta charset="utf-8">"#.into(),
                r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#.into(),
                format!("<title>{title}</title>"),
                format!(r#"<meta name="description" content="{desc}">"#),
                format!(r#"<link rel="canonical" href="{url}">"#),
                r#"<link rel="icon" href="/favicon.ico">"#.into(),
                r#"<link rel="apple-touch-icon" href="/apple-touch-icon.png">"#.into(),
                format!(r#"<meta property="og:title" content="{title}">"#),
                format!(r#"<meta property="og:description" content="{desc}">"#),
                format!(r#"<meta property="og:image" content="{base}/img/share.png">"#),
                format!(r#"<meta property="og:url" content="{url}">"#),
                r#"<meta property="og:type" content="article">"#.into(),
                r#"<meta property="og:site_name" content="Toad Shop">"#.into(),
                r#"<meta name="twitter:card" content="summary_large_image">"#.into(),
                format!(r#"<script type="application/ld+json">{{"@context":"https://schema.org","@type":"WebPage","name":"{topic}"}}</script>"#),
            ],
            body: vec![
                format!("<h1>{topic} explained</h1>"),
                format!("<h2>Why {topic} matters</h2>"),
                format!(r#"<p class="prose">{}</p>"#, prose(topic, 220)),
                r#"<nav><a href="/">Toad Shop home page</a> and <a href="/perfect">our perfect buying guide</a></nav>"#.into(),
            ],
            main: true,
        }
    }

    /// Remove head and body items starting with `prefix`.
    pub fn drop(mut self, prefix: &str) -> Doc {
        self.head.retain(|t| !t.starts_with(prefix));
        self.body.retain(|t| !t.starts_with(prefix));
        self
    }
    pub fn head(mut self, s: impl Into<String>) -> Doc {
        self.head.push(s.into());
        self
    }
    pub fn body(mut self, s: impl Into<String>) -> Doc {
        self.body.push(s.into());
        self
    }
    pub fn prepend(mut self, s: impl Into<String>) -> Doc {
        self.body.insert(0, s.into());
        self
    }
    pub fn title(self, t: &str) -> Doc {
        self.drop("<title").head(format!("<title>{t}</title>"))
    }
    pub fn desc(self, d: &str) -> Doc {
        self.drop("<meta name=\"description\"")
            .head(format!(r#"<meta name="description" content="{d}">"#))
    }
    pub fn canonical(self, href: &str) -> Doc {
        self.drop("<link rel=\"canonical\"")
            .head(format!(r#"<link rel="canonical" href="{href}">"#))
    }
    pub fn robots(self, content: &str) -> Doc {
        self.head(format!(r#"<meta name="robots" content="{content}">"#))
    }
    pub fn h1(self, t: &str) -> Doc {
        self.drop("<h1").prepend(format!("<h1>{t}</h1>"))
    }
    pub fn lang(mut self, l: Option<&str>) -> Doc {
        self.html_attrs = l.map(|l| format!(" lang=\"{l}\"")).unwrap_or_default();
        self
    }
    pub fn html(&self) -> String {
        let body = self.body.join("\n");
        format!(
            "{}<html{}><head>\n{}\n</head><body{}>{}</body></html>",
            if self.doctype {
                "<!DOCTYPE html>\n"
            } else {
                ""
            },
            self.html_attrs,
            self.head.join("\n"),
            self.body_attrs,
            if self.main {
                format!("<main>{body}</main>")
            } else {
                body
            }
        )
    }
    pub fn route(&self) -> Route {
        html(self.html())
    }
}

// ------------------------------------------------------------------ crawling

pub fn config() -> Config {
    Config {
        threads: 12,
        timeout_secs: 4,
        ..Default::default()
    }
}

/// Run a crawl to completion, collecting events the way the CLI does.
pub fn crawl(seeds: &[String], list_mode: bool, cfg: Config) -> Crawl {
    let handle = crawler::start(seeds.to_vec(), list_mode, cfg).expect("crawl should start");
    let mut c = Crawl {
        start_url: crawler::normalize_url(&seeds[0]).unwrap().to_string(),
        ..Default::default()
    };
    let deadline = Instant::now() + Duration::from_secs(50);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match handle
            .rx
            .recv_timeout(left)
            .expect("crawl did not finish in time")
        {
            Event::Page(p) => c.pages.push(*p),
            Event::Site(s) => c.site = *s,
            Event::Finished => break,
            Event::Progress { .. } | Event::Log(_) => {}
        }
    }
    c.pages.sort_by_key(|p| p.id);
    c
}

/// Crawl + audit with assertion helpers. Failures are collected and reported together by `finish`.
pub struct Audited {
    pub crawl: Crawl,
    pub audit: Audit,
    pub base: String,
    pub asserted: RefCell<BTreeSet<String>>,
    pub failures: RefCell<Vec<String>>,
}

impl Audited {
    pub fn new(crawl: Crawl, base: &str) -> Audited {
        let audit = audit::run(&crawl);
        Audited {
            crawl,
            audit,
            base: base.into(),
            asserted: RefCell::default(),
            failures: RefCell::default(),
        }
    }

    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        }
    }

    pub fn find(&self, path: &str) -> Option<&Page> {
        let u = self.url(path);
        self.crawl.pages.iter().find(|p| p.url == u)
    }

    /// The crawled page (an empty one, with a recorded failure, when it was not crawled).
    pub fn page(&self, path: &str) -> &Page {
        self.find(path).unwrap_or_else(|| {
            self.fail(format!("{} was not crawled", self.url(path)));
            Box::leak(Box::default())
        })
    }

    fn check(id: &str) -> usize {
        find(id).unwrap_or_else(|| panic!("unknown check id {id}"))
    }

    /// URLs a check fires on.
    pub fn hits(&self, id: &str) -> Vec<&str> {
        self.audit.hits[Self::check(id)]
            .iter()
            .map(|&i| self.crawl.pages[i].url.as_str())
            .collect()
    }

    pub fn has(&self, id: &str, path: &str) -> bool {
        let u = self.url(path);
        self.hits(id).contains(&u.as_str())
    }

    /// Check ids that fire on a URL.
    pub fn checks_on(&self, path: &str) -> Vec<&'static str> {
        let u = self.url(path);
        let cat = catalogue();
        match self.crawl.pages.iter().position(|p| p.url == u) {
            Some(i) => self.audit.page_issues[i]
                .iter()
                .map(|&c| cat[c].id.as_str())
                .collect(),
            None => Vec::new(),
        }
    }

    fn fail(&self, msg: String) {
        self.failures.borrow_mut().push(msg);
    }

    pub fn hit(&self, id: &str, path: &str) {
        if self.find(path).is_none() {
            self.fail(format!("{id}: {} was not crawled", self.url(path)));
        } else if self.has(id, path) {
            self.asserted.borrow_mut().insert(id.to_string());
        } else {
            self.fail(format!(
                "{id} should fire on {} (fires on {:?})",
                self.url(path),
                self.hits(id)
            ));
        }
    }

    pub fn no_hit(&self, id: &str, path: &str) {
        if self.has(id, path) {
            self.fail(format!("{id} should NOT fire on {}", self.url(path)));
        }
    }

    pub fn expect(&self, ok: bool, msg: &str) {
        if !ok {
            self.fail(msg.to_string());
        }
    }

    /// Every check firing on the page must be in `allowed`.
    pub fn only(&self, path: &str, allowed: &[&str]) {
        for id in self.checks_on(path) {
            if !allowed.contains(&id) {
                self.fail(format!("false positive: {id} fires on {}", self.url(path)));
            }
        }
    }

    /// Ids triggered anywhere in this crawl.
    pub fn triggered(&self) -> BTreeSet<String> {
        let cat = catalogue();
        self.audit
            .triggered()
            .map(|(i, _)| cat[i].id.clone())
            .collect()
    }

    pub fn finish(&self) {
        let f = self.failures.borrow();
        assert!(
            f.is_empty(),
            "{} failed assertions:\n  {}",
            f.len(),
            f.join("\n  ")
        );
    }
}
