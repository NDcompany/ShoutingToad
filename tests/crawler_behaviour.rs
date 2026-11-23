//! Crawler configuration end to end: robots.txt obedience, list mode, limits,
//! include/exclude, custom search and extraction, nofollow and host handling.

mod support;

use shouting_toad::crawler::Config;
use shouting_toad::model::{Crawl, Page, Source};
use support::*;

/// A good page whose body links to `links`.
fn linking(b: &str, path: &str, links: &[&str]) -> Doc {
    let anchors: String = links.iter().map(|l| format!(r#"<a href="{l}">Go to {l}</a> "#)).collect();
    Doc::good(b, path, &format!("Page {path}")).drop("<nav").body(format!("<nav>{anchors}</nav>"))
}

fn site(b: &str, pages: &[(&str, Doc)], extra: Vec<(&str, Route)>) -> Site {
    let mut s: Site = pages.iter().map(|(p, d)| (p.to_string(), d.route())).collect();
    s.extend(extra.into_iter().map(|(p, r)| (p.to_string(), r)));
    let _ = b;
    s
}

fn get<'a>(c: &'a Crawl, srv: &Server, path: &str) -> Option<&'a Page> {
    let u = srv.url(path);
    c.pages.iter().find(|p| p.url == u)
}

#[test]
fn robots_txt_is_obeyed_unless_ignored() {
    let build = |b: &str| {
        site(b, &[
            ("/", linking(b, "/", &["/secret/page", "/toad-only/page", "/all/page"])),
            ("/secret/page", linking(b, "/secret/page", &["/"])),
            ("/toad-only/page", linking(b, "/toad-only/page", &["/"])),
            ("/all/page", linking(b, "/all/page", &["/"])),
        ], vec![("/robots.txt", Route::new(200, "text/plain",
            "User-agent: *\nDisallow: /all/\n\nUser-agent: ShoutingToad\nDisallow: /secret/\nDisallow: /toad-only/\n"))])
    };
    let srv = Server::start(|b, _, _| build(b));
    let c = crawl(&[srv.url("/")], false, config());
    for path in ["/secret/page", "/toad-only/page"] {
        let p = get(&c, &srv, path).expect("blocked URLs are still reported");
        assert!(p.blocked_by_robots && p.status == 0, "{path} should be blocked: {p:?}");
        assert!(!srv.requested(path), "{path} must not be fetched");
    }
    // The ShoutingToad group replaces the * group, so /all/ is allowed for this crawler.
    assert!(srv.requested("/all/page") && get(&c, &srv, "/all/page").is_some_and(|p| p.status == 200));

    let srv = Server::start(|b, _, _| build(b));
    let c = crawl(&[srv.url("/")], false, Config { respect_robots: false, ..config() });
    for path in ["/secret/page", "/toad-only/page"] {
        assert!(srv.requested(path), "{path} must be fetched when robots.txt is ignored");
        assert!(get(&c, &srv, path).is_some_and(|p| p.status == 200 && !p.blocked_by_robots));
    }
}

