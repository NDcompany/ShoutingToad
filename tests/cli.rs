//! The `shouting-toad` binary end to end: exit codes, report files and saved crawls.

mod support;

use shouting_toad::checks::catalogue;
use shouting_toad::model::Crawl;
use std::path::PathBuf;
use std::process::Command;
use support::*;

const FILES: [&str; 9] = [
    "report.html",
    "issues.csv",
    "issue_urls.csv",
    "internal.csv",
    "inlinks.csv",
    "outlinks.csv",
    "images.csv",
    "redirects.csv",
    "crawl.shoutingtoad",
];

fn out_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("st-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn run(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_shouting-toad"))
        .args(args)
        .output()
        .expect("binary runs");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// Three mutually linked pages with robots.txt and a sitemap: nothing to report but plain HTTP.
fn clean_site(b: &str) -> Site {
    let nav = r#"<nav><a href="/">Toad Shop home page</a> <a href="/about">About the Toad Shop</a> <a href="/contact">Contact the Toad Shop</a></nav>"#;
    let mut s = Site::new();
    for (path, topic) in [
        ("/", "Toad Shop home"),
        ("/about", "About our pond shop"),
        ("/contact", "Contact our pond team"),
    ] {
        s.insert(
            path.into(),
            Doc::good(b, path, topic).drop("<nav").body(nav).route(),
        );
    }
    s.insert(
        "/robots.txt".into(),
        Route::new(
            200,
            "text/plain",
            format!("User-agent: *\nAllow: /\nSitemap: {b}/sitemap.xml\n"),
        ),
    );
    s.insert("/sitemap.xml".into(), Route::new(200, "application/xml", format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{b}/</loc><lastmod>2026-11-01</lastmod></url><url><loc>{b}/about</loc></url><url><loc>{b}/contact</loc></url></urlset>"#)));
    s
}

#[test]
fn broken_site_fails_the_build_and_writes_every_report() {
    let srv = Server::start(|b, _, _| {
        let mut s = Site::new();
        s.insert(
            "/".into(),
            Doc::good(b, "/", "Broken home")
                .drop("<title")
                .body(r#"<a href="/missing">Missing toad page</a>"#)
                .route(),
        );
        s
    });
    let dir = out_dir("broken");
    let url = srv.url("/");
    let (code, stdout, stderr) = run(&[
        "crawl",
        &url,
        "--out",
        dir.to_str().unwrap(),
        "--fail-on",
        "high",
        "--quiet",
    ]);
    assert_eq!(
        code, 2,
        "--fail-on high must fail on a broken site\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains("Health score"), "{stdout}");
    for f in FILES {
        let meta = std::fs::metadata(dir.join(f)).unwrap_or_else(|_| panic!("{f} was not written"));
        assert!(meta.len() > 0, "{f} is empty");
    }
    let crawl = Crawl::load(&dir.join("crawl.shoutingtoad")).expect("saved crawl reloads");
    assert!(crawl.pages.iter().any(|p| p.url == url && p.status == 200));
    assert!(crawl
        .pages
        .iter()
        .any(|p| p.url == srv.url("/missing") && p.status == 404));
    let issues = std::fs::read_to_string(dir.join("issues.csv")).unwrap();
    assert!(
        issues.contains("titles.missing") && issues.contains("response.internal_404"),
        "{issues}"
    );
    let urls = std::fs::read_to_string(dir.join("issue_urls.csv")).unwrap();
    assert!(
        urls.lines()
            .any(|l| l.starts_with("response.internal_404,") && l.ends_with(&srv.url("/missing"))),
        "{urls}"
    );
    let report = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(
        report.contains("Page Titles") && report.contains(&srv.url("/missing")),
        "report.html lists the findings"
    );

    let (code, ..) = run(&[
        "crawl",
        &url,
        "--out",
        dir.to_str().unwrap(),
        "--fail-on-check",
        "titles.missing",
        "--quiet",
    ]);
    assert_eq!(code, 2, "--fail-on-check fails when the check fires");
    let (code, ..) = run(&[
        "crawl",
        &url,
        "--out",
        dir.to_str().unwrap(),
        "--fail-on-check",
        "no.such.check",
        "--quiet",
    ]);
    assert_eq!(code, 1, "unknown check ids are an error");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clean_site_passes_and_reports_nothing_but_plain_http() {
    let srv = Server::start(|b, _, _| clean_site(b));
    let dir = out_dir("clean");
    let url = srv.url("/");
    let (code, stdout, stderr) = run(&[
        "crawl",
        &url,
        "--out",
        dir.to_str().unwrap(),
        "--fail-on-check",
        "titles.missing",
        "--fail-on-check",
        "response.internal_404",
        "--quiet",
    ]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let crawl = Crawl::load(&dir.join("crawl.shoutingtoad")).expect("saved crawl reloads");
    let a = Audited::new(crawl, &srv.base);
    for path in ["/", "/about", "/contact"] {
        a.expect(
            a.find(path).is_some_and(|p| p.status == 200),
            &format!("{path} crawled"),
        );
        a.only(path, &["security.http_url", "performance.no_http2"]);
    }
    a.expect(
        a.triggered()
            .iter()
            .all(|id| id == "security.http_url" || id == "performance.no_http2"),
        &format!("clean site triggered {:?}", a.triggered()),
    );
    a.finish();

    // security.http_url is a High issue, so --fail-on high always fails a plain-HTTP site.
    let (code, ..) = run(&[
        "crawl",
        &url,
        "--out",
        dir.to_str().unwrap(),
        "--fail-on",
        "high",
        "--quiet",
    ]);
    assert_eq!(code, 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_command_and_misc_commands() {
    let srv = Server::start(|b, _, _| clean_site(b));
    let dir = out_dir("list");
    std::fs::create_dir_all(&dir).unwrap();
    let list = dir.join("urls.txt");
    std::fs::write(&list, format!("# comment\n{}\n\n", srv.url("/about"))).unwrap();
    let out = dir.join("out");
    let (code, stdout, stderr) = run(&[
        "list",
        list.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--quiet",
    ]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let crawl = Crawl::load(&out.join("crawl.shoutingtoad")).unwrap();
    assert!(crawl.pages.iter().any(|p| p.url == srv.url("/about")));
    assert!(!srv.requested("/contact"), "list mode follows no links");

    let (code, stdout, _) = run(&["checks"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.lines().count(), catalogue().len() + 1);
    assert!(stdout.contains("titles.missing"));
    let (code, stdout, _) = run(&["--version"]);
    assert!(code == 0 && stdout.contains(env!("CARGO_PKG_VERSION")));
    assert_eq!(run(&["crawl"]).0, 1, "crawl without a URL is an error");
    assert_eq!(
        run(&["crawl", &srv.url("/"), "--bogus"]).0,
        1,
        "unknown options are an error"
    );
    assert_eq!(run(&["frobnicate"]).0, 1);
    let _ = std::fs::remove_dir_all(&dir);
}
