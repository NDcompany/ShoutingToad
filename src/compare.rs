//! Crawl comparison: what changed between a crawl taken before (A) and one taken after (B).
//! Also the CI regression gate (`--baseline`, `--fail-on-new`), the Markdown job summary and
//! the SEO health badge.

use crate::audit::{self, indexability_label, Audit, FxMap, Kind, Priority};
use crate::checks::catalogue;
use crate::model::{Crawl, Page};
use crate::report::{csv_row, esc, REPO};
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// One check's findings in both crawls, matched by URL.
#[derive(Clone, Debug, Default)]
pub struct CheckDiff {
    pub check: usize,
    /// Affected in B, not in A (page indices in B).
    pub new: Vec<usize>,
    /// Affected in A, not in B (page indices in A). Includes URLs that are gone from B.
    pub fixed: Vec<usize>,
    /// Affected in both (page indices in B).
    pub persisting: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlChange {
    Added,
    Removed,
    Changed,
}

impl UrlChange {
    pub fn label(self) -> &'static str {
        match self {
            UrlChange::Added => "Added",
            UrlChange::Removed => "Removed",
            UrlChange::Changed => "Changed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FieldChange {
    pub field: &'static str,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug)]
pub struct UrlDiff {
    pub url: String,
    pub change: UrlChange,
    pub fields: Vec<FieldChange>,
}

#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub site_before: String,
    pub site_after: String,
    pub score_before: u32,
    pub score_after: u32,
    pub pages_before: usize,
    pub pages_after: usize,
    /// Every check triggered in either crawl, in catalogue order.
    pub checks: Vec<CheckDiff>,
    /// Internal URLs that were added, removed or changed.
    pub urls: Vec<UrlDiff>,
}

impl Comparison {
    /// Issues (not warnings or opportunities) at `level` or above that affect new URLs.
    pub fn new_issues(&self, level: Priority) -> impl Iterator<Item = &CheckDiff> {
        let cat = catalogue();
        self.checks.iter().filter(move |d| {
            let c = &cat[d.check];
            !d.new.is_empty() && c.kind == Kind::Issue && c.priority <= level
        })
    }

    pub fn count(&self, change: UrlChange) -> usize {
        self.urls.iter().filter(|u| u.change == change).count()
    }

    /// Checks with new / fixed URLs, per kind (Issue, Warning, Opportunity).
    pub fn kind_counts(&self) -> ([usize; 3], [usize; 3]) {
        let cat = catalogue();
        let (mut new, mut fixed) = ([0; 3], [0; 3]);
        for d in &self.checks {
            let k = cat[d.check].kind as usize;
            new[k] += !d.new.is_empty() as usize;
            fixed[k] += !d.fixed.is_empty() as usize;
        }
        (new, fixed)
    }
}

pub fn parse_level(s: &str) -> Result<Priority, String> {
    match s.to_lowercase().as_str() {
        "high" => Ok(Priority::High),
        "medium" => Ok(Priority::Medium),
        "low" => Ok(Priority::Low),
        other => Err(format!(
            "the level must be high, medium or low (got '{other}')"
        )),
    }
}

/// Word counts move a little on pages with dynamic blocks (dates, "latest posts"): only a
/// change of more than 10 words and 10% counts.
fn words_changed(a: usize, b: usize) -> bool {
    let d = a.abs_diff(b);
    d > 10 && d * 10 > a.max(b)
}

fn status(p: &Page) -> String {
    if p.status == 0 {
        "No response".into()
    } else {
        p.status.to_string()
    }
}

fn meta_robots(p: &Page) -> &[String] {
    p.html.as_deref().map(|h| &h.meta_robots[..]).unwrap_or(&[])
}

fn field_changes(a: &Page, b: &Page) -> Vec<FieldChange> {
    let mut out = Vec::new();
    let mut push = |field, before: &dyn ToString, after: &dyn ToString| {
        out.push(FieldChange {
            field,
            before: before.to_string(),
            after: after.to_string(),
        })
    };
    if a.status != b.status {
        push("Status", &status(a), &status(b));
    }
    let (ia, ib) = (indexability_label(a), indexability_label(b));
    if ia != ib {
        push("Indexability", &ia, &ib);
    }
    for (field, fa, fb) in [
        ("Title", a.title(), b.title()),
        ("Meta Description", a.description(), b.description()),
        ("H1", a.h1(), b.h1()),
        ("Canonical", a.canonical(), b.canonical()),
    ] {
        if fa.trim() != fb.trim() {
            push(field, &fa, &fb);
        }
    }
    if meta_robots(a) != meta_robots(b) {
        push(
            "Meta Robots",
            &meta_robots(a).join("; "),
            &meta_robots(b).join("; "),
        );
    }
    if words_changed(a.word_count(), b.word_count()) {
        push("Word Count", &a.word_count(), &b.word_count());
    }
    out
}

/// Compare crawl `a` (before) with crawl `b` (after). URLs match exactly as the crawler
/// normalised them. Linear in pages + findings: a 50k-page pair takes well under a second.
pub fn compare(a: &Crawl, audit_a: &Audit, b: &Crawl, audit_b: &Audit) -> Comparison {
    let b_index: FxMap<&str, usize> = b
        .pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.url.as_str(), i))
        .collect();
    let a_to_b: Vec<Option<usize>> = a
        .pages
        .iter()
        .map(|p| b_index.get(p.url.as_str()).copied())
        .collect();
    let mut b_to_a = vec![None; b.pages.len()];
    for (ai, bi) in a_to_b.iter().enumerate() {
        if let Some(bi) = *bi {
            b_to_a[bi] = Some(ai);
        }
    }

