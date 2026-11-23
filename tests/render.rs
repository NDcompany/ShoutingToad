//! JavaScript rendering end to end, against a JS-heavy fixture site. Needs Chrome, Chromium or
//! Edge; without one the tests print why and pass.

mod support;

use shouting_toad::crawler::{self, Event};
use shouting_toad::render;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use support::*;

fn browser() -> bool {
    match render::find_browser("") {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipping rendering test: {e}");
            false
        }
    }
}

fn site(base: &str) -> Site {
    let mut s = Site::new();
    let page = |body: &str, head: &str| {
        // no CSP: the fixture relies on inline scripts
        html(format!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head}</head><body><main>{body}</main></body></html>"
        ))
        .set("Content-Security-Policy", "")
    };
    s.insert(
        "/".into(),
        page(
            &format!(
                r#"<h1>Toad shop</h1><p>A short raw intro.</p>
<p style="color:#bbb;background:#fff;font-size:16px">Faint grey text that is hard to read</p>
<div id="app"></div>
<div id="clicky" onclick="this.textContent='clicked'" style="padding:10px">Click me</div>
<a href="/good" class="nofocus">Good page</a> <a href="/slow">Slow page</a>
<style>a.nofocus:focus{{outline:none}}</style>
<script>
document.getElementById('app').innerHTML = '<p>' + 'Toads hop along the muddy bank. '.repeat(60) + '</p><a href="/js-only">A page linked only by script</a>';
document.querySelector('link[rel=canonical]').href = '{base}/other';
console.error('fixture boom');
</script>"#
            ),
            &format!("<title>JS toad shop</title><link rel=\"canonical\" href=\"{base}/\">"),
        ),
    );
    s.insert(
        "/good".into(),
        page(
            &format!(
                "<h1>Good page</h1><p style=\"color:#222\">{}</p><a href=\"/\">Home</a>",
                prose("good", 120)
            ),
            &format!("<title>Good toad page</title><link rel=\"canonical\" href=\"{base}/good\">"),
        ),
    );
    s.insert(
        "/js-only".into(),
        page(
            "<h1>Only by script</h1><p>Found after rendering.</p><a href=\"/\">Home</a>",
            &format!("<title>JS only page</title><link rel=\"canonical\" href=\"{base}/js-only\">"),
        ),
    );
    s.insert(
        "/slow".into(),
        page(
            "<h1>Slow</h1><p>Waits for a script that never arrives.</p><script src=\"/hang.js\"></script><p>After the script.</p>",
            &format!("<title>Slow toad page</title><link rel=\"canonical\" href=\"{base}/slow\">"),
        ),
    );
    s.insert(
        "/hang.js".into(),
        Route::new(200, "text/javascript", "1").delay(20_000),
    );
    s
}

/// Profiles left behind, after giving a background clean-up up to 5 s.
fn profile_dirs() -> usize {
    let t = Instant::now();
    while count_profiles() > 0 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100));
    }
    count_profiles()
}

