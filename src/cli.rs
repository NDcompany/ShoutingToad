//! Headless command-line interface: `shouting-toad crawl <url> …`.

use crate::audit::{self, Kind, Priority};
use crate::checks::{catalogue, find};
use crate::crawler::{self, Config, Event};
use crate::model::Crawl;
use crate::report;
use crate::rules;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

const HELP: &str =
    "ShoutingToad SEO Spider: crawls a website and audits it for over 300 SEO and UX issues.

USAGE:
    shouting-toad                         Open the desktop app
    shouting-toad crawl <URL> [OPTIONS]   Crawl a site and write reports
    shouting-toad list <FILE> [OPTIONS]   Audit only the URLs in FILE (one per line)
    shouting-toad checks                  Print every check as CSV
    shouting-toad rules validate <PATH>…  Check rule pack files (or folders of them)
    shouting-toad rules list <PATH>…      List the rules in rule packs
    shouting-toad --version | --help

OPTIONS:
    --out <DIR>             Output folder (default: shoutingtoad-report)
    --max-urls <N>          Stop after discovering N URLs (default 100000)
    --max-depth <N>         Maximum link depth (default 10)
    --threads <N>           Concurrent requests (default 6)
    --delay <MS>            Minimum delay between requests in milliseconds
    --timeout <SECS>        Request timeout (default 20)
    --user-agent <UA>       Custom user agent
    --include <REGEX>       Only crawl internal URLs matching REGEX
    --exclude <REGEX>       Skip internal URLs matching REGEX
    --subdomains            Treat subdomains as internal
    --ignore-robots         Do not obey robots.txt
    --no-external           Do not check external links
    --no-resources          Do not check images, CSS and JavaScript
    --no-sitemaps           Do not crawl URLs found only in XML sitemaps
    --fail-on <LEVEL>       Exit with code 2 if any Issue of this priority or higher
                            is found: high, medium or low
    --fail-on-check <ID>    Exit with code 2 if this check is triggered (repeatable;
                            run `shouting-toad checks` for IDs)
    --rules <PATH>          Add the checks in a rule pack file, or in every .toml pack
                            in a folder (repeatable; see docs/rule-packs.md)
    --quiet                 Only print the summary

OUTPUT FILES:
    report.html, issues.csv, issue_urls.csv, internal.csv, inlinks.csv,
    outlinks.csv, images.csv, redirects.csv, crawl.shoutingtoad (open in the app)

EXIT CODES:
    0 success, 1 error, 2 --fail-on threshold reached";

