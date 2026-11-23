//! WCAG 2.2 mapping of the accessibility-related checks, and the "Accessibility (WCAG 2.2 /
//! EAA) starter report". Automated checks cover only part of WCAG; the report says so.

use crate::audit::{ih, Audit};
use crate::checks::{catalogue, find};
use crate::model::Crawl;
use crate::report::esc;
use std::collections::BTreeSet;
use std::fmt::Write;

pub struct Criterion {
    pub id: &'static str,
    pub name: &'static str,
    pub level: &'static str,
}

const fn c(id: &'static str, name: &'static str, level: &'static str) -> Criterion {
    Criterion { id, name, level }
}

/// WCAG 2.2 success criteria that at least one check relates to.
pub const CRITERIA: &[Criterion] = &[
    c("1.1.1", "Non-text Content", "A"),
    c("1.2.2", "Captions (Prerecorded)", "A"),
    c("1.3.1", "Info and Relationships", "A"),
    c("1.4.2", "Audio Control", "A"),
    c("1.4.3", "Contrast (Minimum)", "AA"),
    c("1.4.4", "Resize Text", "AA"),
    c("1.4.10", "Reflow", "AA"),
    c("2.1.1", "Keyboard", "A"),
    c("2.2.1", "Timing Adjustable", "A"),
    c("2.4.1", "Bypass Blocks", "A"),
    c("2.4.2", "Page Titled", "A"),
    c("2.4.3", "Focus Order", "A"),
    c("2.4.4", "Link Purpose (In Context)", "A"),
    c("2.4.7", "Focus Visible", "AA"),
    c("3.1.1", "Language of Page", "A"),
    c("4.1.2", "Name, Role, Value", "A"),
];

/// Level A and AA success criteria in WCAG 2.2 (4.1.1 Parsing is obsolete and not counted).
pub const A_AA_TOTAL: usize = 55;

/// (check id, success criterion). A check can relate to several criteria. The mapping says a
/// failed check is evidence of a problem under that criterion, not that passing it meets it.
pub const MAP: &[(&str, &str)] = &[
    ("images.missing_alt_attribute", "1.1.1"),
    ("images.generic_alt", "1.1.1"),
    ("images.filename_alt", "1.1.1"),
    ("links.image_link_no_alt", "1.1.1"),
    ("links.image_link_no_alt", "2.4.4"),
    ("a11y.video_captions", "1.2.2"),
    ("a11y.table_headers", "1.3.1"),
    ("a11y.inputs_without_label", "1.3.1"),
    ("a11y.inputs_without_label", "4.1.2"),
    ("a11y.duplicate_ids", "1.3.1"),
    ("a11y.aria_hidden_body", "1.3.1"),
    ("a11y.aria_hidden_body", "4.1.2"),
    ("a11y.autoplay", "1.4.2"),
    ("a11y.low_contrast", "1.4.3"),
    ("mobile.zoom_disabled", "1.4.4"),
    ("mobile.missing_viewport", "1.4.10"),
    ("mobile.viewport_not_device_width", "1.4.10"),
    ("a11y.click_not_keyboard", "2.1.1"),
    ("redirects.meta_refresh", "2.2.1"),
    ("redirects.meta_refresh_reload", "2.2.1"),
    ("a11y.missing_main", "2.4.1"),
    ("titles.missing", "2.4.2"),
    ("a11y.positive_tabindex", "2.4.3"),
    ("a11y.links_without_text", "2.4.4"),
    ("a11y.links_without_text", "4.1.2"),
    ("links.non_descriptive_anchor", "2.4.4"),
    ("a11y.focus_not_visible", "2.4.7"),
    ("a11y.missing_lang", "3.1.1"),
    ("a11y.invalid_lang", "3.1.1"),
    ("a11y.buttons_without_name", "4.1.2"),
    ("a11y.iframe_title", "4.1.2"),
];

/// Checks that need a rendered page (JavaScript rendering on).
const RENDERED: &[&str] = &[
    "a11y.low_contrast",
    "a11y.focus_not_visible",
    "a11y.click_not_keyboard",
];

