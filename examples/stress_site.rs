//! Synthetic website for stress-testing the crawler. Uses only std + flate2.
//!
//! USAGE
//!   cargo run --release --example stress_site -- --pages 50000 --port 8099
//!   shouting-toad crawl http://127.0.0.1:8099/ --max-urls 60000 --threads 32 --no-external
//!
//! OPTIONS
//!   --pages N      article pages /p/0 .. /p/N-1 (default 10000)
//!   --port P       listen port on 127.0.0.1 (default 8099)
//!   --slow-ms MS   delay of the slow pages (default 1500)
//!   --hostile      link the hostile pages below from the home page and serve a 2 MB
//!                  robots.txt with broken sitemaps
//!
//! The site is deterministic: the same N gives the same HTML every run.
//! Normal pages are 30-80 KB with nav + footer, 50-150 links, images, scripts and JSON-LD.
//! Every 13th title is duplicated, every 101st is missing; every 50th page is a near copy of
//! its neighbour. Sprinkled in: 301 chains (/r/i -> /r2/i -> /p/i), 404s (/missing/i),
//! 500s (/err/i), slow pages (/slow/i), gzip on 2 of 3 HTML responses, 20 paginated
//! categories (/c/k/n with rel=next/prev), a sitemap index + child sitemaps (with 50
//! orphan URLs found only in the sitemap) and links to a second host (localhost) that
//! count as external.
//!
//! HOSTILE PAGES (--hostile), all under /h/:
//!   big         25 MB HTML page           binary     1 MB of random bytes as text/html
//!   deep        100k nested <div>s        badutf8    invalid UTF-8 labelled utf-8
//!   cp1252      windows-1252 page         loop1      redirect loop loop1 <-> loop2
//!   cal/N       infinite calendar trap    drip       one byte every 500 ms, forever
//!   reset       connection dropped mid-body
//!   empty       200 with an empty body    headers    200 junk headers of 1 KB
//!   hugeheader  one 1 MB header           many       200k unique links
//!   deepinline  100k nested <b>/<i>/<span> without closing tags

use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

struct Cfg {
    pages: usize,
    slow_ms: u64,
    hostile: bool,
    port: u16,
}

const ORPHANS: usize = 50;
const CATEGORIES: usize = 20;
const CATEGORY_PAGES: usize = 10;

fn main() {
    let mut cfg = Cfg { pages: 10_000, slow_ms: 1500, hostile: false, port: 8099 };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).and_then(|v| v.parse::<u64>().ok()).expect("option needs a number");
        match args[i].as_str() {
            "--pages" => cfg.pages = val(i) as usize,
            "--port" => cfg.port = val(i) as u16,
            "--slow-ms" => cfg.slow_ms = val(i),
            "--hostile" => {
                cfg.hostile = true;
                i += 1;
                continue;
            }
            other => panic!("unknown option {other}"),
        }
        i += 2;
    }
    let listener = TcpListener::bind(("127.0.0.1", cfg.port)).expect("bind");
    eprintln!(
        "stress site: http://127.0.0.1:{}/  ({} pages{})",
        cfg.port,
        cfg.pages,
        if cfg.hostile { ", hostile" } else { "" }
    );
    let cfg = Arc::new(cfg);
    for stream in listener.incoming().flatten() {
        let cfg = cfg.clone();
        thread::spawn(move || {
            let _ = handle(stream, &cfg);
        });
    }
}

// ------------------------------------------------------------------ deterministic RNG

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(mix(seed))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(1);
        mix(self.0)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn word(&mut self) -> &'static str {
        WORDS[self.below(WORDS.len())]
    }
}