fn count_profiles() -> usize {
    let prefix = format!("shoutingtoad-chrome-{}-", std::process::id());
    std::fs::read_dir(std::env::temp_dir())
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn renders_js_site_compares_raw_and_rendered_and_cleans_up() {
    if !browser() {
        return;
    }
    let srv = Server::start(|base, _, _| site(base));
    let cfg = shouting_toad::crawler::Config {
        render: true,
        render_tabs: 2,
        threads: 4,
        timeout_secs: 5,
        check_external: false,
        ..config()
    };
    let a = Audited::new(crawl(&[srv.url("/")], false, cfg.clone()), &srv.base);
    assert_eq!(render::live_browsers(), 0, "browser left running");
    assert_eq!(profile_dirs(), 0, "browser profile left behind");

    let home = a.page("/");
    let r = home.rendered.as_deref().expect("home was rendered");
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        r.word_count > home.word_count() + 250,
        "raw {} rendered {}",
        home.word_count(),
        r.word_count
    );
    assert!(
        r.new_links.contains(&srv.url("/js-only")),
        "{:?}",
        r.new_links
    );
    assert_eq!(r.canonical, srv.url("/other"));
    assert!(
        r.js_errors.iter().any(|e| e.contains("fixture boom")),
        "{:?}",
        r.js_errors
    );
    assert!(r.a11y_tested);
    assert!(r.contrast_fail_count >= 1, "{:?}", r.contrast);
    assert!(r
        .contrast
        .iter()
        .any(|c| c.fg == "#bbbbbb" && c.ratio < 2.0));
    assert!(
        r.click_no_keyboard.iter().any(|c| c.contains("clicky")),
        "{:?}",
        r.click_no_keyboard
    );
    assert!(
        r.focus_not_visible.iter().any(|f| f.contains("Good page")),
        "tested {} {:?}",
        r.focus_tested,
        r.focus_not_visible
    );

    for id in [
        "render.content_js_only",
        "render.links_js_only",
        "render.canonical_changed",
        "render.js_errors",
        "a11y.low_contrast",
        "a11y.click_not_keyboard",
        "a11y.focus_not_visible",
    ] {
        a.hit(id, "/");
    }
    a.hit("render.timeout", "/slow");
    // the page only reachable through a JavaScript link was crawled, and counts as linked
    a.find("/js-only").expect("/js-only crawled");
    a.no_hit("links.no_inlinks", "/js-only");
    let good = a.checks_on("/good");
    assert!(
        !good
            .iter()
            .any(|c| c.starts_with("render.") || RENDER_A11Y.contains(c)),
        "{good:?}"
    );
    assert!(a.page("/good").rendered.as_ref().unwrap().focus_tested >= 1);
    a.finish();

    // reports carry the comparison
    let report = shouting_toad::report::html_report(&a.crawl, &a.audit, "", "");
    assert!(report.contains("raw HTML vs rendered"));
    let wcag = shouting_toad::wcag::html(&a.crawl, &a.audit);
    assert!(wcag.contains("1.4.3") && wcag.contains("JavaScript rendering was on"));

    // Stop while rendering: prompt, and the browser is gone.
    let slow = Server::start(|base, _, _| {
        let mut s = site(base);
        for i in 0..40 {
            s.insert(
                format!("/p/{i}"),
                html(format!("<html><body><a href=\"/p/{}\">next</a><script async src=\"/hang.js\"></script></body></html>", i + 1)),
            );
        }
        s.insert(
            "/".into(),
            html(
                (0..40)
                    .map(|i| format!("<a href=\"/p/{i}\">p</a>"))
                    .collect::<String>(),
            ),
        );
        s
    });
    let h = crawler::start(vec![slow.url("/")], false, cfg).unwrap();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(4) {
        let _ = h.rx.recv_timeout(Duration::from_millis(100));
    }
    let stop_at = Instant::now();
    h.stop.store(true, Ordering::Relaxed);
    loop {
        match h.rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Event::Finished) => break,
            Ok(_) => {}
            Err(e) => panic!("no Finished after stop: {e}"),
        }
    }
    let took = stop_at.elapsed();
    eprintln!("stop took {took:?}");
    assert!(took < Duration::from_secs(3), "stop took {took:?}");
    assert_eq!(
        render::live_browsers(),
        0,
        "browser left running after stop"
    );
    assert_eq!(profile_dirs(), 0, "browser profile left behind after stop");
}

const RENDER_A11Y: [&str; 3] = [
    "a11y.low_contrast",
    "a11y.click_not_keyboard",
    "a11y.focus_not_visible",
];

#[test]
fn missing_browser_is_reported_and_crawl_continues() {
    let srv = Server::start(|base, _, _| site(base));
    let cfg = shouting_toad::crawler::Config {
        render: true,
        browser_path: "/no/such/browser".into(),
        check_external: false,
        ..config()
    };
    let h = crawler::start(vec![srv.url("/")], false, cfg).unwrap();
    let mut logs = Vec::new();
    let mut pages = 0;
    loop {
        match h.rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            Event::Log(m) => logs.push(m),
            Event::Page(p) => {
                assert!(p.rendered.is_none());
                pages += 1;
            }
            Event::Finished => break,
            _ => {}
        }
    }
    assert!(pages >= 3, "crawl continued without rendering");
    assert!(
        logs.iter().any(|l| l.contains("Browser not found")),
        "{logs:?}"
    );
}