    // Per check, stamp the pages it hits with the check index: no clearing between checks.
    let (mut in_a, mut in_b) = (
        vec![usize::MAX; a.pages.len()],
        vec![usize::MAX; b.pages.len()],
    );
    let mut checks = Vec::new();
    for c in 0..catalogue().len() {
        let ha = audit_a.hits.get(c).map(|h| &h[..]).unwrap_or(&[]);
        let hb = audit_b.hits.get(c).map(|h| &h[..]).unwrap_or(&[]);
        if ha.is_empty() && hb.is_empty() {
            continue;
        }
        let ha: Vec<usize> = ha.iter().copied().filter(|&i| i < in_a.len()).collect();
        let hb: Vec<usize> = hb.iter().copied().filter(|&i| i < in_b.len()).collect();
        ha.iter().for_each(|&i| in_a[i] = c);
        hb.iter().for_each(|&i| in_b[i] = c);
        let mut d = CheckDiff {
            check: c,
            ..Default::default()
        };
        for &i in &hb {
            if b_to_a[i].is_some_and(|ai| in_a[ai] == c) {
                d.persisting.push(i);
            } else {
                d.new.push(i);
            }
        }
        d.fixed = ha
            .into_iter()
            .filter(|&i| !a_to_b[i].is_some_and(|bi| in_b[bi] == c))
            .collect();
        checks.push(d);
    }

    let mut urls = Vec::new();
    for (bi, p) in b.pages.iter().enumerate().filter(|(_, p)| p.is_internal) {
        match b_to_a[bi] {
            None => urls.push(UrlDiff {
                url: p.url.clone(),
                change: UrlChange::Added,
                fields: vec![FieldChange {
                    field: "Status",
                    before: String::new(),
                    after: status(p),
                }],
            }),
            Some(ai) => {
                let fields = field_changes(&a.pages[ai], p);
                if !fields.is_empty() {
                    urls.push(UrlDiff {
                        url: p.url.clone(),
                        change: UrlChange::Changed,
                        fields,
                    });
                }
            }
        }
    }
    for (ai, p) in a.pages.iter().enumerate() {
        if p.is_internal && a_to_b[ai].is_none() {
            urls.push(UrlDiff {
                url: p.url.clone(),
                change: UrlChange::Removed,
                fields: vec![FieldChange {
                    field: "Status",
                    before: status(p),
                    after: String::new(),
                }],
            });
        }
    }

    Comparison {
        site_before: a.start_url.clone(),
        site_after: b.start_url.clone(),
        score_before: audit_a.health_score(a),
        score_after: audit_b.health_score(b),
        pages_before: a.pages.len(),
        pages_after: b.pages.len(),
        checks,
        urls,
    }
}

// ------------------------------------------------------------------ exports