const WORDS: &[&str] = &[
    "garden", "river", "copper", "lantern", "harbor", "meadow", "violet", "engine", "summit", "canvas", "timber",
    "orbit", "pepper", "falcon", "marble", "velvet", "candle", "glacier", "thunder", "saddle", "compass", "willow",
    "basket", "pillar", "cobalt", "ribbon", "anchor", "cactus", "dragon", "ember", "fossil", "granite", "hazel",
    "island", "jasmine", "kettle", "ladder", "mirror", "nectar", "oyster", "pebble", "quartz", "rocket", "silver",
    "tunnel", "umbrella", "valley", "walnut", "yonder", "zephyr", "the", "and", "of", "to", "a", "in", "is", "for",
    "with", "on", "that", "by", "this", "from", "at", "as", "an", "be", "are", "was", "it", "or", "we", "our",
    "customers", "shipping", "quality", "design", "comfort", "durable", "lightweight", "classic", "modern", "premium",
    "service", "warranty", "season", "collection", "review", "guide", "choose", "compare", "material", "price",
];

fn cap(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

// ------------------------------------------------------------------ HTTP

struct Resp {
    status: u16,
    ctype: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    gzip_ok: bool,
}

fn resp(status: u16, ctype: &'static str, body: impl Into<Vec<u8>>) -> Resp {
    Resp { status, ctype, headers: Vec::new(), body: body.into(), gzip_ok: false }
}

fn html(body: String) -> Resp {
    Resp { gzip_ok: true, ..resp(200, "text/html; charset=utf-8", body) }
}

fn redirect(to: &str) -> Resp {
    let mut r = resp(301, "text/html", "");
    r.headers.push(("Location".into(), to.into()));
    r
}

fn reason(s: u16) -> &'static str {
    match s {
        200 => "OK",
        301 => "Moved Permanently",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Other",
    }
}

fn handle(stream: TcpStream, cfg: &Cfg) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut w = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("/").to_string();
        let mut gzip = false;
        let mut close = false;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 {
                return Ok(());
            }
            let l = h.trim_end().to_ascii_lowercase();
            if l.is_empty() {
                break;
            }
            if l.starts_with("accept-encoding:") && l.contains("gzip") {
                gzip = true;
            }
            if l.starts_with("connection:") && l.contains("close") {
                close = true;
            }
        }
        // Hostile routes that need the raw socket.
        match path.as_str() {
            "/h/drip" if cfg.hostile => {
                w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 100000000\r\n\r\n<html><body>")?;
                loop {
                    thread::sleep(Duration::from_millis(500));
                    w.write_all(b"x")?;
                }
            }
            "/h/reset" if cfg.hostile => {
                w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 500000\r\n\r\n<html><head><title>Reset")?;
                w.write_all(&vec![b'a'; 20_000])?;
                return Ok(()); // drop the connection mid-body
            }
            _ => {}
        }
        let mut r = route(&path, cfg);
        let slow = path.starts_with("/slow/");
        if slow {
            thread::sleep(Duration::from_millis(cfg.slow_ms));
        }
        let mut head = format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\n", r.status, reason(r.status), r.ctype);
        // gzip 2 of 3 compressible responses (by path hash) so both paths get exercised
        if gzip && r.gzip_ok && !mix(path.len() as u64 ^ path.bytes().map(|b| b as u64).sum::<u64>()).is_multiple_of(3) {
            let mut e = GzEncoder::new(Vec::new(), Compression::fast());
            e.write_all(&r.body)?;
            r.body = e.finish()?;
            head.push_str("Content-Encoding: gzip\r\n");
        }
        for (k, v) in &r.headers {
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", r.body.len()));
        w.write_all(head.as_bytes())?;
        if method != "HEAD" {
            w.write_all(&r.body)?;
        }
        w.flush()?;
        if close {
            return Ok(());
        }
    }
}

fn num(s: &str) -> Option<usize> {
    s.split(['/', '?']).next()?.parse().ok()
}