/// Criteria a check relates to, e.g. "1.1.1 (A)".
pub fn criteria_of(check_id: &str) -> Vec<&'static Criterion> {
    MAP.iter()
        .filter(|(c, _)| *c == check_id)
        .filter_map(|(_, sc)| CRITERIA.iter().find(|k| k.id == *sc))
        .collect()
}

pub struct Row {
    pub criterion: &'static Criterion,
    /// Catalogue indices of the related checks.
    pub checks: Vec<usize>,
    /// Pages these checks ran on.
    pub tested: usize,
    /// Page indices failing at least one related check.
    pub failing: Vec<usize>,
}

pub fn rows(crawl: &Crawl, audit: &Audit) -> Vec<Row> {
    let html = crawl.pages.iter().filter(|p| ih(p)).count();
    let rendered = crawl
        .pages
        .iter()
        .filter(|p| ih(p) && p.rendered.as_ref().is_some_and(|r| r.a11y_tested))
        .count();
    CRITERIA
        .iter()
        .map(|cr| {
            let ids: Vec<&str> = MAP
                .iter()
                .filter(|(_, sc)| *sc == cr.id)
                .map(|(c, _)| *c)
                .collect();
            let checks: Vec<usize> = ids.iter().filter_map(|id| find(id)).collect();
            let tested = if ids.iter().all(|id| RENDERED.contains(id)) {
                rendered
            } else {
                html
            };
            let failing: BTreeSet<usize> = checks
                .iter()
                .flat_map(|&c| audit.hits.get(c).into_iter().flatten().copied())
                .collect();
            Row {
                criterion: cr,
                checks,
                tested,
                failing: failing.into_iter().collect(),
            }
        })
        .collect()
}

fn status(r: &Row) -> &'static str {
    if r.tested == 0 {
        "Not tested"
    } else if r.failing.is_empty() {
        "No failures found"
    } else {
        "Fails"
    }
}

const HONEST_NOTE: &str =
    "Automated testing finds only part of the accessibility barriers on a site: \
     it covers some WCAG success criteria, and only partly. A page with no failures here is not \
     therefore accessible or compliant. Keyboard use, screen readers, zoom and reflow, content \
     meaning, forms and errors, captions and media all need manual testing by people, ideally \
     including disabled users. This is a starting point, not a conformance or EAA compliance \
     statement.";