#[test]
fn list_mode_audits_only_the_listed_urls() {
    let srv = Server::start(|b, _, _| {
        site(b, &[
            ("/", linking(b, "/", &["/a"])),
            ("/a", linking(b, "/a", &["/b", "/c"]).head(r#"<link rel="next" href="/d">"#)),
            ("/b", linking(b, "/b", &["/c"]).canonical(&format!("{b}/e"))),
            ("/c", linking(b, "/c", &[])),
        ], vec![("/sitemap.xml", Route::new(200, "application/xml", format!(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{b}/c</loc></url></urlset>"#)))])
    });
    let c = crawl(&[srv.url("/a"), srv.url("/b")], true, config());
    let mut pages: Vec<&str> = c.pages.iter().filter(|p| p.source != Some(Source::Robots)).map(|p| p.url.as_str()).collect();
    pages.sort();
    assert_eq!(pages, vec![srv.url("/a"), srv.url("/b")], "list mode crawls exactly the listed URLs");
    assert!(c.pages.iter().filter(|p| p.source != Some(Source::Robots)).all(|p| p.source == Some(Source::Seed) && p.status == 200));
    for path in ["/", "/c", "/d", "/e", "/sitemap.xml"] {
        assert!(!srv.requested(path), "list mode must not fetch {path}");
    }
    assert!(!c.site.sitemaps_checked);
}

#[test]
fn max_urls_and_max_depth_limit_the_crawl() {
    let fan: Vec<String> = (1..=20).map(|i| format!("/p{i}")).collect();
    let srv = Server::start(|b, _, _| {
        let refs: Vec<&str> = fan.iter().map(|s| s.as_str()).collect();
        let mut pages = vec![("/", linking(b, "/", &refs))];
        pages.extend(fan.iter().map(|p| (p.as_str(), linking(b, p, &["/"]))));
        site(b, &pages, vec![])
    });
    let c = crawl(&[srv.url("/")], false, Config { max_urls: 6, ..config() });
    assert!((3..=6).contains(&c.pages.len()), "max_urls=6 crawled {} URLs", c.pages.len());
    let fetched = fan.iter().filter(|p| srv.requested(p)).count();
    assert!(fetched <= 5, "max_urls=6 fetched {fetched} of the linked pages");

    let srv = Server::start(|b, _, _| {
        site(b, &[
            ("/", linking(b, "/", &["/d1"])),
            ("/d1", linking(b, "/d1", &["/d2"])),
            // rel=next is followed like a link, so it must respect the depth limit too.
            ("/d2", linking(b, "/d2", &["/d3"]).head(r#"<link rel="next" href="/d3-next">"#)),
            ("/d3", linking(b, "/d3", &["/d4"])),
            ("/d3-next", linking(b, "/d3-next", &[])),
        ], vec![])
    });
    let c = crawl(&[srv.url("/")], false, Config { max_depth: 2, ..config() });
    assert!(get(&c, &srv, "/d2").is_some_and(|p| p.depth == 2 && p.status == 200));
    for path in ["/d3", "/d3-next", "/d4"] {
        assert!(!srv.requested(path), "max_depth=2 must not fetch {path}");
    }
}

#[test]
fn include_and_exclude_patterns_filter_internal_urls() {
    let build = |b: &str| {
        site(b, &[
            ("/", linking(b, "/", &["/keep", "/skip-me", "/other"])),
            ("/keep", linking(b, "/keep", &["/keep/child"]).head(r#"<link rel="next" href="/skip-next">"#).head(hreflang_tag(b, "/skip-alt"))),
            ("/keep/child", linking(b, "/keep/child", &[])),
            ("/skip-me", linking(b, "/skip-me", &[])),
            ("/skip-next", linking(b, "/skip-next", &[])),
            ("/skip-alt", linking(b, "/skip-alt", &[])),
            ("/other", linking(b, "/other", &[])),
            ("/skip-sitemap", linking(b, "/skip-sitemap", &[])),
            ("/keep-sitemap", linking(b, "/keep-sitemap", &[])),
        ], vec![("/sitemap.xml", Route::new(200, "application/xml", format!(
            r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{b}/skip-sitemap</loc></url><url><loc>{b}/keep-sitemap</loc></url></urlset>"#)))])
    };
    let srv = Server::start(|b, _, _| build(b));
    crawl(&[srv.url("/")], false, Config { exclude: "/skip".into(), ..config() });
    for path in ["/skip-me", "/skip-next", "/skip-alt", "/skip-sitemap"] {
        assert!(!srv.requested(path), "exclude must stop {path} being fetched");
    }
    assert!(srv.requested("/keep") && srv.requested("/other") && srv.requested("/keep-sitemap"));

    let srv = Server::start(|b, _, _| build(b));
    crawl(&[srv.url("/")], false, Config { include: "/keep".into(), ..config() });
    assert!(srv.requested("/"), "the start URL is always crawled");
    assert!(srv.requested("/keep") && srv.requested("/keep/child") && srv.requested("/keep-sitemap"));
    for path in ["/other", "/skip-me", "/skip-next", "/skip-alt", "/skip-sitemap"] {
        assert!(!srv.requested(path), "include=/keep must not fetch {path}");
    }
}

fn hreflang_tag(b: &str, path: &str) -> String {
    format!(r#"<link rel="alternate" hreflang="de" href="{b}{path}">"#)
}

#[test]
fn custom_search_and_extraction() {
    let srv = Server::start(|b, _, _| site(b, &[("/", Doc::good(b, "/", "Custom page"))], vec![]));
    let cfg = Config { custom_search: "Toad Shop".into(), custom_extract: "<h1>(.*?)</h1>".into(), ..config() };
    let c = crawl(&[srv.url("/")], false, cfg);
    let p = get(&c, &srv, "/").unwrap();
    assert!(p.custom_search_hits >= 3, "custom search hits: {}", p.custom_search_hits);
    assert_eq!(p.custom_extract, vec!["Custom page explained".to_string()]);
    assert!(shouting_toad::crawler::start(vec![srv.url("/")], false, Config { custom_search: "(".into(), ..config() }).is_err(), "invalid regex is rejected");
}

#[test]
fn nofollow_external_and_resource_options() {
    let build = |b: &str, e: &str| {
        site(b, &[(
            "/",
            Doc::good(b, "/", "Options page")
                .body(r#"<a href="/nf" rel="nofollow">Nofollowed page</a>"#)
                .body(format!(r#"<a href="{e}/ext-only">External page</a>"#))
                .body(r#"<img src="/pic.png" alt="A picture" width="1" height="1">"#),
        ), ("/nf", Doc::good(b, "/nf", "Nofollowed")), ("/ext-only", Doc::good(b, "/ext-only", "External only"))],
        vec![("/pic.png", file("image/png", vec![0u8; 10]))])
    };
    let srv = Server::start(|b, e, _| build(b, e));
    let c = crawl(&[srv.url("/")], false, config());
    assert!(!srv.requested("/nf"), "nofollow links are not followed by default");
    assert!(srv.requested("/ext-only") && srv.requested("/pic.png"));
    let ext = c.pages.iter().find(|p| p.url == format!("{}/ext-only", srv.ext)).expect("external link is checked");
    assert!(!ext.is_internal && ext.html.is_none() && ext.status == 200, "external URLs are status-checked only");

    let srv = Server::start(|b, e, _| build(b, e));
    crawl(&[srv.url("/")], false, Config { follow_nofollow: true, check_external: false, check_resources: false, ..config() });
    assert!(srv.requested("/nf"), "follow_nofollow follows nofollow links");
    assert!(!srv.requested("/ext-only"), "check_external=false skips external links");
    assert!(!srv.requested("/pic.png"), "check_resources=false skips images");
}

#[test]
fn hosts_are_compared_without_port() {
    let other = Server::start(|b, _, _| site(b, &[("/elsewhere", Doc::good(b, "/elsewhere", "Other port page"))], vec![]));
    let other_url = other.url("/elsewhere");
    let srv = Server::start(|b, e, _| site(b, &[("/", linking(b, "/", &[&other_url, &format!("{e}/x")]))], vec![("/x", html("<p>x</p>"))]));
    let c = crawl(&[srv.url("/")], false, config());
    let same_host = c.pages.iter().find(|p| p.url == other_url).expect("other-port URL crawled");
    assert!(same_host.is_internal && same_host.html.is_some(), "same host on another port is internal");
    let localhost = c.pages.iter().find(|p| p.url == format!("{}/x", srv.ext)).expect("localhost URL checked");
    assert!(!localhost.is_internal, "a different host name is external");
}
