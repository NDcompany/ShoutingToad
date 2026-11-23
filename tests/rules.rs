//! Rule packs end to end: a fixture site crawled with tests/fixtures/fixture-pack.toml,
//! which has one rule per condition type. Rule findings must show up like built-in checks:
//! audit hits, categories, reports, CSV, the CLI summary and --fail-on-check.

mod support;

use shouting_toad::checks::{builtin, catalogue, categories};
use shouting_toad::model::Crawl;
use shouting_toad::{audit, report, rules};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use support::*;

fn pack_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fixture-pack.toml")
}

/// Every test in this binary shares the catalogue: install the same pack once.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let (packs, problems) = rules::load_paths(&[pack_path()]);
        assert!(problems.is_empty(), "{problems:?}");
        rules::install(&packs);
    });
}

fn site(b: &str) -> Site {
    let good = |path: &str| Doc::good(b, path, &format!("Toad page {path}"));
    let mut s = Site::new();
    let mut add = |path: &str, r: Route| {
        s.insert(path.to_string(), r);
    };
    let pages = [
        "/perfect",
        "/promo",
        "/nomain",
        "/many-h2",
        "/free",
        "/paid",
        "/wp",
        "/legacy",
        "/php",
        "/noxfo",
        "/oldapache",
        "/gone",
        "/list?sessionid=1",
        "/doc.pdf",
        "/thin",
        "/longtitle",
        "/nodesc",
        "/noh1",
        "/slow",
        "/big",
        "/deep/1",
        "/hub",
        "/gallery",
        "/combo-yes",
        "/combo-no",
        "/noindexed",
    ];
    let mut home = good("/");
    for p in pages {
        home = home.body(format!(
            r#"<p><a href="{p}">Visit the toad page at {p}</a></p>"#
        ));
    }
    add("/", home.route());
    add("/perfect", good("/perfect").route());
    add(
        "/promo",
        good("/promo")
            .body(r#"<div class="promo">Big toad sale</div>"#)
            .route(),
    );
    let mut nomain = good("/nomain");
    nomain.main = false;
    add("/nomain", nomain.route());
    add(
        "/many-h2",
        good("/many-h2")
            .body("<h2>Second toad heading</h2><h2>Third toad heading</h2>")
            .route(),
    );
    add(
        "/free",
        good("/free").body(r#"<p class="price">$0.00</p>"#).route(),
    );
    add(
        "/paid",
        good("/paid").body(r#"<p class="price">$10.00</p>"#).route(),
    );
    add(
        "/wp",
        good("/wp")
            .head(r#"<meta name="generator" content="WordPress 6.4.2">"#)
            .route(),
    );
    add(
        "/legacy",
        good("/legacy")
            .body("<!-- BEGIN-LEGACY-TRACKER -->")
            .route(),
    );
    add(
        "/php",
        good("/php").route().header("X-Powered-By", "PHP/8.1"),
    );
    add("/noxfo", good("/noxfo").route().set("X-Frame-Options", ""));
    add(
        "/oldapache",
        good("/oldapache")
            .route()
            .header("Server", "Apache/2.2.15 (CentOS)"),
    );
    add("/gone", error(410));
    add("/list", good("/list?sessionid=1").route());
    add(
        "/doc.pdf",
        file("application/pdf", b"%PDF-1.4\n%%EOF\n".to_vec()),
    );
    add("/thin", good("/thin").drop("<p class=\"prose\"").route());
    add(
        "/longtitle",
        good("/longtitle")
            .title("A very long toad page title that goes on and on well past seventy characters")
            .route(),
    );
    add(
        "/nodesc",
        good("/nodesc").drop("<meta name=\"description\"").route(),
    );
    add("/noh1", good("/noh1").drop("<h1").route());
    add("/slow", good("/slow").route().delay(600));
    add(
        "/big",
        good("/big")
            .body(format!("<!-- {} -->", "x".repeat(120_000)))
            .route(),
    );
    add(
        "/deep/1",
        good("/deep/1")
            .body(r#"<a href="/deep/2">Deeper toad page two</a>"#)
            .route(),
    );
    add(
        "/deep/2",
        good("/deep/2")
            .body(r#"<a href="/deep/3">Deeper toad page three</a>"#)
            .route(),
    );
    add("/deep/3", good("/deep/3").route());
    let mut hub = good("/hub");
    for i in 0..25 {
        hub = hub.body(format!(r#"<a href="/perfect">Perfect toad guide {i}</a>"#));
    }
    add("/hub", hub.route());
    let mut gallery = good("/gallery");
    for i in 0..3 {
        gallery = gallery.body(format!(
            r#"<img src="/img/{i}.png" alt="Toad photo {i}" width="10" height="10">"#
        ));
        add(
            &format!("/img/{i}.png"),
            file("image/png", b"\x89PNG\r\n\x1a\n".to_vec()),
        );
    }
    add("/gallery", gallery.route());
    add(
        "/combo-yes",
        good("/combo-yes").body("<h6>Small toad note</h6>").route(),
    );
    add(
        "/combo-no",
        good("/combo-no")
            .body("<h6>Small toad note</h6><table><tr><th>Toad</th></tr></table>")
            .route(),
    );
    add(
        "/noindexed",
        good("/noindexed").robots("noindex, follow").route(),
    );
    s
}

fn crawled() -> (Server, Audited) {
    install();
    let srv = Server::start(|b, _, _| site(b));
    let mut cfg = config();
    cfg.check_external = false;
    let c = crawl(&[srv.url("/")], false, cfg);
    let a = Audited::new(c, &srv.base);
    (srv, a)
}

#[test]
fn every_condition_type_fires_where_it_should() {
    let (_srv, a) = crawled();
    let pairs: &[(&str, &str, &str)] = &[
        // (rule, fires on, must not fire on)
        ("sel-exists", "/promo", "/"),
        ("sel-missing", "/nomain", "/promo"),
        ("sel-count", "/many-h2", "/"),
        ("sel-text", "/free", "/paid"),
        ("sel-attr", "/wp", "/"),
        ("html-regex", "/legacy", "/"),
        ("header-exists", "/php", "/"),
        ("header-missing", "/noxfo", "/"),
        ("header-regex", "/oldapache", "/php"),
        ("status", "/gone", "/"),
        ("url", "/list?sessionid=1", "/"),
        ("content-type", "/doc.pdf", "/"),
        ("word-count", "/thin", "/"),
        ("title-length", "/longtitle", "/"),
        ("description-length", "/nodesc", "/"),
        ("h1-count", "/noh1", "/"),
        ("response-ms", "/slow", "/promo"),
        ("size", "/big", "/"),
        ("depth", "/deep/3", "/deep/2"),
        ("inlinks", "/", "/promo"),
        ("outlinks", "/hub", "/promo"),
        ("images", "/gallery", "/"),
        ("combo", "/combo-yes", "/combo-no"),
        ("scope-images", "/img/0.png", "/gallery"),
    ];
    for (rule, yes, no) in pairs {
        let id = format!("fixture/{rule}");
        a.hit(&id, yes);
        a.no_hit(&id, no);
    }
    a.no_hit("fixture/combo", "/promo");
    a.no_hit("fixture/scope-indexable", "/noindexed");
    a.expect(a.hits("fixture/never").is_empty(), "fixture/never fired");
    // Selector tests only run on HTML: they are not "missing" on images or PDFs.
    a.no_hit("fixture/sel-missing", "/doc.pdf");
    a.no_hit("fixture/sel-missing", "/img/0.png");
    a.finish();
}

#[test]
fn rule_checks_join_the_catalogue_after_the_built_in_ones() {
    install();
    let n = builtin().len();
    assert!(n > 400, "built-in checks: {n}");
    assert_eq!(catalogue().len(), n + 26);
    assert_eq!(&catalogue()[..n].len(), &n);
    assert!(catalogue()[..n].iter().all(|c| !c.id.contains('/')));
    assert_eq!(catalogue()[n].id, "fixture/sel-exists");
    let cats = categories();
    assert!(cats.contains(&"Fixture Checks"), "{cats:?}");
    // A rule without a category goes under the pack name; built-in names are reused.
    assert!(cats.contains(&"Fixture"));
    assert_eq!(cats.iter().filter(|c| **c == "Security").count(), 1);
}

#[test]
fn reports_and_exports_include_rule_findings() {
    let (_srv, a) = crawled();
    let html = report::html_report(&a.crawl, &a.audit, "", "");
    assert!(html.contains("Promo box present"));
    assert!(html.contains("Fixture Checks"));
    assert!(html.contains("Remove the promo box."));
    let issues = report::issues_csv(&a.crawl, &a.audit);
    assert!(issues.contains("fixture/sel-exists,Fixture Checks,Promo box present,Issue,High"));
    let urls = report::issue_urls_csv(&a.crawl, &a.audit);
    assert!(urls.contains(&format!(
        "fixture/sel-exists,Fixture Checks,Promo box present,Issue,High,{}",
        a.url("/promo")
    )));
    // Rules cost health points like built-in checks.
    let base = a.audit.health_score(&a.crawl);
    let mut without = a.audit.clone();
    for h in &mut without.hits[builtin().len()..] {
        h.clear();
    }
    assert!(without.health_score(&a.crawl) > base);
}

#[test]
fn saved_crawls_keep_rule_results() {
    let (_srv, a) = crawled();
    let path = std::env::temp_dir().join(format!("st-rules-{}.shoutingtoad", std::process::id()));
    a.crawl.save(&path).unwrap();
    let back = Crawl::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.site.rule_probes, a.crawl.site.rule_probes);
    assert!(!back.site.rule_probes.is_empty());
    let again = Audited::new(back, &a.base);
    again.hit("fixture/sel-exists", "/promo");
    again.hit("fixture/html-regex", "/legacy");
    again.finish();
    // A crawl made without the pack's probes cannot answer selector rules: they stay silent
    // instead of reporting every page as "missing".
    let mut old = a.crawl.clone();
    old.site.rule_probes.clear();
    let audit = audit::run(&old);
    let idx = |id: &str| catalogue().iter().position(|c| c.id == id).unwrap();
    assert!(audit.hits[idx("fixture/sel-missing")].is_empty());
    assert!(
        !audit.hits[idx("fixture/url")].is_empty(),
        "non-selector rules still run"
    );
}

// ------------------------------------------------------------------ CLI

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

fn out_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("st-rules-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn cli_reports_rules_and_fails_on_rule_checks() {
    let srv = Server::start(|b, _, _| site(b));
    let pack = pack_path();
    let pack = pack.to_str().unwrap();
    let out = out_dir("fail");
    let (code, stdout, stderr) = run(&[
        "crawl",
        &srv.url("/"),
        "--rules",
        pack,
        "--no-external",
        "--fail-on-check",
        "fixture/sel-exists",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout}\n{stderr}");
    assert!(
        stderr.contains("Loaded 26 rules from 1 rule pack"),
        "{stderr}"
    );
    assert!(
        stdout.contains("Fixture Checks: Promo box present"),
        "{stdout}"
    );
    let issues = std::fs::read_to_string(out.join("issues.csv")).unwrap();
    assert!(issues.contains("fixture/sel-exists"));
    let report = std::fs::read_to_string(out.join("report.html")).unwrap();
    assert!(report.contains("Promo box present"));
    let urls = std::fs::read_to_string(out.join("issue_urls.csv")).unwrap();
    assert!(urls.contains(&srv.url("/legacy")));

    let out2 = out_dir("pass");
    let (code, stdout, stderr) = run(&[
        "crawl",
        &srv.url("/"),
        "--rules",
        pack,
        "--no-external",
        "--fail-on-check",
        "fixture/never",
        "--quiet",
        "--out",
        out2.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");

    // Rule ids only exist when their pack is loaded.
    let (code, _, stderr) = run(&[
        "crawl",
        &srv.url("/"),
        "--fail-on-check",
        "fixture/sel-exists",
    ]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("unknown check id 'fixture/sel-exists'"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&out2);
}

#[test]
fn cli_lists_and_validates_packs() {
    let pack = pack_path();
    let (code, stdout, _) = run(&["rules", "list", pack.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Fixture (fixture) 0.1.0 by ShoutingToad tests: 26 rules"));
    assert!(stdout.contains("fixture/sel-exists"));
    let starters = Path::new(env!("CARGO_MANIFEST_DIR")).join("rules");
    let (code, stdout, stderr) = run(&["rules", "validate", starters.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("All rule packs are valid."));
}

#[test]
fn invalid_packs_give_clear_errors_and_never_crash() {
    let dir = out_dir("invalid");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.toml");
    std::fs::write(
        &bad,
        "[pack]\nname = \"Bad\"\n\n[[rule]]\nid = \"regex\"\nname = \"R\"\nwhen = { url = '(unclosed' }\n\n[[rule]]\nid = \"css\"\nname = \"C\"\nwhen = { selector = 'div[' }\n\n[[rule]]\nid = \"cmp\"\nname = \"N\"\nwhen = { word_count = 'lots' }\n",
    )
    .unwrap();
    let broken = dir.join("broken.toml");
    std::fs::write(&broken, "[pack\nname = \"x\"\n").unwrap();
    let (code, _, stderr) = run(&["rules", "validate", dir.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(!stderr.contains("panicked"), "{stderr}");
    let bad_s = bad.display().to_string();
    assert!(
        stderr.contains(&format!(
            "{bad_s}:7: rule 'regex': when: invalid regex in `url`"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{bad_s}:12: rule 'css': when: invalid CSS selector"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{bad_s}:17: rule 'cmp': when: word_count: 'lots' is not a comparison"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("{}:1:", broken.display())),
        "{stderr}"
    );
    assert!(stderr.contains("4 problem(s) found"), "{stderr}");

    let (code, _, stderr) = run(&[
        "crawl",
        "http://127.0.0.1:9/",
        "--rules",
        bad.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("error: invalid rule pack"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    let (code, _, stderr) = run(&[
        "rules",
        "validate",
        dir.join("missing.toml").to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("could not read the file"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
