//! AI search readiness end to end: a fixture server that treats AI crawlers differently by user
//! agent, robots.txt groups per bot, bot-specific noindex, a JavaScript-only page and pages with
//! good and poor quotable sections.

mod support;

use shouting_toad::ai_search::{Outcome, BOTS};
use shouting_toad::crawler::{self, Event};
use shouting_toad::model::Crawl;
use std::time::{Duration, Instant};
use support::*;

const GOOD_SECTIONS: &str = r#"<h2>How long do toads live?</h2>
<p>Toads live 10 to 12 years in the wild and up to 40 years in captivity, according to the Toad Trust in Bristol. The oldest recorded common toad lived for 40 years. Lifespan depends on diet, predators and the temperature of the pond where the toad spends the winter.</p>
<h2>What do toads eat?</h2>
<p>Toads eat insects, worms and slugs, catching up to 1,000 insects in a single summer night. Gardeners in Kent count on them to control slugs without pellets. Young toads eat smaller prey such as mites and springtails until they grow to about 2 centimetres long.</p>
<h2>Where do toads breed?</h2>
<p>Toads breed in deep ponds and lakes, returning each March to the same water where they hatched. A female lays up to 5,000 eggs in long strings wrapped around reeds. Migration routes cross roads, so volunteers in Surrey run toad patrols every spring evening.</p>"#;

fn poor_sections() -> String {
    ["Feeding", "Housing", "Handling"]
        .iter()
        .map(|h| format!("<h2>{h}</h2><p>This is something every keeper asks about sooner or later, and it is not simple. there are many opinions and we will look at them in turn so that you can decide what works for you and your pets at home.</p>"))
        .collect()
}

fn page(base: &str, path: &str, topic: &str, body: &str) -> Route {
    Doc::good(base, path, topic)
        .drop("<h2")
        .drop("<p")
        .body(body)
        .route()
}