pub fn run(args: Vec<String>) -> i32 {
    let rest: Vec<String> = args.into_iter().skip(1).collect();
    match rest.first().map(|s| s.as_str()) {
        Some("--version") | Some("-V") => {
            println!("shouting-toad {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("--help") | Some("-h") | Some("help") => {
            println!("{HELP}");
            0
        }
        Some("checks") => {
            print!("{}", report::checks_csv());
            0
        }
        Some("rules") => rules_cmd(&rest[1..]),
        Some("crawl") | Some("list") => match crawl_cmd(&rest) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        Some(other) => {
            eprintln!("error: unknown command '{other}'\n\n{HELP}");
            1
        }
        None => 0,
    }
}

fn crawl_cmd(args: &[String]) -> Result<i32, String> {
    let list_mode = args[0] == "list";
    let mut cfg = Config::default();
    let mut out = PathBuf::from("shoutingtoad-report");
    let mut fail_on: Option<Priority> = None;
    let mut fail_check_ids: Vec<String> = Vec::new();
    let mut rule_paths: Vec<PathBuf> = Vec::new();
    let mut quiet = false;
    let mut target: Option<String> = None;

    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        let num = |s: String, name: &str| {
            s.parse::<usize>()
                .map_err(|_| format!("{name} must be a number"))
        };
        match a.as_str() {
            "--out" => out = PathBuf::from(val("--out")?),
            "--max-urls" => cfg.max_urls = num(val("--max-urls")?, "--max-urls")?,
            "--max-depth" => cfg.max_depth = num(val("--max-depth")?, "--max-depth")?,
            "--threads" => cfg.threads = num(val("--threads")?, "--threads")?.clamp(1, 64),
            "--delay" => cfg.delay_ms = num(val("--delay")?, "--delay")? as u64,
            "--timeout" => cfg.timeout_secs = num(val("--timeout")?, "--timeout")? as u64,
            "--user-agent" => cfg.user_agent = val("--user-agent")?,
            "--include" => cfg.include = val("--include")?,
            "--exclude" => cfg.exclude = val("--exclude")?,
            "--subdomains" => cfg.crawl_subdomains = true,
            "--ignore-robots" => cfg.respect_robots = false,
            "--no-external" => cfg.check_external = false,
            "--no-resources" => cfg.check_resources = false,
            "--no-sitemaps" => cfg.crawl_sitemaps = false,
            "--quiet" | "-q" => quiet = true,
            "--fail-on" => {
                fail_on = Some(match val("--fail-on")?.to_lowercase().as_str() {
                    "high" => Priority::High,
                    "medium" => Priority::Medium,
                    "low" => Priority::Low,
                    other => {
                        return Err(format!(
                            "--fail-on must be high, medium or low (got '{other}')"
                        ))
                    }
                })
            }
            "--fail-on-check" => fail_check_ids.push(val("--fail-on-check")?),
            "--rules" => rule_paths.push(PathBuf::from(val("--rules")?)),
            s if s.starts_with("--") => return Err(format!("unknown option '{s}'")),
            s => target = Some(s.to_string()),
        }
    }
    if !rule_paths.is_empty() {
        let (packs, problems) = rules::load_paths(&rule_paths);
        if !problems.is_empty() {
            let list: Vec<String> = problems.iter().map(|p| format!("  {p}")).collect();
            return Err(format!(
                "invalid rule pack:
{}",
                list.join(
                    "
"
                )
            ));
        }
        rules::install(&packs);
        if !quiet {
            let n: usize = packs.iter().map(|p| p.rules.len()).sum();
            eprintln!("Loaded {n} rules from {} rule pack(s)", packs.len());
        }
    }
    let fail_checks = fail_check_ids
        .iter()
        .map(|id| {
            find(id).ok_or_else(|| {
                format!("unknown check id '{id}' (run `shouting-toad checks` or `shouting-toad rules list`)")
            })
        })
        .collect::<Result<Vec<usize>, String>>()?;
    let target = target.ok_or(if list_mode {
        "list needs a file of URLs"
    } else {
        "crawl needs a URL"
    })?;
    let seeds: Vec<String> = if list_mode {
        std::fs::read_to_string(&target)
            .map_err(|e| format!("could not read {target}: {e}"))?
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter(|l| match crawler::normalize_url(l) {
                Ok(_) => true,
                Err(e) => {
                    eprintln!("warning: skipping {e}");
                    false
                }
            })
            .collect()
    } else {
        vec![target.clone()]
    };

    let started = Instant::now();
    let handle = crawler::start(seeds.clone(), list_mode, cfg)?;
    let mut crawl = Crawl {
        start_url: crawler::normalize_url(&seeds[0])
            .map(|u| u.to_string())
            .unwrap_or_default(),
        started: chrono::Local::now().to_rfc3339(),
        ..Default::default()
    };
    if !quiet {
        eprintln!("Crawling {} …", crawl.start_url);
    }
    for ev in handle.rx.iter() {
        match ev {
            Event::Page(p) => crawl.pages.push(*p),
            Event::Site(s) => crawl.site = *s,
            Event::Progress {
                crawled, queued, ..
            } => {
                if !quiet && crawled % 50 == 0 {
                    eprintln!("  {crawled} crawled, {queued} queued");
                }
            }
            Event::Log(m) => {
                if !quiet {
                    eprintln!("  note: {m}");
                }
            }
            Event::Finished => break,
        }
    }
    crawl.finished = chrono::Local::now().to_rfc3339();
    crawl.pages.sort_by_key(|p| p.id);
    let crawl_secs = started.elapsed().as_secs_f32();

    let t_audit = Instant::now();
    let audit = audit::run(&crawl);
    let audit_secs = t_audit.elapsed().as_secs_f32();
    let t_write = Instant::now();
    std::fs::create_dir_all(&out)
        .map_err(|e| format!("could not create {}: {e}", out.display()))?;
    // The exports are independent and the per-link ones reach hundreds of MB at 50k+ URLs:
    // write them in parallel, streaming straight to disk.
    let file =
        |name: &str| File::create(out.join(name)).map(|f| BufWriter::with_capacity(1 << 20, f));
    let file = &file;
    let (c, a, project) = (&crawl, &audit, out.join("crawl.shoutingtoad"));
    type Export<'a> = (
        &'static str,
        Box<dyn FnOnce() -> std::io::Result<()> + Send + 'a>,
    );
    let exports: Vec<Export> = vec![
        (
            "report.html",
            Box::new(move || {
                file("report.html")?.write_all(report::html_report(c, a, "", "").as_bytes())
            }),
        ),
        (
            "issues.csv",
            Box::new(move || file("issues.csv")?.write_all(report::issues_csv(c, a).as_bytes())),
        ),
        (
            "issue_urls.csv",
            Box::new(move || report::issue_urls_csv_to(file("issue_urls.csv")?, c, a).map(drop)),
        ),
        (
            "internal.csv",
            Box::new(move || report::internal_csv_to(file("internal.csv")?, c, a).map(drop)),
        ),
        (
            "inlinks.csv",
            Box::new(move || report::inlinks_csv_to(file("inlinks.csv")?, c, a).map(drop)),
        ),
        (
            "outlinks.csv",
            Box::new(move || report::outlinks_csv_to(file("outlinks.csv")?, c).map(drop)),
        ),
        (
            "images.csv",
            Box::new(move || report::images_csv_to(file("images.csv")?, c).map(drop)),
        ),
        (
            "redirects.csv",
            Box::new(move || report::redirects_csv_to(file("redirects.csv")?, c).map(drop)),
        ),
        (
            "crawl.shoutingtoad",
            Box::new(move || c.save(&project).map_err(std::io::Error::other)),
        ),
    ];
    std::thread::scope(|s| {
        let running: Vec<_> = exports
            .into_iter()
            .map(|(name, f)| (name, s.spawn(f)))
            .collect();
        running
            .into_iter()
            .try_for_each(|(name, h)| match h.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(format!("could not write {name}: {e}")),
                Err(_) => Err(format!("could not write {name}: the export crashed")),
            })
    })?;
    let write_secs = t_write.elapsed().as_secs_f32();

    let cat = catalogue();
    let count = |k: Kind| audit.triggered().filter(|(i, _)| cat[*i].kind == k).count();
    println!(
        "\nCrawled {} URLs in {:.1}s ({:.0} URLs/s). Health score: {}/100",
        crawl.pages.len(),
        crawl_secs,
        crawl.pages.len() as f32 / crawl_secs.max(0.001),
        audit.health_score(&crawl)
    );
    println!("Audit took {audit_secs:.1}s, writing reports took {write_secs:.1}s");
    println!(
        "{} issues, {} warnings, {} opportunities ({} checks run)",
        count(Kind::Issue),
        count(Kind::Warning),
        count(Kind::Opportunity),
        cat.len()
    );
    let mut top: Vec<(usize, &Vec<usize>)> = audit.triggered().collect();
    top.sort_by_key(|(i, h)| (cat[*i].kind, cat[*i].priority, std::cmp::Reverse(h.len())));
    for (i, hits) in top.iter().take(15) {
        let c = &cat[*i];
        println!(
            "  [{:<11} {:<6}] {:<55} {:>6} URLs",
            c.kind.label(),
            c.priority.label(),
            format!("{}: {}", c.category, c.name),
            hits.len()
        );
    }
    println!("Reports written to {}", out.display());

    let failed_level = fail_on.is_some_and(|lvl| {
        audit
            .triggered()
            .any(|(i, _)| cat[i].kind == Kind::Issue && cat[i].priority <= lvl)
    });
    let failed_check = fail_checks.iter().any(|&i| !audit.hits[i].is_empty());
    // ponytail: the process exits right after this; freeing millions of small allocations one
    // by one takes ~10s at 50k URLs, so leave it to the OS.
    std::mem::forget((crawl, audit));
    if failed_level || failed_check {
        eprintln!("Failing: issues at or above the --fail-on threshold were found.");
        return Ok(2);
    }
    Ok(0)
}

