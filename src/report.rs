//! CSV exports, the HTML audit report and the CI workflow template.

use crate::ai::{LinkOpportunity, Suggestion};
use crate::audit::{ih, indexability_label, pixel_width, Audit, Kind, Priority};
use crate::checks::{catalogue, CATEGORIES};
use crate::model::Crawl;
use std::collections::HashSet;
use std::fmt::{Display, Write};
use std::io;

pub const REPO: &str = "https://github.com/NDcompany/ShoutingToad";

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn row(cells: &[String]) -> String {
    let mut line = cells.iter().map(|c| csv(c)).collect::<Vec<_>>().join(",");
    line.push('\n');
    line
}

/// One CSV line from borrowed cells.
pub fn csv_row(cells: &[&str]) -> String {
    let mut line = cells.iter().map(|c| csv(c)).collect::<Vec<_>>().join(",");
    line.push('\n');
    line
}

macro_rules! cells {
    ($($x:expr),* $(,)?) => { [$($x.to_string()),*] };
}

/// Streaming CSV writer for the big per-URL and per-link exports: same escaping as [`row`],
/// without a `Vec<String>` per row or the whole file in memory.
struct Csv<W: io::Write> {
    w: W,
    buf: String,
}

impl<W: io::Write> Csv<W> {
    fn new(w: W, header: &[&str]) -> io::Result<Self> {
        let mut c = Csv {
            w,
            buf: String::new(),
        };
        c.w.write_all(row(&header.iter().map(|h| h.to_string()).collect::<Vec<_>>()).as_bytes())?;
        Ok(c)
    }

    fn row(&mut self, cells: &[&dyn Display]) -> io::Result<()> {
        for (i, c) in cells.iter().enumerate() {
            if i > 0 {
                self.w.write_all(b",")?;
            }
            self.buf.clear();
            let _ = write!(self.buf, "{c}");
            if self.buf.contains([',', '"', '\n', '\r']) {
                self.w.write_all(csv(&self.buf).as_bytes())?;
            } else {
                self.w.write_all(self.buf.as_bytes())?;
            }
        }
        self.w.write_all(b"\n")
    }

    fn finish(mut self) -> io::Result<W> {
        self.w.flush()?;
        Ok(self.w)
    }
}

/// Run a streaming export into memory (for the GUI's save dialogs).
fn to_string(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
    let mut out = Vec::new();
    let _ = f(&mut out);
    String::from_utf8(out).unwrap_or_default()
}

pub fn internal_csv(crawl: &Crawl, audit: &Audit) -> String {
    to_string(|w| internal_csv_to(w, crawl, audit).map(drop))
}

pub fn internal_csv_to<W: io::Write>(w: W, crawl: &Crawl, audit: &Audit) -> io::Result<W> {
    let mut out = Csv::new(
        w,
        &[
            "Address",
            "Type",
            "Status Code",
            "Status",
            "Indexability",
            "Title 1",
            "Title 1 Length",
            "Title 1 Pixel Width",
            "Meta Description 1",
            "Meta Description 1 Length",
            "H1-1",
            "H1 Count",
            "H2-1",
            "Canonical",
            "Meta Robots",
            "X-Robots-Tag",
            "Word Count",
            "Flesch Reading Ease",
            "Size (Bytes)",
            "Transferred (Bytes)",
            "Response Time (ms)",
            "Crawl Depth",
            "Inlinks",
            "Outlinks",
            "Images",
            "HTTP Version",
            "Content Encoding",
            "Redirect URL",
            "Issues",
            "Custom Search Hits",
            "Custom Extraction",
        ],
    )?;
    for (i, p) in crawl.pages.iter().enumerate() {
        let h = p.html.as_deref();
        out.row(&[
            &p.url,
            &p.kind(),
            &p.status,
            &p.status_text,
            &indexability_label(p),
            &p.title(),
            &p.title().chars().count(),
            &pixel_width(p.title(), 20.0),
            &p.description(),
            &p.description().chars().count(),
            &p.h1(),
            &h.map(|h| h.h1.len()).unwrap_or(0),
            &h.and_then(|h| h.h2.first())
                .map(|s| s.as_str())
                .unwrap_or(""),
            &p.canonical(),
            &h.map(|h| h.meta_robots.join("; ")).unwrap_or_default(),
            &p.headers_all("x-robots-tag").collect::<Vec<_>>().join("; "),
            &p.word_count(),
            &h.map(|h| format!("{:.1}", h.flesch)).unwrap_or_default(),
            &p.size,
            &p.transfer_size,
            &p.response_ms,
            &p.depth,
            &audit.inlinks.get(i).map(|v| v.len()).unwrap_or(0),
            &h.map(|h| h.links.len()).unwrap_or(0),
            &h.map(|h| h.images.len()).unwrap_or(0),
            &p.http_version,
            &p.content_encoding,
            &p.redirect_to.as_deref().unwrap_or(""),
            &audit.page_issues.get(i).map(|v| v.len()).unwrap_or(0),
            &p.custom_search_hits,
            &p.custom_extract.join(" | "),
        ])?;
    }
    out.finish()
}