/// A site whose firewall treats several AI crawlers differently.
fn hostile_site(base: &str) -> Site {
    let mut s = Site::new();
    s.insert(
        "/robots.txt".into(),
        Route::new(
            200,
            "text/plain",
            "User-agent: GPTBot\nDisallow: /\n\nUser-agent: Claude-SearchBot\nDisallow: /\n\nUser-agent: *\nDisallow:\n",
        ),
    );
    let links = r#"<p><a href="/guide">Toad guide</a> <a href="/poor">Keeping toads</a> <a href="/good">Toad facts</a> <a href="/app">Toad app</a> <a href="/faq">Toad questions</a></p>"#;
    let home = Doc::good(base, "/", "Toads")
        .body(links)
        .route()
        .for_ua(
            "OAI-SearchBot",
            Route::new(403, "text/html", "<h1>Forbidden</h1>").header("Server", "cloudflare"),
        )
        .for_ua(
            "PerplexityBot",
            Route::new(
                403,
                "text/html",
                "<html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt={}</script></body></html>",
            )
            .header("cf-mitigated", "challenge"),
        )
        .for_ua("ChatGPT-User", Route::new(403, "text/html", "Forbidden"))
        .for_ua("CCBot", Route::new(403, "text/html", "Forbidden"));
    s.insert("/".into(), home);
    let guide = Doc::good(base, "/guide", "Toad guide")
        .head(r#"<meta name="GPTBot" content="noindex">"#)
        .route()
        .for_ua(
            "Applebot",
            html("<html><body><p>Hello toad</p></body></html>"),
        );
    s.insert("/guide".into(), guide);
    s.insert(
        "/poor".into(),
        page(base, "/poor", "Keeping toads", &poor_sections()),
    );
    s.insert(
        "/good".into(),
        page(base, "/good", "Toad facts", GOOD_SECTIONS),
    );
    let app = Doc::good(base, "/app", "Toad app")
        .drop("<h2")
        .drop("<p")
        .body(r#"<div id="root"></div><script src="/a.js"></script><script src="/b.js"></script><script src="/c.js"></script>"#);
    s.insert("/app".into(), app.route());
    for js in ["/a.js", "/b.js", "/c.js"] {
        s.insert(js.into(), file("application/javascript", "1"));
    }
    s.insert(
        "/faq".into(),
        Doc::good(base, "/faq", "Toad questions")
            .route()
            .header("X-Robots-Tag", "ClaudeBot: noindex"),
    );
    s
}

fn ai_hits(a: &Audited) -> Vec<String> {
    a.triggered()
        .into_iter()
        .filter(|id| id.starts_with("ai."))
        .collect()
}

#[test]
fn firewall_robots_noindex_js_and_quotability_findings() {
    let srv = Server::start(|base, _, _| hostile_site(base));
    let crawl = crawl(&[srv.url("/")], false, config());
    let a = Audited::new(crawl, &srv.base);
    let r = a.crawl.site.ai_bots.clone().expect("AI access test ran");

    // start page first, then the most-linked indexable pages
    let urls: Vec<&str> = r.pages.iter().map(|p| p.url.as_str()).collect();
    a.expect(
        urls == [
            a.url("/"),
            a.url("/app"),
            a.url("/good"),
            a.url("/guide"),
            a.url("/poor"),
        ],
        &format!("tested pages {urls:?}"),
    );
    a.expect(
        r.rows.len() == BOTS.len() * r.pages.len(),
        "one row per bot and page",
    );
    let row = |bot: &str, page: usize| {
        r.rows
            .iter()
            .find(|x| x.bot == bot && x.page == page)
            .unwrap()
            .clone()
    };
    let home = |bot: &str| row(bot, 0);
    a.expect(
        home("OAI-SearchBot").outcome == Outcome::Blocked,
        "OAI-SearchBot blocked",
    );
    a.expect(
        home("OAI-SearchBot").detail.contains("Cloudflare"),
        "block attributed to Cloudflare",
    );
    a.expect(
        home("PerplexityBot").outcome == Outcome::Challenged,
        "PerplexityBot challenged",
    );
    a.expect(
        home("ChatGPT-User").outcome == Outcome::Blocked,
        "ChatGPT-User blocked",
    );
    a.expect(home("CCBot").outcome == Outcome::Blocked, "CCBot blocked");
    a.expect(
        home("ClaudeBot").outcome == Outcome::Ok,
        "ClaudeBot gets the page",
    );
    let gpt = home("GPTBot");
    a.expect(
        !gpt.robots_allowed && gpt.outcome == Outcome::NotFetched,
        "GPTBot disallowed and not fetched",
    );
    a.expect(
        home("Google-Extended").outcome == Outcome::NotFetched,
        "robots-only token not fetched",
    );
    a.expect(
        row("Applebot", 3).outcome == Outcome::LessContent,
        "Applebot thin /guide",
    );
    a.expect(
        row("ClaudeBot", 3).outcome == Outcome::Ok,
        "ClaudeBot full /guide",
    );
    a.expect(
        srv.requests_by("GPTBot") == 0,
        "GPTBot obeys robots.txt: never requested",
    );
    a.expect(
        srv.requests_by("ClaudeBot/") == r.pages.len(),
        "ClaudeBot fetched every tested page",
    );
    a.expect(
        r.robots_found && !r.llms_txt_ok && r.llms_txt_status == 404,
        "llms.txt missing",
    );
    a.expect(r.pages[0].words > 100, "raw HTML words recorded");

    a.hit("ai.search_bot_firewall_blocked", "/");
    a.hit("ai.search_bot_robots_blocked", "/");
    a.hit("ai.user_fetcher_blocked", "/");
    a.hit("ai.training_bot_firewall_only", "/");
    a.hit("ai.different_content", "/guide");
    a.hit("ai.bot_noindex", "/guide");
    a.hit("ai.bot_noindex", "/faq");
    a.hit("ai.js_only_content", "/app");
    a.hit("ai.low_quotability", "/poor");
    a.no_hit("ai.low_quotability", "/good");
    a.no_hit("ai.js_only_content", "/good");
    a.no_hit("ai.bot_noindex", "/good");
    a.no_hit("ai.search_bot_firewall_blocked", "/good");
    a.no_hit("ai.training_all_blocked", "/");
    let good = a.page("/good").html.as_ref().unwrap();
    a.expect(
        (good.ai.sections, good.ai.quotable) == (3, 3),
        &format!("good page sections {:?}", good.ai),
    );

    // exports
    let csv = shouting_toad::report::ai_bots_csv(&a.crawl);
    a.expect(
        csv.lines().count() == 1 + r.rows.len(),
        "ai_bots.csv has a row per result",
    );
    a.expect(
        csv.contains("OAI-SearchBot,OpenAI,Search / answers"),
        "csv bot columns",
    );
    let html = shouting_toad::report::html_report(&a.crawl, &a.audit, "", "");
    a.expect(
        html.contains("AI search readiness") && html.contains("Challenge 403"),
        "report section",
    );
    let line = shouting_toad::ai_search::summary(&r);
    a.expect(line.contains("OAI-SearchBot (blocked 403)"), &line);
    a.finish();
}

#[test]
fn training_opt_out_and_clean_sites() {
    let robots = "User-agent: GPTBot\nUser-agent: ClaudeBot\nUser-agent: Google-Extended\nUser-agent: Applebot-Extended\nUser-agent: CCBot\nUser-agent: Meta-ExternalAgent\nUser-agent: Bytespider\nUser-agent: Amazonbot\nDisallow: /\n";
    let srv = Server::start(|base, _, _| {
        let mut s = Site::new();
        s.insert("/robots.txt".into(), Route::new(200, "text/plain", robots));
        s.insert(
            "/llms.txt".into(),
            Route::new(200, "text/plain", "# Toads\n> A toad site."),
        );
        s.insert("/".into(), Doc::good(base, "/", "Toads").route());
        s
    });
    let a = Audited::new(crawl(&[srv.url("/")], false, config()), &srv.base);
    a.hit("ai.training_all_blocked", "/");
    a.expect(
        ai_hits(&a) == ["ai.training_all_blocked"],
        &format!("{:?}", ai_hits(&a)),
    );
    let r = a.crawl.site.ai_bots.as_ref().unwrap();
    a.expect(r.llms_txt_ok, "llms.txt found");
    a.finish();

    // No robots.txt, nothing special: no AI Search finding at all, every bot gets the page.
    let srv = Server::start(|base, _, _| {
        let mut s = Site::new();
        s.insert(
            "/".into(),
            Doc::good(base, "/", "Toads")
                .body(r#"<a href="/b">More toads</a>"#)
                .route(),
        );
        s.insert("/b".into(), Doc::good(base, "/b", "More toads").route());
        s
    });
    let a = Audited::new(crawl(&[srv.url("/")], false, config()), &srv.base);
    a.expect(
        ai_hits(&a).is_empty(),
        &format!("clean site: {:?}", ai_hits(&a)),
    );
    let r = a.crawl.site.ai_bots.as_ref().unwrap();
    a.expect(r.pages.len() == 2, "home and /b tested");
    a.expect(
        r.rows
            .iter()
            .all(|x| matches!(x.outcome, Outcome::Ok | Outcome::NotFetched) && x.robots_allowed),
        "every bot allowed and served",
    );
    a.finish();
}

#[test]
fn can_be_turned_off() {
    let srv = Server::start(|base, _, _| hostile_site(base));
    let mut cfg = config();
    cfg.ai_bots = false;
    let c = crawl(&[srv.url("/")], false, cfg);
    assert!(c.site.ai_bots.is_none());
    assert_eq!(srv.requests_by("ClaudeBot"), 0);
    assert!(!srv.requested("/llms.txt"));
}

#[test]
fn stop_is_prompt_during_the_access_test() {
    let srv = Server::start(|base, _, _| {
        let mut s = Site::new();
        let slow = Route::new(200, "text/html", "<html><body>slow</body></html>").delay(3000);
        s.insert(
            "/".into(),
            Doc::good(base, "/", "Toads")
                .route()
                .for_ua("Bot", slow.clone())
                .for_ua("User", slow),
        );
        s
    });
    let h = crawler::start(vec![srv.url("/")], false, config()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stopped: Option<Instant> = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match h.rx.recv_timeout(left).expect("crawl finished") {
            Event::Log(m) if m.starts_with("Testing AI crawler") => {
                std::thread::sleep(Duration::from_millis(300));
                h.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                stopped = Some(Instant::now());
            }
            Event::Site(s) => assert!(s.ai_bots.is_none(), "a stopped test reports nothing"),
            Event::Finished => break,
            _ => {}
        }
    }
    let took = stopped.expect("the access test started").elapsed();
    assert!(took < Duration::from_secs(2), "stop took {took:?}");
}

#[test]
fn crawl_files_without_ai_fields_still_load() {
    let srv = Server::start(|base, _, _| hostile_site(base));
    let c = crawl(&[srv.url("/")], false, config());
    let mut v = serde_json::to_value(&c).unwrap();
    v["site"].as_object_mut().unwrap().remove("ai_bots");
    for p in v["pages"].as_array_mut().unwrap() {
        if let Some(h) = p["html"].as_object_mut() {
            h.remove("ai");
        }
    }
    let path = std::env::temp_dir().join(format!("st-ai-old-{}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let back = Crawl::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(back.site.ai_bots.is_none());
    assert_eq!(back.pages.len(), c.pages.len());
    shouting_toad::audit::run(&back);
}