/// `rules validate <PATH>…` and `rules list <PATH>…`.
fn rules_cmd(args: &[String]) -> i32 {
    let (cmd, paths) = match args.split_first() {
        Some((c, p)) if (c == "validate" || c == "list") && !p.is_empty() => {
            (c.as_str(), p.iter().map(PathBuf::from).collect::<Vec<_>>())
        }
        _ => {
            eprintln!(
                "usage: shouting-toad rules validate <PATH>…
       shouting-toad rules list <PATH>…"
            );
            return 1;
        }
    };
    let (packs, problems) = rules::load_paths(&paths);
    for p in &packs {
        let n = p.rules.len();
        let by = if p.author.is_empty() {
            String::new()
        } else {
            format!(" by {}", p.author)
        };
        println!(
            "{} ({}) {}{by}: {n} rule{} in {}",
            p.name,
            p.id,
            p.version,
            if n == 1 { "" } else { "s" },
            p.path.display()
        );
        if cmd == "list" {
            for r in &p.rules {
                println!(
                    "  {:<44} {:<11} {:<6} {}: {}",
                    r.id,
                    r.kind.label(),
                    r.priority.label(),
                    r.category,
                    r.name
                );
            }
        }
    }
    for p in &problems {
        eprintln!("error: {p}");
    }
    if !problems.is_empty() {
        eprintln!("{} problem(s) found", problems.len());
        return 1;
    }
    if packs.is_empty() {
        eprintln!("error: no rule packs found");
        return 1;
    }
    if cmd == "validate" {
        println!("All rule packs are valid.");
    }
    0
}