pub fn issues_csv(crawl: &Crawl, audit: &Audit) -> String {
    let cat = catalogue();
    let mut out = row(&cells![
        "Issue ID",
        "Category",
        "Issue",
        "Type",
        "Priority",
        "URLs",
        "% of Internal HTML",
        "Description",
        "How To Fix"
    ]);
    let mut list: Vec<(usize, &Vec<usize>)> = audit.triggered().collect();
    list.sort_by_key(|(i, h)| (cat[*i].kind, cat[*i].priority, std::cmp::Reverse(h.len())));
    for (i, hits) in list {
        let c = &cat[i];
        out.push_str(&row(&cells![
            c.id,
            c.category,
            c.name,
            c.kind.label(),
            c.priority.label(),
            hits.len(),
            format!("{:.1}", hit_share(crawl, hits)),
            c.description,
            c.fix
        ]));
    }
    out
}

pub fn issue_urls_csv(crawl: &Crawl, audit: &Audit) -> String {
    to_string(|w| issue_urls_csv_to(w, crawl, audit).map(drop))
}

pub fn issue_urls_csv_to<W: io::Write>(w: W, crawl: &Crawl, audit: &Audit) -> io::Result<W> {
    let cat = catalogue();
    let mut out = Csv::new(
        w,
        &[
            "Issue ID", "Category", "Issue", "Type", "Priority", "Address",
        ],
    )?;
    for (i, hits) in audit.triggered() {
        let c = &cat[i];
        for &pi in hits {
            out.row(&[
                &c.id,
                &c.category,
                &c.name,
                &c.kind.label(),
                &c.priority.label(),
                &crawl.pages[pi].url,
            ])?;
        }
    }
    out.finish()
}

pub fn checks_csv() -> String {
    let mut out = row(&cells![
        "Issue ID",
        "Category",
        "Issue",
        "Type",
        "Priority",
        "Description",
        "How To Fix"
    ]);
    for c in catalogue() {
        out.push_str(&row(&cells![
            c.id,
            c.category,
            c.name,
            c.kind.label(),
            c.priority.label(),
            c.description,
            c.fix
        ]));
    }
    out
}

pub fn inlinks_csv(crawl: &Crawl, audit: &Audit) -> String {
    to_string(|w| inlinks_csv_to(w, crawl, audit).map(drop))
}

pub fn inlinks_csv_to<W: io::Write>(w: W, crawl: &Crawl, audit: &Audit) -> io::Result<W> {
    let mut out = Csv::new(
        w,
        &[
            "Destination",
            "Destination Status",
            "Source",
            "Anchor Text",
            "Follow",
            "Rel",
        ],
    )?;
    for (ti, ins) in audit.inlinks.iter().enumerate() {
        let Some(t) = crawl.pages.get(ti) else { break };
        for &(si, li) in ins {
            let Some(src) = crawl.pages.get(si) else {
                continue;
            };
            let Some(l) = src.html.as_ref().and_then(|h| h.links.get(li)) else {
                continue;
            };
            out.row(&[
                &t.url,
                &t.status,
                &src.url,
                &l.anchor,
                &if l.nofollow { "Nofollow" } else { "Follow" },
                &rel(l),
            ])?;
        }
    }
    out.finish()
}

fn rel(l: &crate::model::Link) -> String {
    let mut r = Vec::new();
    if l.nofollow {
        r.push("nofollow");
    }
    if l.sponsored {
        r.push("sponsored");
    }
    if l.ugc {
        r.push("ugc");
    }
    if l.noopener {
        r.push("noopener");
    }
    r.join(" ")
}