/// Per check: URL counts before and after, new, fixed and persisting.
pub fn checks_csv(cmp: &Comparison) -> String {
    let cat = catalogue();
    let mut out = csv_row(&[
        "Issue ID",
        "Category",
        "Issue",
        "Type",
        "Priority",
        "URLs Before",
        "URLs After",
        "New",
        "Fixed",
        "Persisting",
    ]);
    for d in &cmp.checks {
        let c = &cat[d.check];
        let before = d.fixed.len() + d.persisting.len();
        let after = d.new.len() + d.persisting.len();
        out.push_str(&csv_row(&[
            &c.id,
            c.category,
            &c.name,
            c.kind.label(),
            c.priority.label(),
            &before.to_string(),
            &after.to_string(),
            &d.new.len().to_string(),
            &d.fixed.len().to_string(),
            &d.persisting.len().to_string(),
        ]));
    }
    out
}

/// One row per new or fixed (check, URL) pair. Persisting findings are in each crawl's own
/// issue_urls.csv.
pub fn issue_urls_csv(a: &Crawl, b: &Crawl, cmp: &Comparison) -> String {
    let cat = catalogue();
    let mut out = csv_row(&[
        "Change", "Issue ID", "Category", "Issue", "Type", "Priority", "Address",
    ]);
    for d in &cmp.checks {
        let c = &cat[d.check];
        let rows = d
            .new
            .iter()
            .map(|&i| ("New", &b.pages[i]))
            .chain(d.fixed.iter().map(|&i| ("Fixed", &a.pages[i])));
        for (change, p) in rows {
            out.push_str(&csv_row(&[
                change,
                &c.id,
                c.category,
                &c.name,
                c.kind.label(),
                c.priority.label(),
                &p.url,
            ]));
        }
    }
    out
}

/// One row per changed field of an added, removed or changed URL.
pub fn url_changes_csv(cmp: &Comparison) -> String {
    let mut out = csv_row(&["Address", "Change", "Field", "Before", "After"]);
    for u in &cmp.urls {
        for f in &u.fields {
            out.push_str(&csv_row(&[
                &u.url,
                u.change.label(),
                f.field,
                &f.before,
                &f.after,
            ]));
        }
    }
    out
}

fn delta(before: u32, after: u32) -> String {
    match after as i64 - before as i64 {
        0 => "±0".into(),
        d if d > 0 => format!("+{d}"),
        d => d.to_string(),
    }
}

fn score_color(score: u32) -> &'static str {
    if score >= 85 {
        "#1a7f37"
    } else if score >= 60 {
        "#9a6700"
    } else {
        "#cf222e"
    }
}