fn route(path: &str, cfg: &Cfg) -> Resp {
    let n = cfg.pages;
    if path == "/" {
        return html(article(0, cfg));
    }
    if path == "/robots.txt" {
        return robots(cfg);
    }
    if let Some(rest) = path.strip_prefix("/p/") {
        return match num(rest) {
            Some(i) if i < n + ORPHANS => html(article(i, cfg)),
            _ => resp(404, "text/html", "<html><body>Not found</body></html>"),
        };
    }
    if let Some(rest) = path.strip_prefix("/r/") {
        return redirect(&format!("/r2/{rest}"));
    }
    if let Some(rest) = path.strip_prefix("/r2/") {
        return redirect(&format!("/p/{rest}"));
    }
    if let Some(rest) = path.strip_prefix("/slow/") {
        return html(article(num(rest).unwrap_or(0) % n.max(1), cfg));
    }
    if path.starts_with("/missing/") {
        return resp(404, "text/html", "<html><head><title>Not found</title></head><body><h1>404</h1></body></html>");
    }
    if path.starts_with("/err/") {
        return resp(500, "text/html", "<html><head><title>Error</title></head><body><h1>500</h1></body></html>");
    }
    if let Some(rest) = path.strip_prefix("/c/") {
        let mut it = rest.split('/');
        let k: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let pg: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        return html(category(k, pg, cfg));
    }
    if matches!(path, "/about" | "/contact" | "/privacy" | "/terms") {
        return html(simple_page(&path[1..]));
    }
    if path.starts_with("/img/") {
        return resp(200, "image/jpeg", vec![0xffu8; 2048]);
    }
    if path.starts_with("/js/") {
        return resp(200, "application/javascript", "console.log('stress');".repeat(50));
    }
    if path.starts_with("/css/") {
        return resp(200, "text/css", "body{margin:0}".repeat(100));
    }
    if path == "/sitemap_index.xml" {
        let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
        for k in 0..(n + ORPHANS).div_ceil(10_000) {
            s.push_str(&format!("<sitemap><loc>http://127.0.0.1:{}/sitemap_{k}.xml</loc></sitemap>", cfg.port));
        }
        s.push_str("</sitemapindex>");
        return resp(200, "application/xml", s);
    }
    if let Some(k) = path.strip_prefix("/sitemap_").and_then(|r| r.strip_suffix(".xml")).and_then(|k| k.parse::<usize>().ok()) {
        let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
        for i in k * 10_000..((k + 1) * 10_000).min(n + ORPHANS) {
            s.push_str(&format!("<url><loc>http://127.0.0.1:{}/p/{i}</loc><lastmod>2026-01-01</lastmod></url>", cfg.port));
        }
        s.push_str("</urlset>");
        return resp(200, "application/xml", s);
    }
    if path.starts_with("/ext/") {
        return html(simple_page("external"));
    }
    if cfg.hostile {
        if let Some(r) = hostile(path, cfg) {
            return r;
        }
    }
    resp(404, "text/html", "<html><body>Not found</body></html>")
}

fn robots(cfg: &Cfg) -> Resp {
    let mut s = format!(
        "User-agent: *\nDisallow: /private/\nAllow: /\n\nSitemap: http://127.0.0.1:{}/sitemap_index.xml\n",
        cfg.port
    );
    if cfg.hostile {
        s.push_str(&format!("Sitemap: http://127.0.0.1:{}/h/sitemap_bad.xml\n", cfg.port));
        s.push_str(&format!("Sitemap: http://127.0.0.1:{}/h/sitemap_self.xml\n", cfg.port));
        s.push_str("this line is not valid\n");
        let mut k = 0;
        while s.len() < 2 * 1024 * 1024 {
            s.push_str(&format!("Disallow: /junk/{k}/*/deep*path$\n"));
            k += 1;
        }
    }
    resp(200, "text/plain", s)
}

fn simple_page(name: &str) -> String {
    format!("<!DOCTYPE html><html lang=\"en\"><head><title>{} | Stress Site</title><meta name=\"description\" content=\"The {name} page.\"></head><body><h1>{}</h1><p>Static {name} page.</p><a href=\"/\">Home</a></body></html>", cap(name), cap(name))
}