pub fn outlinks_csv(crawl: &Crawl) -> String {
    to_string(|w| outlinks_csv_to(w, crawl).map(drop))
}

pub fn outlinks_csv_to<W: io::Write>(w: W, crawl: &Crawl) -> io::Result<W> {
    let ctx = crate::audit::Ctx::urls_only(crawl);
    let mut out = Csv::new(
        w,
        &[
            "Source",
            "Destination",
            "Destination Status",
            "Anchor Text",
            "Type",
            "Follow",
            "Rel",
        ],
    )?;
    for p in &crawl.pages {
        let Some(h) = &p.html else { continue };
        for l in &h.links {
            let dest = ctx.page(&l.url);
            let status = dest.map(|d| d.status.to_string()).unwrap_or_default();
            let internal = dest.map(|d| d.is_internal).unwrap_or(false);
            out.row(&[
                &p.url,
                &l.url,
                &status,
                &l.anchor,
                &if internal { "Internal" } else { "External" },
                &if l.nofollow { "Nofollow" } else { "Follow" },
                &rel(l),
            ])?;
        }
    }
    out.finish()
}

pub fn images_csv(crawl: &Crawl) -> String {
    to_string(|w| images_csv_to(w, crawl).map(drop))
}

pub fn images_csv_to<W: io::Write>(w: W, crawl: &Crawl) -> io::Result<W> {
    let ctx = crate::audit::Ctx::urls_only(crawl);
    let mut out = Csv::new(
        w,
        &[
            "Page",
            "Image",
            "Alt Text",
            "Alt Attribute",
            "Width & Height Set",
            "Lazy",
            "Image Status",
            "Image Size (Bytes)",
            "Content Type",
        ],
    )?;
    for p in &crawl.pages {
        let Some(h) = &p.html else { continue };
        for img in &h.images {
            let t = ctx.page(&img.url);
            out.row(&[
                &p.url,
                &img.url,
                &img.alt.as_deref().unwrap_or(""),
                &if img.alt.is_some() {
                    "Present"
                } else {
                    "Missing"
                },
                &(img.has_width && img.has_height),
                &img.lazy,
                &t.map(|t| t.status.to_string()).unwrap_or_default(),
                &t.map(|t| t.size.to_string()).unwrap_or_default(),
                &t.map(|t| t.content_type.as_str()).unwrap_or_default(),
            ])?;
        }
    }
    out.finish()
}

pub fn redirects_csv(crawl: &Crawl) -> String {
    to_string(|w| redirects_csv_to(w, crawl).map(drop))
}

pub fn redirects_csv_to<W: io::Write>(w: W, crawl: &Crawl) -> io::Result<W> {
    let ctx = crate::audit::Ctx::urls_only(crawl);
    let mut out = Csv::new(
        w,
        &[
            "Address",
            "Status",
            "Redirect To",
            "Hops",
            "Final Address",
            "Final Status",
            "Loop",
        ],
    )?;
    for p in crawl.pages.iter().filter(|p| p.is_redirect()) {
        let (fin, hops, looped) = ctx.follow(&p.url);
        out.row(&[
            &p.url,
            &p.status,
            &p.redirect_to.as_deref().unwrap_or(""),
            &hops,
            &fin.map(|f| f.url.as_str()).unwrap_or(""),
            &fin.map(|f| f.status.to_string()).unwrap_or_default(),
            &looped,
        ])?;
    }
    out.finish()
}

pub fn suggestions_csv(s: &[Suggestion]) -> String {
    let mut out = row(&cells![
        "Address",
        "Reason",
        "Current Title",
        "Suggested Title",
        "Title Length",
        "Current Description",
        "Suggested Description",
        "Description Length",
        "Source",
        "Error"
    ]);
    for x in s {
        out.push_str(&row(&cells![
            x.url,
            x.reason,
            x.current_title,
            x.title,
            x.title.chars().count(),
            x.current_description,
            x.description,
            x.description.chars().count(),
            x.source,
            x.error.as_deref().unwrap_or("")
        ]));
    }
    out
}