/// Self-contained HTML comparison report. Every crawl-derived string is escaped.
pub fn html_report(a: &Crawl, b: &Crawl, cmp: &Comparison) -> String {
    let cat = catalogue();
    let (new_k, fixed_k) = cmp.kind_counts();
    let mut sections = String::new();
    for (title, new) in [("New findings", true), ("Fixed findings", false)] {
        let mut list: Vec<&CheckDiff> = cmp
            .checks
            .iter()
            .filter(|d| {
                if new {
                    !d.new.is_empty()
                } else {
                    !d.fixed.is_empty()
                }
            })
            .collect();
        list.sort_by_key(|d| {
            let c = &cat[d.check];
            let n = if new { d.new.len() } else { d.fixed.len() };
            (c.kind, c.priority, std::cmp::Reverse(n))
        });
        let _ = write!(sections, "<h2>{title}</h2>");
        if list.is_empty() {
            let _ = write!(sections, "<p class=muted>None.</p>");
        }
        for d in list {
            let c = &cat[d.check];
            let (idx, crawl) = if new { (&d.new, b) } else { (&d.fixed, a) };
            let mut urls = String::new();
            for &i in idx.iter().take(50) {
                let u = esc(&crawl.pages[i].url);
                let _ = write!(urls, "<li><a href=\"{u}\">{u}</a></li>");
            }
            if idx.len() > 50 {
                let _ = write!(
                    urls,
                    "<li class=muted>…and {} more (see comparison_issue_urls.csv)</li>",
                    idx.len() - 50
                );
            }
            let kind = c.kind.label().to_lowercase();
            let _ = write!(
                sections,
                "<details class=row><summary><span class=\"pill {kind}\">{}</span><span class=\"prio {}\">{}</span><span class=title>{}</span><span class=muted>{}</span><span class=muted>{} URL{} · before {} → after {}</span></summary><div class=body><p>{}</p><p><strong>How to fix:</strong> {}</p><ul class=urls>{urls}</ul></div></details>",
                c.kind.label(),
                c.priority.label().to_lowercase(),
                c.priority.label(),
                esc(&c.name),
                esc(c.category),
                idx.len(),
                if idx.len() == 1 { "" } else { "s" },
                d.fixed.len() + d.persisting.len(),
                d.new.len() + d.persisting.len(),
                esc(&c.description),
                esc(&c.fix),
            );
        }
    }

    let mut rows = String::new();
    let mut shown = 0;
    'outer: for u in &cmp.urls {
        for f in &u.fields {
            if shown == 1000 {
                break 'outer;
            }
            shown += 1;
            let _ = write!(
                rows,
                "<tr><td class=url>{}</td><td><span class=\"chg {}\">{}</span></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&u.url),
                u.change.label().to_lowercase(),
                u.change.label(),
                f.field,
                esc(&f.before),
                esc(&f.after)
            );
        }
    }
    let total_rows: usize = cmp.urls.iter().map(|u| u.fields.len()).sum();
    let more = if total_rows > shown {
        format!(
            "<p class=muted>Showing {shown} of {total_rows} changes; see url_changes.csv for all of them.</p>"
        )
    } else {
        String::new()
    };
    let table = if rows.is_empty() {
        "<p class=muted>No URL was added, removed or changed.</p>".to_string()
    } else {
        format!("<table><thead><tr><th>Address</th><th>Change</th><th>Field</th><th>Before</th><th>After</th></tr></thead><tbody>{rows}</tbody></table>{more}")
    };
    let site = esc(&b.start_url);
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SEO crawl comparison · {site}</title>
<style>
:root {{ --ink:#1f2328; --muted:#59636e; --line:#d1d9e0; --soft:#f6f8fa; --issue:#cf222e; --warning:#9a6700; --opp:#0969da; --good:#1a7f37; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; color:var(--ink); background:#fff; }}
main {{ max-width:1180px; margin:0 auto; padding:40px 24px 64px; }}
header {{ border-bottom:3px solid #2da44e; padding-bottom:20px; margin-bottom:28px; }}
h1 {{ margin:0 0 6px; font-size:28px; }}
h2 {{ font-size:20px; margin:36px 0 12px; }}
.muted {{ color:var(--muted); }}
.cards {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(170px,1fr)); gap:14px; }}
.card {{ background:var(--soft); border:1px solid var(--line); border-radius:10px; padding:16px; }}
.card .label {{ font-size:12px; text-transform:uppercase; letter-spacing:.04em; color:var(--muted); font-weight:600; }}
.card .value {{ font-size:28px; font-weight:700; margin-top:4px; }}
table {{ width:100%; border-collapse:collapse; font-size:13px; }}
th,td {{ text-align:left; padding:6px 8px; border-bottom:1px solid var(--line); vertical-align:top; }}
th {{ font-size:12px; text-transform:uppercase; color:var(--muted); }}
td.url {{ word-break:break-all; }}
.pill {{ display:inline-block; padding:1px 9px; border-radius:999px; font-size:12px; font-weight:600; }}
.pill.issue {{ background:#ffebe9; color:var(--issue); }}
.pill.warning {{ background:#fff8c5; color:var(--warning); }}
.pill.opportunity {{ background:#ddf4ff; color:var(--opp); }}
.chg {{ font-weight:600; }} .chg.added {{ color:var(--good); }} .chg.removed {{ color:var(--issue); }} .chg.changed {{ color:var(--warning); }}
.row {{ border:1px solid var(--line); border-radius:8px; margin-bottom:8px; }}
.row summary {{ cursor:pointer; display:flex; gap:10px; align-items:center; padding:10px 14px; flex-wrap:wrap; }}
.row .title {{ font-weight:600; flex:1; min-width:200px; }}
.prio {{ font-size:12px; font-weight:600; color:var(--muted); }} .prio.high {{ color:var(--issue); }}
.row .body {{ padding:0 14px 12px; border-top:1px solid var(--line); }}
.urls {{ font-size:13px; max-height:320px; overflow:auto; padding-left:20px; }}
.urls a {{ color:var(--opp); word-break:break-all; }}
footer {{ margin-top:48px; padding-top:14px; border-top:1px solid var(--line); color:var(--muted); font-size:13px; }}
footer a {{ color:inherit; }}
@media print {{ .row {{ break-inside:avoid; }} .urls {{ max-height:none; }} }}
</style>
</head>
<body>
<main>
<header>
  <h1>SEO crawl comparison</h1>
  <div class="muted">Before: {before_site} ({before_date}) · After: {site} ({after_date})</div>
</header>
<div class="cards">
  <div class="card"><div class="label">Health score</div><div class="value"><span style="color:{c_before}">{s_before}</span> → <span style="color:{c_after}">{s_after}</span> <span class=muted>({delta})</span></div></div>
  <div class="card"><div class="label">URLs crawled</div><div class="value">{p_before} → {p_after}</div></div>
  <div class="card"><div class="label">Added · Removed · Changed</div><div class="value">{added} · {removed} · {changed}</div></div>
  <div class="card"><div class="label">New issues · warnings · opps</div><div class="value"><span style="color:var(--issue)">{n0}</span> · <span style="color:var(--warning)">{n1}</span> · <span style="color:var(--opp)">{n2}</span></div></div>
  <div class="card"><div class="label">Fixed issues · warnings · opps</div><div class="value"><span style="color:var(--good)">{f0} · {f1} · {f2}</span></div></div>
</div>
{sections}
<h2>URL changes</h2>
{table}
<footer>Generated with <a href="{REPO}">ShoutingToad SEO Spider</a> v{version}.</footer>
</main>
</body>
</html>
"##,
        before_site = esc(&a.start_url),
        before_date = esc(&a.finished),
        after_date = esc(&b.finished),
        c_before = score_color(cmp.score_before),
        c_after = score_color(cmp.score_after),
        s_before = cmp.score_before,
        s_after = cmp.score_after,
        delta = delta(cmp.score_before, cmp.score_after),
        p_before = cmp.pages_before,
        p_after = cmp.pages_after,
        added = cmp.count(UrlChange::Added),
        removed = cmp.count(UrlChange::Removed),
        changed = cmp.count(UrlChange::Changed),
        n0 = new_k[0],
        n1 = new_k[1],
        n2 = new_k[2],
        f0 = fixed_k[0],
        f1 = fixed_k[1],
        f2 = fixed_k[2],
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// Escape text for a GitHub-flavoured Markdown table cell.
fn md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '#' | '~' | '!' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// A URL as inline code in a table cell (`|` would end the cell even inside code).
fn md_url(u: &str) -> String {
    format!("`{}`", u.replace('|', "%7C").replace('`', "%60"))
}

/// Markdown summary for a pull request comment or `$GITHUB_STEP_SUMMARY`.
pub fn markdown_summary(
    b: &Crawl,
    audit_b: &Audit,
    cmp: Option<(&Crawl, &Comparison)>,
    fail_on_new: Option<Priority>,
) -> String {
    let cat = catalogue();
    let score = audit_b.health_score(b);
    let mut out = format!("## ShoutingToad SEO check: {}\n\n", md(&b.start_url));
    let Some((a, cmp)) = cmp else {
        let issues = audit_b
            .triggered()
            .filter(|(i, _)| cat[*i].kind == Kind::Issue)
            .count();
        let _ = writeln!(
            out,
            "**Health score:** {score}/100 · **URLs:** {} · **Issues:** {issues}\n\nNo baseline crawl to compare with yet: this crawl becomes the baseline.",
            b.pages.len()
        );
        return out;
    };
    let _ = writeln!(
        out,
        "**Health score:** {}/100 (was {}, {}) · **URLs:** {} (was {}): {} added, {} removed, {} changed\n",
        cmp.score_after,
        cmp.score_before,
        delta(cmp.score_before, cmp.score_after),
        cmp.pages_after,
        cmp.pages_before,
        cmp.count(UrlChange::Added),
        cmp.count(UrlChange::Removed),
        cmp.count(UrlChange::Changed),
    );
    let (new_k, fixed_k) = cmp.kind_counts();
    let _ = writeln!(
        out,
        "| | Issues | Warnings | Opportunities |\n|---|---:|---:|---:|\n| New | {} | {} | {} |\n| Fixed | {} | {} | {} |\n",
        new_k[0], new_k[1], new_k[2], fixed_k[0], fixed_k[1], fixed_k[2]
    );
    if let Some(level) = fail_on_new {
        let failing = cmp.new_issues(level).count();
        let _ = writeln!(
            out,
            "{}\n",
            if failing > 0 {
                format!(
                    "**Failed:** {failing} new issue type(s) at {} priority or above.",
                    level.label()
                )
            } else {
                format!(
                    "**Passed:** no new issues at {} priority or above.",
                    level.label()
                )
            }
        );
    }
    for (title, new) in [("New", true), ("Fixed", false)] {
        let mut list: Vec<&CheckDiff> = cmp
            .checks
            .iter()
            .filter(|d| {
                if new {
                    !d.new.is_empty()
                } else {
                    !d.fixed.is_empty()
                }
            })
            .collect();
        if list.is_empty() {
            continue;
        }
        list.sort_by_key(|d| {
            let c = &cat[d.check];
            (
                c.kind,
                c.priority,
                std::cmp::Reverse(d.new.len() + d.fixed.len()),
            )
        });
        let _ = writeln!(
            out,
            "### {title} findings\n\n| Type | Priority | Finding | URLs | Example |\n|---|---|---|---:|---|"
        );
        for d in list.iter().take(25) {
            let c = &cat[d.check];
            let (idx, crawl) = if new { (&d.new, b) } else { (&d.fixed, a) };
            let _ = writeln!(
                out,
                "| {} | {} | {}: {} | {} | {} |",
                c.kind.label(),
                c.priority.label(),
                md(c.category),
                md(&c.name),
                idx.len(),
                md_url(&crawl.pages[idx[0]].url)
            );
        }
        if list.len() > 25 {
            let _ = writeln!(out, "\n…and {} more.", list.len() - 25);
        }
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "<sub>Generated by [ShoutingToad]({REPO}) v{}. Full details in comparison.html.</sub>",
        env!("CARGO_PKG_VERSION")
    );
    out
}

/// Shields-style "SEO | 94/100" badge, coloured by score.
pub fn badge_svg(score: u32) -> String {
    let (label, value) = ("SEO", format!("{score}/100"));
    let color = match score {
        90.. => "#4c1",
        80..=89 => "#97ca00",
        70..=79 => "#a4a61d",
        60..=69 => "#dfb317",
        50..=59 => "#fe7d37",
        _ => "#e05d44",
    };
    // Verdana 11px is close to the Arial metrics the title checks use.
    let w = |s: &str| audit::pixel_width(s, 11.0) + 12;
    let (lw, vw) = (w(label), w(&value));
    let total = lw + vw;
    let title = format!("SEO health score: {score}/100, audited by ShoutingToad");
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total}" height="20" role="img" aria-label="{title}"><title>{title}</title><linearGradient id="s" x2="0" y2="100%"><stop offset="0" stop-color="#bbb" stop-opacity=".1"/><stop offset="1" stop-opacity=".1"/></linearGradient><clipPath id="r"><rect width="{total}" height="20" rx="3" fill="#fff"/></clipPath><g clip-path="url(#r)"><rect width="{lw}" height="20" fill="#555"/><rect x="{lw}" width="{vw}" height="20" fill="{color}"/><rect width="{total}" height="20" fill="url(#s)"/></g><g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11"><text x="{lx}" y="15" fill="#010101" fill-opacity=".3">{label}</text><text x="{lx}" y="14">{label}</text><text x="{vx}" y="15" fill="#010101" fill-opacity=".3">{value}</text><text x="{vx}" y="14">{value}</text></g></svg>
"##,
        lx = lw as f32 / 2.0,
        vx = lw as f32 + vw as f32 / 2.0,
    )
}

/// Write comparison.html, comparison.csv, comparison_issue_urls.csv and url_changes.csv.
pub fn write_reports(dir: &Path, a: &Crawl, b: &Crawl, cmp: &Comparison) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    for (name, data) in [
        ("comparison.html", html_report(a, b, cmp)),
        ("comparison.csv", checks_csv(cmp)),
        ("comparison_issue_urls.csv", issue_urls_csv(a, b, cmp)),
        ("url_changes.csv", url_changes_csv(cmp)),
    ] {
        write(&dir.join(name), &data)?;
    }
    Ok(())
}

fn write(path: &Path, data: &str) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("could not write {}: {e}", path.display()))
}

// ------------------------------------------------------------------ CLI

/// The regression-gate options shared by `crawl`, `list` and `compare`.
#[derive(Default)]
pub struct Gate {
    pub baseline: Option<PathBuf>,
    pub fail_on_new: Option<Priority>,
    pub summary_md: Option<PathBuf>,
    pub badge: Option<PathBuf>,
}

impl Gate {
    /// Handle one of the gate options; false when `opt` is not one of them.
    pub fn option(
        &mut self,
        opt: &str,
        val: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Result<bool, String> {
        match opt {
            "--baseline" => self.baseline = Some(val(opt)?.into()),
            "--fail-on-new" => self.fail_on_new = Some(parse_level(&val(opt)?)?),
            "--summary-md" => self.summary_md = Some(val(opt)?.into()),
            "--badge" => self.badge = Some(val(opt)?.into()),
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.fail_on_new.is_some() && self.baseline.is_none() {
            return Err("--fail-on-new needs --baseline <previous.shoutingtoad>".into());
        }
        Ok(())
    }

    /// Compare with the baseline (a missing baseline file is a first run, not an error), write
    /// the reports, summary and badge. True when `--fail-on-new` is reached.
    pub fn run(&self, b: &Crawl, audit_b: &Audit, out: &Path) -> Result<bool, String> {
        let a = match &self.baseline {
            Some(p) if p.exists() => Some(Crawl::load(p)?),
            Some(p) => {
                eprintln!(
                    "note: baseline {} does not exist yet; nothing to compare with",
                    p.display()
                );
                None
            }
            None => None,
        };
        let audit_a = a.as_ref().map(audit::run);
        let cmp = a
            .as_ref()
            .zip(audit_a.as_ref())
            .map(|(a, aa)| compare(a, aa, b, audit_b));
        let failed = self.finish(a.as_ref().zip(cmp.as_ref()), b, audit_b, out);
        // see cli.rs: freeing a big crawl one allocation at a time takes seconds
        std::mem::forget((a, audit_a, cmp));
        failed
    }

    fn finish(
        &self,
        cmp: Option<(&Crawl, &Comparison)>,
        b: &Crawl,
        audit_b: &Audit,
        out: &Path,
    ) -> Result<bool, String> {
        let mut failed = false;
        if let Some((a, cmp)) = cmp {
            write_reports(out, a, b, cmp)?;
            let (new_k, fixed_k) = cmp.kind_counts();
            println!(
                "Compared with {}: health {} -> {} ({}), {} added, {} removed, {} changed URLs",
                a.start_url,
                cmp.score_before,
                cmp.score_after,
                delta(cmp.score_before, cmp.score_after),
                cmp.count(UrlChange::Added),
                cmp.count(UrlChange::Removed),
                cmp.count(UrlChange::Changed)
            );
            println!(
                "  new: {} issues, {} warnings, {} opportunities; fixed: {} issues, {} warnings, {} opportunities",
                new_k[0], new_k[1], new_k[2], fixed_k[0], fixed_k[1], fixed_k[2]
            );
            if let Some(level) = self.fail_on_new {
                let cat = catalogue();
                for d in cmp.new_issues(level) {
                    let c = &cat[d.check];
                    println!(
                        "  NEW [{} {}] {}: {} ({} URLs, e.g. {})",
                        c.kind.label(),
                        c.priority.label(),
                        c.category,
                        c.name,
                        d.new.len(),
                        b.pages[d.new[0]].url
                    );
                    failed = true;
                }
            }
        }
        if let Some(p) = &self.summary_md {
            write(p, &markdown_summary(b, audit_b, cmp, self.fail_on_new))?;
        }
        if let Some(p) = &self.badge {
            write(p, &badge_svg(audit_b.health_score(b)))?;
        }
        Ok(failed)
    }
}

/// `shouting-toad compare BEFORE.shoutingtoad AFTER.shoutingtoad --out DIR [gate options]`
pub fn cli(args: &[String]) -> Result<i32, String> {
    let mut gate = Gate::default();
    let mut out = PathBuf::from("shoutingtoad-comparison");
    let mut files = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        if a == "--out" {
            out = val("--out")?.into();
        } else if !gate.option(a, &mut val)? {
            if a.starts_with("--") {
                return Err(format!("unknown option '{a}'"));
            }
            files.push(PathBuf::from(a));
        }
    }
    let [before, after] = &files[..] else {
        return Err("compare needs two crawl files: BEFORE.shoutingtoad AFTER.shoutingtoad".into());
    };
    if gate.baseline.is_some() {
        return Err("compare takes the baseline as its first file, not --baseline".into());
    }
    let t = std::time::Instant::now();
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| Crawl::load(before));
        let b = Crawl::load(after);
        (a.join().unwrap_or_else(|_| Err("loader crashed".into())), b)
    });
    let (a, b) = (a?, b?);
    let t_audit = std::time::Instant::now();
    let (audit_a, audit_b) = std::thread::scope(|s| {
        let x = s.spawn(|| audit::run(&a));
        (x.join().unwrap_or_default(), audit::run(&b))
    });
    let t_cmp = std::time::Instant::now();
    let cmp = compare(&a, &audit_a, &b, &audit_b);
    let cmp_secs = t_cmp.elapsed().as_secs_f32();
    let failed = gate.finish(Some((&a, &cmp)), &b, &audit_b, &out)?;
    println!(
        "Loaded in {:.1}s, audited in {:.1}s, compared in {cmp_secs:.2}s, {:.1}s in total",
        t_audit.duration_since(t).as_secs_f32(),
        t_cmp.duration_since(t_audit).as_secs_f32(),
        t.elapsed().as_secs_f32()
    );
    println!("Reports written to {}", out.display());
    std::mem::forget((a, b, audit_a, audit_b, cmp));
    if failed {
        eprintln!("Failing: new issues at or above the --fail-on-new threshold were found.");
        return Ok(2);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HtmlData;

    fn page(id: usize, url: &str, title: &str, words: usize) -> Page {
        Page {
            id,
            url: url.into(),
            is_internal: true,
            status: 200,
            content_type: "text/html".into(),
            html: Some(Box::new(HtmlData {
                titles: vec![title.into()],
                word_count: words,
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[test]
    fn matches_findings_and_urls_by_address() {
        let a = Crawl {
            pages: vec![
                page(0, "https://a.test/", "Home", 300),
                page(1, "https://a.test/old", "Old", 300),
                page(2, "https://a.test/same", "Same", 300),
            ],
            ..Default::default()
        };
        let mut b = Crawl {
            pages: vec![
                page(0, "https://a.test/", "Home page", 305),
                page(1, "https://a.test/same", "Same", 100),
                page(2, "https://a.test/new", "New", 300),
            ],
            ..Default::default()
        };
        b.pages[1].status = 404;
        let n = catalogue().len();
        let mut aa = Audit {
            hits: vec![Vec::new(); n],
            ..Default::default()
        };
        let mut ab = aa.clone();
        aa.hits[0] = vec![0, 1]; // home + old
        ab.hits[0] = vec![0, 2]; // home + new
        aa.hits[1] = vec![2];
        let cmp = compare(&a, &aa, &b, &ab);
        let d0 = &cmp.checks[0];
        assert_eq!(
            (d0.new.clone(), d0.fixed.clone(), d0.persisting.clone()),
            (vec![2], vec![1], vec![0])
        );
        let d1 = &cmp.checks[1];
        assert_eq!((d1.new.len(), d1.fixed.clone()), (0, vec![2]));
        assert_eq!(cmp.checks.len(), 2);
        let kinds: Vec<(&str, UrlChange)> = cmp
            .urls
            .iter()
            .map(|u| (u.url.as_str(), u.change))
            .collect();
        assert_eq!(
            kinds,
            [
                ("https://a.test/", UrlChange::Changed),
                ("https://a.test/same", UrlChange::Changed),
                ("https://a.test/new", UrlChange::Added),
                ("https://a.test/old", UrlChange::Removed),
            ]
        );
        // home: title changed, the 5-word drift is not a change
        let home: Vec<&str> = cmp.urls[0].fields.iter().map(|f| f.field).collect();
        assert_eq!(home, ["Title"]);
        let same: Vec<&str> = cmp.urls[1].fields.iter().map(|f| f.field).collect();
        assert_eq!(same, ["Status", "Indexability", "Word Count"]);
    }

    #[test]
    fn word_count_threshold() {
        assert!(!words_changed(1000, 1050));
        assert!(!words_changed(50, 58));
        assert!(words_changed(1000, 1200));
        assert!(words_changed(0, 300));
    }

    #[test]
    fn badge_is_escaped_svg_with_title() {
        let s = badge_svg(94);
        assert!(s.starts_with("<svg") && s.trim_end().ends_with("</svg>"));
        assert!(s.contains("<title>SEO health score: 94/100") && s.contains("#4c1"));
        assert!(badge_svg(42).contains("#e05d44"));
        assert!(roxmltree::Document::parse(&s).is_ok());
    }

    #[test]
    fn markdown_cells_are_escaped() {
        assert_eq!(md("a|b <x> *y*"), "a\\|b \\<x\\> \\*y\\*");
        assert_eq!(md_url("https://a.test/x|y"), "`https://a.test/x%7Cy`");
    }
}