fn nav(out: &mut String) {
    out.push_str("<header><nav><a href=\"/\">Home</a>");
    for k in 0..CATEGORIES {
        out.push_str(&format!("<a href=\"/c/{k}/1\">Category {k}</a>"));
    }
    out.push_str("</nav></header>");
}

fn footer(out: &mut String) {
    out.push_str("<footer><a href=\"/about\">About</a><a href=\"/contact\">Contact</a><a href=\"/privacy\">Privacy</a><a href=\"/terms\">Terms</a><p>© Stress Site</p></footer>");
}

fn category(k: usize, pg: usize, cfg: &Cfg) -> String {
    let pg = pg.clamp(1, CATEGORY_PAGES);
    let mut s = format!("<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Category {k} page {pg} | Stress Site</title><meta name=\"description\" content=\"Items in category {k}, page {pg}.\">");
    if pg > 1 {
        s.push_str(&format!("<link rel=\"prev\" href=\"/c/{k}/{}\">", pg - 1));
    }
    if pg < CATEGORY_PAGES {
        s.push_str(&format!("<link rel=\"next\" href=\"/c/{k}/{}\">", pg + 1));
    }
    s.push_str("</head><body>");
    nav(&mut s);
    s.push_str(&format!("<main><h1>Category {k}</h1><ul>"));
    let mut r = Rng::new((k * 1000 + pg) as u64);
    for _ in 0..30 {
        let t = r.below(cfg.pages);
        s.push_str(&format!("<li><a href=\"/p/{t}\">{} {}</a></li>", cap(r.word()), r.word()));
    }
    s.push_str("</ul>");
    if pg > 1 {
        s.push_str(&format!("<a href=\"/c/{k}/{}\">Previous</a> ", pg - 1));
    }
    if pg < CATEGORY_PAGES {
        s.push_str(&format!("<a href=\"/c/{k}/{}\">Next</a>", pg + 1));
    }
    s.push_str("</main>");
    footer(&mut s);
    s.push_str("</body></html>");
    s
}