pub fn link_opportunities_csv(o: &[LinkOpportunity]) -> String {
    let mut out = row(&cells![
        "Source Page",
        "Phrase",
        "Mentions",
        "Suggested Target",
        "Target Title",
        "Context"
    ]);
    for x in o {
        out.push_str(&row(&cells![
            x.source_url,
            x.phrase,
            x.mentions,
            x.target_url,
            x.target_title,
            x.context
        ]));
    }
    out
}

/// Share of affected URLs in percent. Checks on HTML pages are measured against
/// internal HTML pages; checks that also hit redirects, images or other files are
/// measured against all internal URLs, so the figure never exceeds 100%.
fn hit_share(crawl: &Crawl, hits: &[usize]) -> f32 {
    let base = if hits.iter().all(|&i| ih(&crawl.pages[i])) {
        crawl.pages.iter().filter(|p| ih(p)).count()
    } else {
        crawl.pages.iter().filter(|p| p.is_internal).count()
    };
    (hits.len() as f32 * 100.0 / base.max(1) as f32).min(100.0)
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Self-contained, print-friendly HTML audit report.
pub fn html_report(crawl: &Crawl, audit: &Audit, client: &str, prepared_by: &str) -> String {
    let cat = catalogue();
    let score = audit.health_score(crawl);
    let count = |k: Kind| audit.triggered().filter(|(i, _)| cat[*i].kind == k).count();
    let (issues, warnings, opps) = (
        count(Kind::Issue),
        count(Kind::Warning),
        count(Kind::Opportunity),
    );
    let affected: HashSet<usize> = audit
        .triggered()
        .filter(|(i, _)| cat[*i].kind == Kind::Issue)
        .flat_map(|(_, h)| h.iter().copied())
        .collect();
    let score_color = if score >= 85 {
        "#1a7f37"
    } else if score >= 60 {
        "#9a6700"
    } else {
        "#cf222e"
    };
    let date = chrono::Local::now().format("%d %B %Y").to_string();
    let site = url::Url::parse(&crawl.start_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| crawl.start_url.clone());

    let mut cats = String::new();
    for c in CATEGORIES {
        let mut n = [0usize; 3];
        for (i, _) in audit.triggered().filter(|(i, _)| cat[*i].category == *c) {
            n[cat[i].kind as usize] += 1;
        }
        let total = cat.iter().filter(|x| x.category == *c).count();
        let _ = write!(cats, "<tr><td>{}</td><td class=num>{total}</td><td class=num><span class=\"pill issue\">{}</span></td><td class=num><span class=\"pill warning\">{}</span></td><td class=num><span class=\"pill opportunity\">{}</span></td></tr>", esc(c), n[0], n[1], n[2]);
    }

    let mut list: Vec<(usize, &Vec<usize>)> = audit.triggered().collect();
    list.sort_by_key(|(i, h)| (cat[*i].kind, cat[*i].priority, std::cmp::Reverse(h.len())));
    let mut rows = String::new();
    for (i, hits) in &list {
        let c = &cat[*i];
        let kind = c.kind.label().to_lowercase();
        let mut urls = String::new();
        for &pi in hits.iter().take(50) {
            let u = esc(&crawl.pages[pi].url);
            let _ = write!(urls, "<li><a href=\"{u}\">{u}</a></li>");
        }
        if hits.len() > 50 {
            let _ = write!(
                urls,
                "<li class=muted>…and {} more (see the CSV export)</li>",
                hits.len() - 50
            );
        }
        let _ = write!(
            rows,
            "<details class=issue-row><summary><span class=\"pill {kind}\">{}</span><span class=\"prio {}\">{}</span><span class=title>{}</span><span class=cat>{}</span><span class=count>{} URL{} · {:.1}%</span></summary><div class=body><p>{}</p><p><strong>How to fix:</strong> {}</p><ul class=urls>{urls}</ul></div></details>",
            c.kind.label(),
            c.priority.label().to_lowercase(),
            c.priority.label(),
            esc(&c.name),
            esc(c.category),
            hits.len(),
            if hits.len() == 1 { "" } else { "s" },
            hit_share(crawl, hits),
            esc(&c.description),
            esc(&c.fix),
        );
    }
    if rows.is_empty() {
        rows = "<p>No issues were found. Excellent work.</p>".into();
    }

    let mut status_counts = [0usize; 5];
    for p in crawl.pages.iter().filter(|p| p.is_internal) {
        let k = match p.status {
            200..=299 => 0,
            300..=399 => 1,
            400..=499 => 2,
            500..=599 => 3,
            _ => 4,
        };
        status_counts[k] += 1;
    }
    let indexable = crawl
        .pages
        .iter()
        .filter(|p| crate::audit::indexable(p))
        .count();
    let prepared = if prepared_by.trim().is_empty() {
        String::new()
    } else {
        format!(" · Prepared by {}", esc(prepared_by))
    };
    let client_line = if client.trim().is_empty() {
        esc(&site)
    } else {
        esc(client)
    };

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SEO Audit · {client_line}</title>
<style>
:root {{ --ink:#1f2328; --muted:#59636e; --line:#d1d9e0; --bg:#ffffff; --soft:#f6f8fa; --issue:#cf222e; --warning:#9a6700; --opp:#0969da; --good:#1a7f37; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; color:var(--ink); background:var(--bg); }}
main {{ max-width:1080px; margin:0 auto; padding:40px 24px 64px; }}
header {{ border-bottom:3px solid #2da44e; padding-bottom:20px; margin-bottom:28px; }}
h1 {{ margin:0 0 6px; font-size:28px; }}
h2 {{ font-size:20px; margin:36px 0 12px; }}
.sub {{ color:var(--muted); }}
.cards {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(170px,1fr)); gap:14px; }}
.card {{ background:var(--soft); border:1px solid var(--line); border-radius:10px; padding:16px; }}
.card .label {{ font-size:12px; text-transform:uppercase; letter-spacing:.04em; color:var(--muted); font-weight:600; }}
.card .value {{ font-size:30px; font-weight:700; margin-top:4px; }}
.score {{ color:{score_color}; }}
table {{ width:100%; border-collapse:collapse; }}
th,td {{ text-align:left; padding:8px 10px; border-bottom:1px solid var(--line); }}
th {{ font-size:12px; text-transform:uppercase; color:var(--muted); }}
td.num, th.num {{ text-align:right; }}
.pill {{ display:inline-block; min-width:28px; text-align:center; padding:1px 9px; border-radius:999px; font-size:12px; font-weight:600; }}
.pill.issue {{ background:#ffebe9; color:var(--issue); }}
.pill.warning {{ background:#fff8c5; color:var(--warning); }}
.pill.opportunity {{ background:#ddf4ff; color:var(--opp); }}
.issue-row {{ border:1px solid var(--line); border-radius:8px; margin-bottom:8px; background:#fff; }}
.issue-row summary {{ cursor:pointer; display:flex; gap:10px; align-items:center; padding:10px 14px; flex-wrap:wrap; }}
.issue-row .title {{ font-weight:600; flex:1; min-width:200px; }}
.issue-row .cat, .issue-row .count {{ color:var(--muted); font-size:13px; }}
.prio {{ font-size:12px; font-weight:600; color:var(--muted); }}
.prio.high {{ color:var(--issue); }}
.issue-row .body {{ padding:0 14px 12px; border-top:1px solid var(--line); }}
.urls {{ font-size:13px; max-height:320px; overflow:auto; padding-left:20px; }}
.urls a {{ color:var(--opp); word-break:break-all; }}
.muted {{ color:var(--muted); }}
footer {{ margin-top:48px; padding-top:14px; border-top:1px solid var(--line); color:var(--muted); font-size:13px; }}
footer a {{ color:inherit; }}
@media print {{ .issue-row {{ break-inside:avoid; }} details {{ display:block; }} .urls {{ max-height:none; }} }}
</style>
</head>
<body>
<main>
<header>
  <h1>Technical SEO Audit</h1>
  <div class="sub">{client_line} · {site_esc} · {date}{prepared}</div>
</header>

<div class="cards">
  <div class="card"><div class="label">Health score</div><div class="value score">{score}/100</div></div>
  <div class="card"><div class="label">URLs crawled</div><div class="value">{total}</div></div>
  <div class="card"><div class="label">Indexable pages</div><div class="value">{indexable}</div></div>
  <div class="card"><div class="label">Pages with issues</div><div class="value">{affected}</div></div>
  <div class="card"><div class="label">Issues · Warnings · Opps</div><div class="value"><span style="color:var(--issue)">{issues}</span> · <span style="color:var(--warning)">{warnings}</span> · <span style="color:var(--opp)">{opps}</span></div></div>
</div>

<h2>Response codes (internal)</h2>
<table><thead><tr><th>2xx Success</th><th>3xx Redirect</th><th>4xx Client error</th><th>5xx Server error</th><th>No response / blocked</th></tr></thead>
<tbody><tr><td>{s0}</td><td>{s1}</td><td>{s2}</td><td>{s3}</td><td>{s4}</td></tr></tbody></table>

<h2>Findings by category</h2>
<p class="muted">{checks} checks were run across {cat_count} categories.</p>
<table><thead><tr><th>Category</th><th class=num>Checks</th><th class=num>Issues</th><th class=num>Warnings</th><th class=num>Opportunities</th></tr></thead><tbody>{cats}</tbody></table>

<h2>All findings, most important first</h2>
{rows}

<footer>Generated with <a href="{REPO}">ShoutingToad SEO Spider</a> v{version}.</footer>
</main>
</body>
</html>
"##,
        site_esc = esc(&crawl.start_url),
        total = crawl.pages.len(),
        affected = affected.len(),
        s0 = status_counts[0],
        s1 = status_counts[1],
        s2 = status_counts[2],
        s3 = status_counts[3],
        s4 = status_counts[4],
        checks = cat.len(),
        cat_count = CATEGORIES.len(),
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// GitHub Actions workflow that runs a headless crawl on every push and on a schedule. With
/// `baseline`, the last passing crawl is kept in the Actions cache and the job fails only on
/// issues that are new since then.
pub fn ci_workflow(url: &str, fail_on: Priority, baseline: bool) -> String {
    let level = fail_on.label().to_lowercase();
    let steps = if baseline {
        format!(
            r#"      - name: Restore the previous crawl
        uses: actions/cache/restore@v4
        with:
          path: seo-baseline
          key: seo-baseline-${{{{ github.run_id }}}}
          restore-keys: seo-baseline-

      - name: Crawl, audit and compare with the previous crawl
        run: |
          ./shouting-toad crawl "{url}" --max-urls 5000 --out seo-report \
            --baseline seo-baseline/crawl.shoutingtoad --fail-on-new {level} \
            --summary-md seo-report/summary.md --badge seo-report/seo-badge.svg

      - name: Job summary
        if: always()
        run: cat seo-report/summary.md >> "$GITHUB_STEP_SUMMARY" || true

      - name: Keep this crawl as the next baseline
        if: success()
        run: mkdir -p seo-baseline && cp seo-report/crawl.shoutingtoad seo-baseline/

      - name: Save the baseline
        if: success()
        uses: actions/cache/save@v4
        with:
          path: seo-baseline
          key: seo-baseline-${{{{ github.run_id }}}}
"#
        )
    } else {
        format!(
            r#"      - name: Crawl and audit
        run: |
          ./shouting-toad crawl "{url}" --max-urls 5000 --out seo-report --fail-on {level}
"#
        )
    };
    format!(
        r#"name: SEO audit (ShoutingToad)

on:
  push:
    branches: [main]
  schedule:
    - cron: "0 3 * * 1"
  workflow_dispatch:

jobs:
  seo-audit:
    runs-on: ubuntu-latest
    steps:
      - name: Download ShoutingToad
        run: |
          curl -fsSL -o shouting-toad.tar.gz \
            {REPO}/releases/latest/download/shouting-toad-x86_64-unknown-linux-gnu.tar.gz
          tar -xzf shouting-toad.tar.gz
          chmod +x shouting-toad

{steps}
      - name: Upload report
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: seo-report
          path: seo-report
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escaping() {
        assert_eq!(csv("plain"), "plain");
        assert_eq!(csv("a,b"), "\"a,b\"");
        assert_eq!(csv("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn ci_workflow_with_baseline() {
        let plain = ci_workflow("https://e.com", Priority::High, false);
        assert!(plain.contains("--fail-on high") && !plain.contains("--baseline"));
        let y = ci_workflow("https://e.com", Priority::Medium, true);
        assert!(y.contains("--baseline seo-baseline/crawl.shoutingtoad --fail-on-new medium"));
        assert!(y.contains("key: seo-baseline-${{ github.run_id }}"));
        assert!(y.contains("actions/cache/save@v4") && y.contains("$GITHUB_STEP_SUMMARY"));
    }

    #[test]
    fn html_escaping() {
        assert_eq!(
            esc("<a href=\"x\">&</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
    }
}
