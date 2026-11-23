//! Crawl comparison, migration mapping, redirect validation and the CI regression gate, end to
//! end against fixture sites: a "before" site and an "after" site where pages were moved,
//! renamed, deleted, changed and added.

mod support;

use shouting_toad::audit;
use shouting_toad::checks::find;
use shouting_toad::compare::{self, UrlChange};
use shouting_toad::migrate::{self, Method};
use shouting_toad::model::Crawl;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::*;

fn site(b: &str, pages: Vec<(&str, Doc)>) -> Site {
    let mut s = Site::new();
    let nav: String = pages
        .iter()
        .map(|(p, _)| format!(r#"<a href="{p}">Go to {p}</a> "#))
        .collect();
    s.insert(
        "/".into(),
        Doc::good(b, "/", "Toad Shop home")
            .body(format!("<ul>{nav}</ul>"))
            .route(),
    );
    for (path, doc) in pages {
        s.insert(path.into(), doc.route());
    }
    s
}

fn before(b: &str) -> Site {
    site(
        b,
        vec![
            ("/about", Doc::good(b, "/about", "About our pond shop")),
            ("/team", Doc::good(b, "/team", "Our toad team")),
            (
                "/guides/pond-care",
                Doc::good(b, "/guides/pond-care", "Pond care")
                    .title("Pond care basics | Toad Shop"),
            ),
            (
                "/blog/green-tree-frogs",
                Doc::good(b, "/blog/green-tree-frogs", "Green tree frogs"),
            ),
            ("/story", Doc::good(b, "/story", "Story of Tom")),
            (
                "/blog/2019/old-sale",
                Doc::good(b, "/blog/2019/old-sale", "Summer sale 2019"),
            ),
            ("/changed", Doc::good(b, "/changed", "Changing page")),
            ("/stays", Doc::good(b, "/stays", "Stable page")),
            (
                "/fixed-page",
                Doc::good(b, "/fixed-page", "Fixed later").drop("<title"),
            ),
        ],
    )
}

/// The relaunched site: /team moved (same title), the pond guide moved and retitled (same H1),
/// the frog post rewritten under a similar URL, the story moved with the same text, the 2019
/// sale deleted, /changed retitled without a description, /fixed-page fixed, /new-page added
/// without a title or H1.
fn after(b: &str) -> Site {
    site(
        b,
        vec![
            ("/about", Doc::good(b, "/about", "About our pond shop")),
            ("/people", Doc::good(b, "/people", "Our toad team")),
            (
                "/learn/pond",
                Doc::good(b, "/learn/pond", "Pond care")
                    .title("Looking after your pond | Toad Shop"),
            ),
            (
                "/frogs/green-tree-frogs",
                Doc::good(b, "/frogs/green-tree-frogs", "Tree frog facts"),
            ),
            (
                "/stories/toad-tale",
                Doc::good(b, "/stories/toad-tale", "A toad tale")
                    .drop("<p class=\"prose\"")
                    .body(format!(
                        r#"<p class="prose">{}</p>"#,
                        prose("Story of Tom", 220)
                    )),
            ),
            ("/blog", Doc::good(b, "/blog", "Toad blog")),
            (
                "/changed",
                Doc::good(b, "/changed", "Changing page")
                    .title("A brand new title for the changing page")
                    .drop("<meta name=\"description\""),
            ),
            ("/stays", Doc::good(b, "/stays", "Stable page")),
            ("/fixed-page", Doc::good(b, "/fixed-page", "Fixed later")),
            (
                "/new-page",
                Doc::good(b, "/new-page", "Brand new")
                    .drop("<title")
                    .drop("<h1"),
            ),
        ],
    )
}

/// The same crawl as if it had been taken on another origin (both fixtures run on random
/// ports; comparison matches exact URLs).
fn rebase(c: &Crawl, from: &Server, to: &Server) -> Crawl {
    let json = serde_json::to_string(c)
        .unwrap()
        .replace(&from.base, &to.base)
        .replace(&from.ext, &to.ext);
    serde_json::from_str(&json).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("st-cmp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
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

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn crawl_site(srv: &Server) -> Crawl {
    crawl(&[srv.url("/")], false, config())
}

#[test]
fn comparison_and_migration_of_a_relaunched_site() {
    let a_srv = Server::start(|b, _, _| before(b));
    let b_srv = Server::start(|b, _, _| after(b));
    let a = crawl_site(&a_srv);
    let b = crawl_site(&b_srv);
    let (ab, bb) = (&a_srv.base, &b_srv.base);

    // ---- comparison: A as if it were taken on B's origin
    let a2 = rebase(&a, &a_srv, &b_srv);
    let (audit_a, audit_b) = (audit::run(&a2), audit::run(&b));
    let cmp = compare::compare(&a2, &audit_a, &b, &audit_b);
    let urls = |change: UrlChange| -> Vec<String> {
        let mut v: Vec<String> = cmp
            .urls
            .iter()
            .filter(|u| u.change == change)
            .map(|u| u.url.trim_start_matches(bb.as_str()).to_string())
            .filter(|u| !u.starts_with("http") && u != "/perfect")
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        urls(UrlChange::Added),
        [
            "/blog",
            "/frogs/green-tree-frogs",
            "/learn/pond",
            "/new-page",
            "/people",
            "/stories/toad-tale"
        ]
    );
    assert_eq!(
        urls(UrlChange::Removed),
        [
            "/blog/2019/old-sale",
            "/blog/green-tree-frogs",
            "/guides/pond-care",
            "/story",
            "/team"
        ]
    );
    let changed: Vec<(&str, Vec<&str>)> = cmp
        .urls
        .iter()
        .filter(|u| u.change == UrlChange::Changed)
        .map(|u| {
            (
                u.url.trim_start_matches(bb.as_str()),
                u.fields.iter().map(|f| f.field).collect(),
            )
        })
        .collect();
    assert!(
        changed.contains(&("/changed", vec!["Title", "Meta Description"])),
        "{changed:?}"
    );
    assert!(
        changed
            .iter()
            .all(|(u, _)| !["/about", "/stays"].contains(u)),
        "unchanged pages reported as changed: {changed:?}"
    );
    let diff = |id: &str| {
        let c = find(id).unwrap();
        let d = cmp.checks.iter().find(|d| d.check == c);
        let urls = |idx: &[usize], crawl: &Crawl| -> Vec<String> {
            idx.iter()
                .map(|&i| {
                    crawl.pages[i]
                        .url
                        .trim_start_matches(bb.as_str())
                        .to_string()
                })
                .collect()
        };
        d.map(|d| {
            (
                urls(&d.new, &b),
                urls(&d.fixed, &a2),
                urls(&d.persisting, &b),
            )
        })
        .unwrap_or_default()
    };
    let (new, fixed, _) = diff("titles.missing");
    assert_eq!(
        (new, fixed),
        (
            vec!["/new-page".to_string()],
            vec!["/fixed-page".to_string()]
        )
    );
    assert_eq!(diff("h1.missing").0, ["/new-page"]);
    assert_eq!(diff("description.missing").0, ["/changed"]);
    assert!(cmp
        .new_issues(audit::Priority::High)
        .any(|d| d.check == find("titles.missing").unwrap()));
    assert_eq!(cmp.score_before, audit_a.health_score(&a2));
    // every finding on an unchanged URL persists
    let about = b
        .pages
        .iter()
        .position(|p| p.url == format!("{bb}/about"))
        .unwrap();
    assert!(cmp.checks.iter().any(|d| d.persisting.contains(&about)));
    assert!(cmp.checks.iter().all(|d| !d.new.contains(&about)));
    let html = compare::html_report(&a2, &b, &cmp);
    assert!(html.contains("/new-page") && !html.contains("<script"));

    // ---- migration: old site A -> new site B on another origin
    let maps = migrate::propose(&a, &b);
    let got = |path: &str| {
        let m = maps
            .iter()
            .find(|m| m.old == format!("{ab}{path}"))
            .unwrap_or_else(|| panic!("no mapping for {path}: {maps:#?}"));
        (
            m.method,
            m.new.trim_start_matches(bb.as_str()).to_string(),
            m.confidence,
        )
    };
    let expect = [
        ("/", Method::Path, "/"),
        ("/about", Method::Path, "/about"),
        ("/stays", Method::Path, "/stays"),
        ("/team", Method::Title, "/people"),
        ("/guides/pond-care", Method::H1, "/learn/pond"),
        ("/story", Method::Content, "/stories/toad-tale"),
        (
            "/blog/green-tree-frogs",
            Method::Slug,
            "/frogs/green-tree-frogs",
        ),
        ("/blog/2019/old-sale", Method::Parent, "/blog"),
    ];
    for (old, method, new) in expect {
        let (m, n, _) = got(old);
        assert_eq!((m, n.as_str()), (method, new), "{old}: {maps:#?}");
    }
    assert!(got("/about").2 == 100 && got("/team").2 >= 85 && got("/blog/2019/old-sale").2 < 20);
    // /fixed-page has no title in A but the same path in B
    assert_eq!(got("/fixed-page").0, Method::Path);
    assert_eq!(maps.len(), 10, "only indexable old URLs: {maps:#?}");

    let dir = tmp("migrate-lib");
    migrate::write_all(&dir, &maps).unwrap();
    let ht = read(&dir.join("redirects.htaccess"));
    assert!(
        ht.contains(&format!(
            "RewriteRule \"^team/?$\" \"{bb}/people\" [R=301,L,NE]"
        )),
        "{ht}"
    );
    assert!(
        !ht.contains("old\\-sale"),
        "parent suggestions need review first:\n{ht}"
    );
    let ng = read(&dir.join("redirects.nginx.conf"));
    assert!(
        ng.contains(&format!("\"/guides/pond-care\" \"{bb}/learn/pond\";")),
        "{ng}"
    );
    let nf = read(&dir.join("_redirects"));
    assert!(
        nf.contains(&format!("/story  {bb}/stories/toad-tale  301")),
        "{nf}"
    );
    let cf = read(&dir.join("cloudflare_bulk_redirects.csv"));
    let host = ab.trim_start_matches("http://");
    assert!(
        cf.contains(&format!(
            "{host}/team,{bb}/people,301,TRUE,FALSE,FALSE,FALSE"
        )),
        "{cf}"
    );
    let csv = read(&dir.join("redirect_map.csv"));
    assert!(csv.starts_with("old,new,confidence,reason\n"));
    assert_eq!(
        migrate::read_map(&dir.join("redirect_map.csv"))
            .unwrap()
            .len(),
        maps.len()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_baseline_gate_compare_migrate_and_badge() {
    let a_srv = Server::start(|b, _, _| before(b));
    let b_srv = Server::start(|b, _, _| after(b));
    let dir = tmp("cli");
    let a_file = dir.join("before.json");
    let a = crawl_site(&a_srv);
    std::fs::write(
        &a_file,
        serde_json::to_vec(&rebase(&a, &a_srv, &b_srv)).unwrap(),
    )
    .unwrap();
    let a_orig = dir.join("old-site.json");
    std::fs::write(&a_orig, serde_json::to_vec(&a).unwrap()).unwrap();
    let s = |p: &Path| p.display().to_string();
    let (out1, md, svg) = (
        dir.join("run1"),
        dir.join("summary.md"),
        dir.join("badge.svg"),
    );

    // a new High issue (/new-page has no title) fails the gate
    let (code, stdout, stderr) = run(&[
        "crawl",
        &b_srv.url("/"),
        "--out",
        &s(&out1),
        "--quiet",
        "--baseline",
        &s(&a_file),
        "--fail-on-new",
        "high",
        "--summary-md",
        &s(&md),
        "--badge",
        &s(&svg),
    ]);
    assert_eq!(code, 2, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("NEW [Issue High] Page Titles: Missing"),
        "{stdout}"
    );
    for f in [
        "comparison.html",
        "comparison.csv",
        "comparison_issue_urls.csv",
        "url_changes.csv",
    ] {
        assert!(out1.join(f).exists(), "{f} missing");
    }
    let summary = read(&md);
    assert!(
        summary.contains("**Failed:**") && summary.contains("### New findings"),
        "{summary}"
    );
    assert!(
        summary.contains("/new-page`") && summary.contains("### Fixed findings"),
        "{summary}"
    );
    let badge = read(&svg);
    let doc = roxmltree::Document::parse(&badge).expect("badge is valid XML");
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "svg");
    let score: String = stdout
        .split("Health score: ")
        .nth(1)
        .unwrap()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    assert!(root
        .attribute("aria-label")
        .unwrap()
        .contains(&format!("{score}/100")));
    assert!(doc
        .descendants()
        .any(|n| n.has_tag_name("title") && n.text().unwrap().contains("ShoutingToad")));
    let changes = read(&out1.join("url_changes.csv"));
    assert!(
        changes.contains(&format!("{}/team,Removed,Status,200,", b_srv.base)),
        "{changes}"
    );
    assert!(
        changes.contains(&format!("{}/people,Added,Status,,200", b_srv.base)),
        "{changes}"
    );

    // the same site again: nothing is new, so even --fail-on-new low passes
    let out2 = dir.join("run2");
    let (code, stdout, stderr) = run(&[
        "crawl",
        &b_srv.url("/"),
        "--out",
        &s(&out2),
        "--quiet",
        "--baseline",
        &s(&out1.join("crawl.shoutingtoad")),
        "--fail-on-new",
        "low",
    ]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("0 added, 0 removed, 0 changed URLs"),
        "{stdout}"
    );

    // first CI run: no baseline file yet
    let (code, _, stderr) = run(&[
        "crawl",
        &b_srv.url("/"),
        "--out",
        &s(&dir.join("run3")),
        "--quiet",
        "--baseline",
        &s(&dir.join("nope.shoutingtoad")),
        "--fail-on-new",
        "high",
        "--summary-md",
        &s(&md),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("does not exist yet") && read(&md).contains("No baseline"));
    let (code, _, stderr) = run(&["crawl", &b_srv.url("/"), "--fail-on-new", "high"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("needs --baseline"));

    // compare two saved crawls
    let cmp_dir = dir.join("cmp");
    let after_file = s(&out1.join("crawl.shoutingtoad"));
    let (code, stdout, _) = run(&["compare", &s(&a_file), &after_file, "--out", &s(&cmp_dir)]);
    assert_eq!(code, 0, "{stdout}");
    assert!(read(&cmp_dir.join("comparison.csv"))
        .contains("titles.missing,Page Titles,Missing,Issue,High,1,1,1,1,0"));
    let (code, ..) = run(&[
        "compare",
        &s(&a_file),
        &after_file,
        "--out",
        &s(&cmp_dir),
        "--fail-on-new",
        "medium",
    ]);
    assert_eq!(code, 2);
    let (code, ..) = run(&["compare", &s(&a_file), "--out", &s(&cmp_dir)]);
    assert_eq!(code, 1);

    // migration mapping from the CLI
    let mig = dir.join("mig");
    let (code, stdout, stderr) = run(&["migrate", &s(&a_orig), &after_file, "--out", &s(&mig)]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Title match") && stdout.contains("Content match"),
        "{stdout}"
    );
    let map = read(&mig.join("redirect_map.csv"));
    assert!(
        map.contains(&format!("{}/team,{}/people,90,", a_srv.base, b_srv.base)),
        "{map}"
    );
    for f in [
        "redirects.htaccess",
        "redirects.nginx.conf",
        "_redirects",
        "cloudflare_bulk_redirects.csv",
    ] {
        assert!(mig.join(f).exists());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn redirect_map_validation_finds_wrong_redirects() {
    let b_srv = Server::start(|b, _, _| after(b));
    let bb = b_srv.base.clone();
    let old = Server::start(move |c, _, _| {
        let mut s = Site::new();
        s.insert("/about".into(), redirect(301, &format!("{bb}/about")));
        s.insert("/team".into(), redirect(302, &format!("{bb}/people")));
        s.insert(
            "/guides/pond-care".into(),
            redirect(301, &format!("{c}/hop")),
        );
        s.insert("/hop".into(), redirect(301, &format!("{bb}/learn/pond")));
        // /blog/green-tree-frogs: not redirected at all (404)
        s.insert("/story".into(), redirect(301, &format!("{bb}/about")));
        s.insert(
            "/changed".into(),
            redirect(308, &format!("{bb}/gone-target")),
        );
        s
    });
    let (c, b) = (&old.base, &b_srv.base);
    let dir = tmp("validate");
    let map = dir.join("map.csv");
    std::fs::write(
        &map,
        format!(
            "old,new,confidence,reason\n\
             {c}/about,{b}/about,100,Path match\n\
             {c}/team,{b}/people,90,\"Title match: same title \"\"our toad team\"\", really\"\n\
             {c}/guides/pond-care,{b}/learn/pond,85,H1 match\n\
             {c}/blog/green-tree-frogs,{b}/frogs/green-tree-frogs,70,Slug match\n\
             {c}/story,{b}/stories/toad-tale,80,Content match\n\
             {c}/changed,{b}/gone-target,50,Manual\n"
        ),
    )
    .unwrap();
    let out = dir.join("out");
    let s = |p: &Path| p.display().to_string();
    let (code, stdout, stderr) = run(&[
        "validate-redirects",
        &s(&map),
        "--out",
        &s(&out),
        "--threads",
        "4",
        "--quiet",
    ]);
    assert_eq!(code, 2, "{stdout}\n{stderr}");
    let csv = read(&out.join("redirect_validation.csv"));
    let result = |path: &str| -> String {
        csv.lines()
            .find(|l| l.starts_with(&format!("{c}{path},")))
            .unwrap_or_else(|| panic!("{path} missing:\n{csv}"))
            .rsplit(',')
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(result("/about"), "OK");
    assert_eq!(result("/team"), "temporary redirect (302)");
    assert_eq!(result("/guides/pond-care"), "redirect chain (2 hops)");
    assert_eq!(result("/blog/green-tree-frogs"), "not redirected (404)");
    assert_eq!(result("/story"), "redirects to a different URL than mapped");
    assert_eq!(result("/changed"), "target returns 404");
    assert!(stdout.contains("1 OK, 5 with problems"), "{stdout}");
    assert!(read(&out.join("old_urls.txt")).lines().count() == 6);

    // the same check against the saved list-mode crawl, without crawling again
    let (code, stdout, _) = run(&[
        "validate-redirects",
        &s(&map),
        "--out",
        &s(&dir.join("again")),
        "--crawl",
        &s(&out.join("crawl.shoutingtoad")),
    ]);
    assert_eq!(code, 2);
    assert!(stdout.contains("1 OK, 5 with problems"), "{stdout}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Stress run against a saved crawl (e.g. 50k URLs from examples/stress_site.rs):
/// `ST_STRESS_CRAWL=path cargo test --release --test compare stress -- --ignored --nocapture`.
/// B is A mutated: 5% of pages removed, 5% moved to new paths, 5% retitled, 2% added.
#[test]
#[ignore]
fn stress_compare_and_migrate() {
    use std::time::Instant;
    let path =
        std::env::var("ST_STRESS_CRAWL").expect("set ST_STRESS_CRAWL to a .shoutingtoad file");
    let t = Instant::now();
    let a = Crawl::load(Path::new(&path)).unwrap();
    println!(
        "loaded {} pages in {:.1}s",
        a.pages.len(),
        t.elapsed().as_secs_f32()
    );
    let mut b = a.clone();
    let n = b.pages.len();
    let mut removed = 0;
    b.pages.retain(|p| {
        let keep = !(p.id % 20 == 3 && p.url.contains("/p/"));
        removed += !keep as usize;
        keep
    });
    for p in b.pages.iter_mut() {
        if p.id % 20 == 7 && p.url.contains("/p/") {
            p.url = p.url.replace("/p/", "/articles/");
        } else if p.id % 20 == 11 {
            if let Some(h) = p.html.as_mut() {
                h.titles = vec![format!("Retitled page {}", p.id)];
            }
        }
    }
    let extra: Vec<_> = b.pages.iter().filter(|p| p.id % 50 == 1).cloned().collect();
    for (k, mut p) in extra.into_iter().enumerate() {
        p.id = n + 10 + k;
        p.url = format!("{}-copy", p.url);
        b.pages.push(p);
    }
    println!("B: {} pages ({removed} removed)", b.pages.len());
    let t = Instant::now();
    let (aa, ab) = (audit::run(&a), audit::run(&b));
    println!("audits: {:.1}s", t.elapsed().as_secs_f32());
    let t = Instant::now();
    let cmp = compare::compare(&a, &aa, &b, &ab);
    let cmp_secs = t.elapsed().as_secs_f32();
    println!(
        "compare: {cmp_secs:.2}s; {} added, {} removed, {} changed URLs, {} checks differ",
        cmp.count(UrlChange::Added),
        cmp.count(UrlChange::Removed),
        cmp.count(UrlChange::Changed),
        cmp.checks
            .iter()
            .filter(|d| !d.new.is_empty() || !d.fixed.is_empty())
            .count()
    );
    let t = Instant::now();
    let html = compare::html_report(&a, &b, &cmp);
    let csvs = compare::url_changes_csv(&cmp).len() + compare::issue_urls_csv(&a, &b, &cmp).len();
    println!(
        "reports: {:.2}s ({} KB HTML, {} KB CSV)",
        t.elapsed().as_secs_f32(),
        html.len() / 1024,
        csvs / 1024
    );
    let t = Instant::now();
    let maps = migrate::propose(&a, &b);
    let mig_secs = t.elapsed().as_secs_f32();
    let mut by: Vec<(&str, usize)> = Vec::new();
    for m in &maps {
        match by.iter_mut().find(|(k, _)| *k == m.method.label()) {
            Some(c) => c.1 += 1,
            None => by.push((m.method.label(), 1)),
        }
    }
    println!(
        "migrate: {mig_secs:.2}s for {} old URLs: {by:?}",
        maps.len()
    );
    let t = Instant::now();
    let snippets = migrate::htaccess(&maps).len() + migrate::nginx(&maps).len();
    println!(
        "snippets: {:.2}s ({} KB)",
        t.elapsed().as_secs_f32(),
        snippets / 1024
    );
    // moved pages keep their title, so they must be found by title (or content)
    let moved = maps.iter().filter(|m| m.new.contains("/articles/")).count();
    println!("moved pages mapped to their new URL: {moved}");
    assert!(cmp_secs < 5.0 && mig_secs < 20.0);
    std::mem::forget((a, b, aa, ab, cmp, maps));
}