/// CSV: one row per criterion and failing URL (criteria without failures get one row).
pub fn csv(crawl: &Crawl, audit: &Audit) -> String {
    let cat = catalogue();
    let q = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::from(
        "Criterion,Name,Level,Status,Pages Tested,Pages Failing,Check IDs,URL,Failed Checks\n",
    );
    for r in rows(crawl, audit) {
        let ids = r
            .checks
            .iter()
            .map(|&c| cat[c].id.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let head = format!(
            "{},{},{},{},{},{},{}",
            r.criterion.id,
            q(r.criterion.name),
            r.criterion.level,
            status(&r),
            r.tested,
            r.failing.len(),
            q(&ids)
        );
        if r.failing.is_empty() {
            let _ = writeln!(out, "{head},,");
            continue;
        }
        for &pi in &r.failing {
            let failed: Vec<&str> = r
                .checks
                .iter()
                .filter(|&&c| audit.hits[c].binary_search(&pi).is_ok())
                .map(|&c| cat[c].id.as_str())
                .collect();
            let _ = writeln!(
                out,
                "{head},{},{}",
                q(&crawl.pages[pi].url),
                q(&failed.join(" "))
            );
        }
    }
    out
}

/// Self-contained HTML starter report.
pub fn html(crawl: &Crawl, audit: &Audit) -> String {
    let cat = catalogue();
    let rows = rows(crawl, audit);
    let site = esc(&crawl.start_url);
    let date = chrono::Local::now().format("%d %B %Y");
    let html_pages = crawl.pages.iter().filter(|p| ih(p)).count();
    let rendered = crawl
        .pages
        .iter()
        .any(|p| p.rendered.as_ref().is_some_and(|r| r.a11y_tested));
    let failing_criteria = rows.iter().filter(|r| !r.failing.is_empty()).count();
    let mut table = String::new();
    let mut details = String::new();
    for r in &rows {
        let st = status(r);
        let cls = match st {
            "Fails" => "fail",
            "Not tested" => "muted",
            _ => "pass",
        };
        let names: Vec<String> = r.checks.iter().map(|&c| esc(&cat[c].name)).collect();
        let _ = write!(
            table,
            "<tr><td><strong>{}</strong> {}</td><td>{}</td><td class={cls}>{st}</td><td class=num>{}</td><td class=num>{}</td><td class=num>{}</td><td class=small>{}</td></tr>",
            r.criterion.id,
            esc(r.criterion.name),
            r.criterion.level,
            r.tested,
            r.tested.saturating_sub(r.failing.len()),
            r.failing.len(),
            names.join("; ")
        );
        if r.failing.is_empty() {
            continue;
        }
        let mut urls = String::new();
        for &pi in r.failing.iter().take(50) {
            let failed: Vec<String> = r
                .checks
                .iter()
                .filter(|&&c| audit.hits[c].binary_search(&pi).is_ok())
                .map(|&c| esc(&cat[c].name))
                .collect();
            let u = esc(&crawl.pages[pi].url);
            let _ = write!(
                urls,
                "<li><a href=\"{u}\">{u}</a> <span class=muted>({})</span></li>",
                failed.join(", ")
            );
        }
        if r.failing.len() > 50 {
            let _ = write!(
                urls,
                "<li class=muted>…and {} more (see accessibility.csv)</li>",
                r.failing.len() - 50
            );
        }
        let _ = write!(
            details,
            "<details><summary><strong>{} {}</strong> (Level {}) · {} page{}</summary><ul>{urls}</ul></details>",
            r.criterion.id,
            esc(r.criterion.name),
            r.criterion.level,
            r.failing.len(),
            if r.failing.len() == 1 { "" } else { "s" }
        );
    }
    let render_note = if rendered {
        "JavaScript rendering was on, so colour contrast (1.4.3), visible keyboard focus (2.4.7) and mouse-only click handlers (2.1.1) were measured in a real browser."
    } else {
        "JavaScript rendering was off, so colour contrast (1.4.3), visible focus (2.4.7) and keyboard access to click handlers (2.1.1) were not measured. Turn rendering on in Spider settings (or --render) to include them."
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Accessibility starter report · {site}</title>
<style>
body {{ margin:0; font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; color:#1f2328; background:#fff; }}
main {{ max-width:1080px; margin:0 auto; padding:40px 24px 64px; }}
h1 {{ margin:0 0 6px; font-size:28px; }}
h2 {{ font-size:20px; margin:32px 0 12px; }}
.sub, .muted {{ color:#59636e; }}
.note {{ border:2px solid #9a6700; background:#fff8c5; border-radius:10px; padding:14px 18px; margin:20px 0; }}
.note strong {{ display:block; font-size:16px; margin-bottom:4px; }}
table {{ width:100%; border-collapse:collapse; }}
th, td {{ text-align:left; padding:8px 10px; border-bottom:1px solid #d1d9e0; vertical-align:top; }}
th {{ font-size:12px; text-transform:uppercase; color:#59636e; }}
td.num, th.num {{ text-align:right; }}
.small {{ font-size:13px; color:#59636e; }}
.fail {{ color:#cf222e; font-weight:600; }}
.pass {{ color:#1a7f37; }}
details {{ border:1px solid #d1d9e0; border-radius:8px; margin-bottom:8px; padding:8px 14px; }}
summary {{ cursor:pointer; }}
li {{ word-break:break-all; font-size:13px; }}
a {{ color:#0969da; }}
footer {{ margin-top:40px; padding-top:12px; border-top:1px solid #d1d9e0; color:#59636e; font-size:13px; }}
</style>
</head>
<body>
<main>
<h1>Accessibility starter report (WCAG 2.2 / EAA)</h1>
<div class="sub">{site} · {date} · {html_pages} HTML pages tested</div>
<div class="note"><strong>Automated testing covers only part of WCAG. Manual testing is required.</strong>{note}</div>
<p>The European Accessibility Act (EAA) refers to the harmonised standard EN 301 549, whose web requirements are the WCAG Level A and AA success criteria. This report relates ShoutingToad's automated checks to {covered} of the {total} Level A and AA success criteria in WCAG 2.2; the other {uncovered} cannot be checked by this tool at all. Failures found: <strong>{failing_criteria}</strong> of the {covered} criteria checked.</p>
<p class="muted">{render_note}</p>
<h2>Success criteria with automated checks</h2>
<table><thead><tr><th>Success criterion</th><th>Level</th><th>Result</th><th class=num>Pages tested</th><th class=num>Passed</th><th class=num>Failed</th><th>Related checks</th></tr></thead><tbody>{table}</tbody></table>
<p class="muted">"No failures found" means the automated checks for that criterion found nothing on the pages tested. It does not mean the criterion is met.</p>
<h2>Affected URLs</h2>
{details_or_none}
<footer>Generated with <a href="{repo}">ShoutingToad SEO Spider</a> v{version}. This report is not legal advice and not a conformance claim.</footer>
</main>
</body>
</html>
"#,
        note = esc(HONEST_NOTE),
        covered = CRITERIA.len(),
        total = A_AA_TOTAL,
        uncovered = A_AA_TOTAL - CRITERIA.len(),
        details_or_none = if details.is_empty() {
            "<p>No failures were found by the automated checks.</p>".to_string()
        } else {
            details
        },
        repo = crate::report::REPO,
        version = env!("CARGO_PKG_VERSION"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_consistent() {
        for (id, sc) in MAP {
            assert!(find(id).is_some(), "unknown check {id}");
            assert!(
                CRITERIA.iter().any(|c| c.id == *sc),
                "unknown criterion {sc}"
            );
        }
        for c in CRITERIA {
            assert!(c.level == "A" || c.level == "AA", "{} {}", c.id, c.level);
            assert!(
                MAP.iter().any(|(_, sc)| *sc == c.id),
                "{} has no checks",
                c.id
            );
        }
        // every check in the Accessibility category is mapped
        for k in catalogue().iter().filter(|k| k.category == "Accessibility") {
            assert!(!criteria_of(&k.id).is_empty(), "{} is not mapped", k.id);
        }
        assert_eq!(criteria_of("a11y.low_contrast")[0].id, "1.4.3");
        assert_eq!(criteria_of("a11y.focus_not_visible")[0].level, "AA");
        assert!(CRITERIA.len() < A_AA_TOTAL);
    }

    #[test]
    fn report_counts_and_is_honest() {
        let url = url::Url::parse("https://e.com/").unwrap();
        let page = |id: usize, path: &str, html: &str| crate::model::Page {
            id,
            url: format!("https://e.com{path}"),
            is_internal: true,
            status: 200,
            source: Some(if id == 1 {
                crate::model::Source::Seed
            } else {
                crate::model::Source::Link
            }),
            html: Some(Box::new(crate::parse::parse_html(html, &url))),
            ..Default::default()
        };
        let crawl = Crawl {
            start_url: "https://e.com/".into(),
            pages: vec![
                page(
                    1,
                    "/",
                    "<html lang=en><title>Home</title><main><img src=a.png></main></html>",
                ),
                page(
                    2,
                    "/b",
                    "<html lang=en><title>B</title><main>hi</main></html>",
                ),
            ],
            ..Default::default()
        };
        let audit = crate::audit::run(&crawl);
        let r = rows(&crawl, &audit);
        let alt = r.iter().find(|r| r.criterion.id == "1.1.1").unwrap();
        assert_eq!((alt.tested, alt.failing.len()), (2, 1));
        // rendered-only criteria are not tested without rendering
        let contrast = r.iter().find(|r| r.criterion.id == "1.4.3").unwrap();
        assert_eq!(contrast.tested, 0);
        let h = html(&crawl, &audit);
        assert!(h.contains("Manual testing is required"));
        assert!(h.contains("https://e.com/"));
        let c = csv(&crawl, &audit);
        assert!(c
            .lines()
            .any(|l| l.starts_with("1.1.1,") && l.contains("https://e.com/")));
        assert!(c
            .lines()
            .any(|l| l.starts_with("1.4.3,") && l.contains("Not tested")));
    }
}
