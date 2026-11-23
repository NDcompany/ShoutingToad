//! Raw HTML vs rendered DOM: what crawlers that do not run JavaScript (most AI crawlers) see,
//! against what browsers and Google see. Only pages rendered with JavaScript rendering on.

use super::*;
use crate::render::Rendered;

/// Raw data and a successful render of an internal 200 HTML page.
fn both(p: &Page) -> Option<(&HtmlData, &Rendered)> {
    let h = hd(p)?;
    let r = p.rendered.as_deref()?;
    r.error.is_none().then_some((h, r))
}

/// At least 50 words, and a quarter of the rendered text, appear only after JavaScript.
pub fn js_content_gap(raw_words: usize, rendered_words: usize) -> bool {
    let gap = rendered_words.saturating_sub(raw_words);
    gap >= 50 && gap * 4 >= rendered_words
}

/// Robots directives as a sorted set, so order and spacing do not count as a change.
fn directives(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .split([',', ';'])
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn add(v: &mut Vec<Check>) {
    const CAT: &str = "Rendering";
    add!(v, "render.content_js_only", CAT, Warning, High, "Content Only Visible After JavaScript",
        "A large share of the page's text appears only after JavaScript runs. Google renders pages, but later and not always; most AI crawlers and many tools read only the raw HTML, so they miss this content.",
        "Render the main content on the server (SSR, static generation or pre-rendering) so it is in the HTML response.",
        |p, _| both(p).is_some_and(|(h, r)| js_content_gap(h.word_count, r.word_count)));
    add!(v, "render.links_js_only", CAT, Warning, Medium, "Internal Links Only in Rendered HTML",
        "Internal links that exist only after JavaScript runs. Crawlers that do not render cannot follow them, and Google discovers them later.",
        "Output navigation and internal links as plain <a href> elements in the HTML response.",
        |p, _| both(p).is_some_and(|(_, r)| {
            let site = bare_host(&p.url);
            r.new_links.iter().any(|u| bare_host(u) == site)
        }));
    add!(v, "render.title_changed", CAT, Warning, Medium, "Title Changed by JavaScript",
        "The <title> after rendering differs from the one in the raw HTML, so crawlers that do not render see a different title from Google.",
        "Serve the final title in the HTML response.",
        |p, _| both(p).is_some_and(|(h, r)| first(&h.titles).trim() != r.title.trim()));
    add!(v, "render.h1_changed", CAT, Warning, Low, "H1 Changed by JavaScript",
        "The first H1 after rendering differs from the raw HTML (added, removed or rewritten by JavaScript).",
        "Serve the final H1 in the HTML response.",
        |p, _| both(p).is_some_and(|(h, r)| first(&h.h1).trim() != r.h1.trim()));
    add!(v, "render.canonical_changed", CAT, Issue, High, "Canonical Changed by JavaScript",
        "The canonical URL after rendering differs from the raw HTML (added, removed or changed). Google may use either one, and crawlers that do not render only see the raw one.",
        "Put the correct canonical in the HTML response and do not change it with JavaScript.",
        |p, _| both(p).is_some_and(|(h, r)| first(&h.canonicals) != r.canonical));
    add!(v, "render.robots_changed", CAT, Issue, High, "Meta Robots Changed by JavaScript",
        "The meta robots directives after rendering differ from the raw HTML. Google applies the stricter of the two, so a noindex added or removed by JavaScript is unreliable.",
        "Serve the intended robots directives in the HTML response and do not change them with JavaScript.",
        |p, _| both(p).is_some_and(|(h, r)| directives(&h.meta_robots.join(",")) != directives(&r.meta_robots)));
    add!(v, "render.js_errors", CAT, Warning, Medium, "JavaScript Errors",
        "The page logged uncaught exceptions or console errors while rendering. Errors can stop content, links or tracking from loading.",
        "Open the page with the browser console open, and fix the errors listed in the inspector.",
        |p, _| ih(p) && p.rendered.as_deref().is_some_and(|r| !r.js_errors.is_empty()));
    add!(v, "render.timeout", CAT, Warning, Medium, "Render Timeout",
        "The page had not finished loading when the render time limit ran out (the timeout setting). Slow or never-ending requests delay rendering for Google too.",
        "Find the slow or hanging requests (scripts, APIs, third parties) and make the page finish loading quickly.",
        |p, _| ih(p) && p.rendered.as_deref().is_some_and(|r| r.timed_out));
    add!(v, "render.failed", CAT, Warning, Low, "Page Could Not Be Rendered",
        "The browser could not render this page (navigation error or crash), so rendered checks were skipped for it.",
        "Open the page in a browser and check that it loads; see the error in the inspector.",
        |p, _| ih(p) && p.rendered.as_deref().is_some_and(|r| r.error.is_some() && !r.timed_out));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_gap_threshold() {
        assert!(js_content_gap(10, 400));
        assert!(js_content_gap(0, 50));
        assert!(!js_content_gap(0, 49));
        // 60 more words on a 2000-word page is not "JS-only content"
        assert!(!js_content_gap(1940, 2000));
        assert!(js_content_gap(1000, 1400));
        assert!(!js_content_gap(500, 400));
        assert_eq!(directives("NOINDEX, follow"), directives("follow,noindex"));
    }
}