fn article(i: usize, cfg: &Cfg) -> String {
    let n = cfg.pages;
    // every 50th page is a near copy of its neighbour (same text seed, different title)
    let text_seed = if i.is_multiple_of(50) && i > 0 { i - 1 } else { i } as u64;
    let mut r = Rng::new(i as u64 * 7919);
    let mut t = Rng::new(text_seed * 104_729);
    let mut s = String::with_capacity(64 * 1024);
    s.push_str("<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    let (w1, w2) = (cap(r.word()), r.word());
    if !i.is_multiple_of(101) {
        if i.is_multiple_of(13) {
            s.push_str("<title>Product page | Stress Site</title>");
        } else {
            s.push_str(&format!("<title>{w1} {w2} guide, article {i} | Stress Site</title>"));
        }
    }
    if !i.is_multiple_of(17) {
        s.push_str(&format!("<meta name=\"description\" content=\"Everything about {w1} {w2}: a long read about article {i} with tips and reviews.\">"));
    }
    s.push_str(&format!("<link rel=\"canonical\" href=\"http://127.0.0.1:{}{}\">", cfg.port, if i == 0 { "/".to_string() } else { format!("/p/{i}") }));
    s.push_str("<link rel=\"stylesheet\" href=\"/css/site.css\"><script src=\"/js/0.js\" defer></script>");
    s.push_str(&format!(
        "<script type=\"application/ld+json\">{{\"@context\":\"https://schema.org\",\"@type\":\"Article\",\"headline\":\"{w1} {w2} {i}\",\"author\":{{\"@type\":\"Person\",\"name\":\"Writer {}\"}},\"datePublished\":\"2026-01-{:02}\"}}</script>",
        i % 40,
        i % 28 + 1
    ));
    s.push_str("</head><body>");
    nav(&mut s);
    s.push_str(&format!("<main><h1>{w1} {w2} {i}</h1>"));
    // tree links keep every page within log10(N) clicks of the home page
    let mut links: Vec<String> = (i * 10 + 1..=i * 10 + 10).filter(|&c| c < n).map(|c| format!("/p/{c}")).collect();
    for _ in 0..40 + r.below(90) {
        links.push(format!("/p/{}", r.below(n)));
    }
    if i.is_multiple_of(40) {
        links.push(format!("/r/{i}"));
    }
    if i % 50 == 7 {
        links.push(format!("/missing/{i}"));
    }
    if i % 200 == 3 {
        links.push(format!("/err/{i}"));
    }
    if i % 499 == 5 {
        links.push(format!("/slow/{i}"));
    }
    if i % 100 == 9 {
        links.push(format!("http://localhost:{}/ext/{}", cfg.port, i % 20));
    }
    if i == 0 && cfg.hostile {
        for h in [
            "big", "binary", "deep", "badutf8", "cp1252", "loop1", "cal/0", "drip", "reset", "empty", "headers",
            "hugeheader", "many", "deepinline",
        ] {
            links.push(format!("/h/{h}"));
        }
    }
    let paragraphs = 25 + t.below(50);
    let mut li = 0;
    for p in 0..paragraphs {
        if p % 8 == 0 {
            s.push_str(&format!("<h2>{} {}</h2>", cap(t.word()), t.word()));
        }
        s.push_str("<p>");
        for k in 0..60 + t.below(60) {
            s.push_str(t.word());
            s.push_str(if k % 12 == 11 { ". " } else { " " });
        }
        // a few links per paragraph, plus an image now and then
        for _ in 0..3 {
            if li < links.len() {
                s.push_str(&format!("<a href=\"{}\">{} {}</a> ", links[li], t.word(), t.word()));
                li += 1;
            }
        }
        if p % 4 == 0 {
            let img = r.below(1000);
            if img.is_multiple_of(9) {
                s.push_str(&format!("<img src=\"/img/{img}.jpg\">"));
            } else {
                s.push_str(&format!("<img src=\"/img/{img}.jpg\" alt=\"{} {}\" width=\"400\" height=\"300\" loading=\"lazy\">", t.word(), t.word()));
            }
        }
        s.push_str("</p>");
    }
    s.push_str("<ul class=\"related\">");
    for l in &links[li.min(links.len())..] {
        s.push_str(&format!("<li><a href=\"{l}\">Related {}</a></li>", r.word()));
    }
    s.push_str("</ul></main>");
    footer(&mut s);
    s.push_str(&format!("<script src=\"/js/{}.js\" async></script></body></html>", 1 + i % 4));
    s
}

// ------------------------------------------------------------------ hostile pages

fn hostile(path: &str, cfg: &Cfg) -> Option<Resp> {
    static BIG: OnceLock<Vec<u8>> = OnceLock::new();
    let p = path.strip_prefix("/h/")?;
    Some(match p {
        "big" => {
            let body = BIG.get_or_init(|| {
                let mut s = String::from("<!DOCTYPE html><html><head><title>Big page</title></head><body>");
                let mut k = 0;
                while s.len() < 25 * 1024 * 1024 {
                    s.push_str(&format!("<p>Paragraph {k} with some filler words to make it longer. <a href=\"/p/{}\">link</a></p>\n", k % cfg.pages.max(1)));
                    k += 1;
                }
                s.push_str("</body></html>");
                s.into_bytes()
            });
            resp(200, "text/html", body.clone())
        }
        "binary" => {
            let mut r = Rng::new(42);
            resp(200, "text/html", (0..1024 * 1024).map(|_| r.next() as u8).collect::<Vec<u8>>())
        }
        "deep" => {
            let depth = 100_000;
            let mut s = String::from("<!DOCTYPE html><html><head><title>Deep</title></head><body>");
            s.push_str(&"<div>".repeat(depth));
            s.push_str("deep text <a href=\"/p/1\">down here</a>");
            s.push_str(&"</div>".repeat(depth));
            s.push_str("</body></html>");
            resp(200, "text/html", s)
        }
        "deepinline" => {
            let mut s = String::from("<!DOCTYPE html><html><head><title>Deep inline</title></head><body>");
            for k in 0..100_000 {
                s.push_str(["<b>", "<i>", "<span>", "<a href=\"/p/2\">"][k % 4]);
            }
            s.push_str("text</body></html>");
            resp(200, "text/html", s)
        }
        "badutf8" => {
            let mut b: Vec<u8> = b"<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Bad \xff\xfe UTF-8 ".to_vec();
            b.extend("é".repeat(80).as_bytes());
            b.extend("😀".repeat(30).as_bytes());
            b.extend(b"\xc3</title><meta name=\"description\" content=\"");
            b.extend("ü\u{0301}".repeat(200).as_bytes());
            b.extend(b"\xe2\x82\"></head><body><h1>\xf0\x9f\x98</h1><p>");
            b.extend("文字化け ".repeat(3000).as_bytes());
            b.extend(b"\x80\x81</p><a href=\"/p/3?q=\xff\">bad href</a></body></html>\xe2");
            resp(200, "text/html; charset=utf-8", b)
        }
        "cp1252" => {
            let mut b: Vec<u8> = b"<!DOCTYPE html><html><head><title>Caf\xe9 \x93quoted\x94 \x96 dash</title></head><body><p>".to_vec();
            for _ in 0..500 {
                b.extend(b"Na\xefve r\xe9sum\xe9 \x80 price \x85 ");
            }
            b.extend(b"</p></body></html>");
            resp(200, "text/html; charset=windows-1252", b)
        }
        "loop1" => redirect("/h/loop2"),
        "loop2" => redirect("/h/loop1"),
        "empty" => resp(200, "text/html", ""),
        "headers" => {
            let mut r = html(simple_page("headers"));
            for k in 0..200 {
                r.headers.push((format!("X-Junk-{k}"), "j".repeat(1024)));
            }
            r
        }
        "hugeheader" => {
            let mut r = html(simple_page("hugeheader"));
            r.headers.push(("X-Huge".into(), "h".repeat(1024 * 1024)));
            r
        }
        "many" => {
            let mut s = String::from("<!DOCTYPE html><html><head><title>Many links</title></head><body>");
            for k in 0..200_000 {
                s.push_str(&format!("<a href=\"/h/many/{k}\">l{k}</a>"));
            }
            s.push_str("</body></html>");
            html(s)
        }
        "sitemap_bad.xml" => resp(200, "application/xml", "<?xml version=\"1.0\"?><urlset><url><loc>http://127.0.0.1/p/1</loc><url><loc>broken"),
        "sitemap_self.xml" => resp(
            200,
            "application/xml",
            format!("<?xml version=\"1.0\"?><sitemapindex><sitemap><loc>http://127.0.0.1:{}/h/sitemap_self.xml</loc></sitemap></sitemapindex>", cfg.port),
        ),
        _ if p.starts_with("cal/") => {
            let d: u64 = p[4..].parse().unwrap_or(0);
            let mut s = format!("<!DOCTYPE html><html><head><title>Calendar day {d}</title></head><body><h1>Day {d}</h1>");
            for k in 1..=10 {
                s.push_str(&format!("<a href=\"/h/cal/{}\">day {}</a> ", d.wrapping_mul(10) + k, d.wrapping_mul(10) + k));
            }
            s.push_str("</body></html>");
            html(s)
        }
        _ if p.starts_with("many/") => html(simple_page("many")),
        _ => return None,
    })
}
