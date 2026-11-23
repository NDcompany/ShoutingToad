//! End-to-end audit of a local fixture website: every check category has pages
//! built to trigger it, plus a perfect page that must stay clean.

mod support;

use shouting_toad::checks::{catalogue, CATEGORIES};
use std::collections::BTreeSet;
use support::*;

const STATUS_CODES: [u16; 17] = [
    400, 401, 403, 404, 405, 406, 408, 410, 418, 429, 451, 500, 501, 502, 503, 504, 599,
];
const REDIRECT_CODES: [u16; 5] = [301, 302, 303, 307, 308];

fn ld(json: &str) -> String {
    format!(r#"<script type="application/ld+json">{json}</script>"#)
}

fn hreflang(code: &str, href: &str) -> String {
    format!(r#"<link rel="alternate" hreflang="{code}" href="{href}">"#)
}

fn img(src: &str, alt: &str) -> String {
    format!(r#"<img src="{src}" alt="{alt}" width="40" height="30">"#)
}

/// The fixture site. `b` = internal origin, `e` = the same server under an external host name.
fn main_site(b: &str, e: &str, port: u16) -> Site {
    let mut s = Site::new();
    let mut linked: Vec<String> = Vec::new();
    let g = |path: &str, topic: &str| Doc::good(b, path, topic);
    let closed = closed_port();
    macro_rules! page {
        ($path:expr, $route:expr) => {{
            let p: String = $path.to_string();
            linked.push(p.clone());
            s.insert(p, $route);
        }};
    }
    macro_rules! unlinked {
        ($path:expr, $route:expr) => {
            s.insert($path.to_string(), $route)
        };
    }

    // ---- robots.txt and sitemaps
    unlinked!("/robots.txt", Route::new(200, "text/plain", format!(
        "User-agent: *\nDisallow: /blocked/\nDisallow: /private-img/\nCrawl-delay: 2\nNoindex: /old/\nthis line is bogus\n\n\
         Sitemap: /relative-sitemap.xml\nSitemap: {b}/sitemap_index.xml\nSitemap: {b}/sitemap-missing.xml\nSitemap: {b}/sitemap-bad.xml\nSitemap: {b}/sitemap-empty.xml\n")));
    unlinked!(
        "/sitemap_index.xml",
        Route::new(
            200,
            "application/xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><sitemap><loc>{b}/sitemap-pages.xml</loc></sitemap><sitemap><loc>{b}/sitemap-extra.xml.gz</loc></sitemap></sitemapindex>"#
            )
        )
    );
    let entries: Vec<(String, &str)> = vec![
        (format!("{b}/"), "2026-11-01"),
        (format!("{b}/perfect"), "2026-11-01T10:00:00+00:00"),
        (format!("{b}/orphan"), "2026-11"),
        (format!("{b}/dir/noindex"), "2026-11-01T10:00Z"),
        (format!("{b}/status/301"), ""),
        (format!("{b}/status/404"), ""),
        (format!("{b}/status/500"), ""),
        (format!("{b}/canon/to-other"), ""),
        (format!("{b}/blocked/page"), "23/11/2026"),
        (format!("{b}/links/nofollow-target"), ""),
        (format!("{b}/gone-from-sitemap"), ""),
        (format!("{e}/ext-ok"), ""),
    ];
    let urls: String = entries
        .iter()
        .map(|(u, lm)| {
            if lm.is_empty() {
                format!("<url><loc>{u}</loc></url>")
            } else {
                format!("<url><loc>{u}</loc><lastmod>{lm}</lastmod></url>")
            }
        })
        .collect();
    unlinked!(
        "/sitemap-pages.xml",
        Route::new(
            200,
            "application/xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>"#
            )
        )
    );
    unlinked!("/sitemap-extra.xml.gz", Route::new(200, "application/gzip", gz(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{b}/orphan-gz</loc></url></urlset>"#).as_bytes())));
    unlinked!(
        "/sitemap-bad.xml",
        Route::new(200, "application/xml", "<urlset><url><loc>broken")
    );
    unlinked!(
        "/sitemap-empty.xml",
        Route::new(
            200,
            "application/xml",
            r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"></urlset>"#
        )
    );
    unlinked!("/orphan", g("/orphan", "Orphan toad page").route());
    unlinked!(
        "/orphan-gz",
        g("/orphan-gz", "Compressed sitemap toad").route()
    );

    // ---- the perfect page
    let perfect_ld = format!(
        r#"[{{"@context":"https://schema.org","@type":"Article","headline":"Perfect pond toad care","image":"{b}/img/red-toad-shoe.png","datePublished":"2026-11-01","dateModified":"2026-11-20T10:00:00+05:30","author":{{"@type":"Person","name":"Ada Toad"}}}},
{{"@context":"https://schema.org","@type":"Organization","name":"Toad Shop","url":"{b}/","logo":"{b}/img/logo.png"}},
{{"@context":"https://schema.org","@type":"BreadcrumbList","itemListElement":[{{"@type":"ListItem","position":1,"name":"Home","item":"{b}/"}},{{"@type":"ListItem","position":2,"name":"Perfect","item":"{b}/perfect"}}]}},
{{"@context":"https://schema.org","@type":"WebSite","name":"Toad Shop","url":"{b}/"}}]"#
    );
    page!("/perfect", g("/perfect", "Perfect pond toad care")
        .head(r#"<script src="/js/app.js" defer></script>"#)
        .head(r#"<link rel="stylesheet" href="/css/main.css">"#)
        .head(ld(&perfect_ld))
        .body(format!(r#"<h2 id="section-two">Second section on pond care</h2><p><a name="legacy-anchor"></a>{}</p>"#, prose("perfect second", 60)))
        .body(img("/img/red-toad-shoe.png", "Red toad shoe on a lily pad"))
        .body(img("/img/photography-tips.jpg", "Toad photography tips"))
        .body(img("/2024/01/01/pond-at-dawn.png", "Pond at dawn"))
        .route());
    page!(
        "/redirect-target",
        g("/redirect-target", "Redirect landing page").route()
    );
    unlinked!("/ext-ok", g("/ext-ok", "Partner toad shop").route());

    // ---- static files
    let png = vec![0x89u8; 2000];
    for f in [
        "/img/red-toad-shoe.png",
        "/img/green-frog.png",
        "/img/lily.png",
        "/img/a1.png",
        "/img/a2.png",
        "/img/logo.png",
        "/2024/01/01/pond-at-dawn.png",
        "/private-img/secret.png",
    ] {
        unlinked!(f, file("image/png", png.clone()));
    }
    unlinked!("/img/photography-tips.jpg", file("image/jpeg", png.clone()));
    unlinked!("/img/IMG_1234.jpg", file("image/jpeg", png.clone()));
    unlinked!("/img/pond.png", Route::new(200, "image/png", png.clone()));
    unlinked!("/img/big.jpg", file("image/jpeg", vec![1u8; 150 * 1024]));
    unlinked!("/img/huge.png", file("image/png", vec![1u8; 600 * 1024]));
    unlinked!("/img/heavy.png", file("image/png", vec![1u8; 3200 * 1024]));
    unlinked!("/img/old.bmp", file("image/bmp", png.clone()));
    unlinked!("/img/old.png", redirect(301, "/img/red-toad-shoe.png"));
    unlinked!(
        "/img/not-image.png",
        html("<html><body>not an image</body></html>")
    );
    let js = "function hop(){return 1}\n".repeat(80);
    for f in ["/js/app.js", "/js/vendor.js", "/js/chunk.js"] {
        unlinked!(f, file("application/javascript", js.clone()).gzip(true));
    }
    unlinked!("/js/old.js", redirect(301, "/js/app.js"));
    unlinked!(
        "/js/big.js",
        file("application/javascript", "var x=1;\n".repeat(70_000))
    );
    unlinked!(
        "/css/main.css",
        file("text/css", "body{margin:0}\n".repeat(200)).gzip(true)
    );
    unlinked!(
        "/css/site.css",
        Route::new(200, "text/css", "p{color:red}\n")
            .header("Cache-Control", "max-age=60")
            .header("ETag", "\"s\"")
    );
    unlinked!(
        "/css/big.css",
        file("text/css", "a{color:blue}\n".repeat(20_000))
    );

    // ---- response codes (internal and external)
    for code in REDIRECT_CODES {
        page!(
            format!("/status/{code}"),
            redirect(code, "/redirect-target")
        );
        linked.push(format!("{e}/status/{code}"));
    }
    page!("/status/300", Route::new(300, "text/html", ""));
    linked.push(format!("{e}/status/300"));
    for code in STATUS_CODES {
        page!(format!("/status/{code}"), error(code));
        linked.push(format!("{e}/status/{code}"));
    }
    page!("/hang", g("/hang", "Hanging page").route().delay(5000));
    page!(
        "/drop",
        Route {
            drop: true,
            ..Default::default()
        }
    );
    for u in [
        format!("http://127.0.0.1:{closed}/gone"),
        format!("https://127.0.0.1:{port}/tls"),
        format!("{e}/hang"),
        format!("{e}/drop"),
        format!("http://localhost:{closed}/gone"),
        format!("https://localhost:{port}/tls"),
    ] {
        linked.push(u);
    }
    page!(
        "/blocked/page",
        g("/blocked/page", "Blocked toad page").route()
    );

    // ---- redirects
    page!("/r/chain1", redirect(301, "/r/chain2"));
    unlinked!("/r/chain2", redirect(301, "/r/chain3"));
    unlinked!("/r/chain3", redirect(301, "/redirect-target"));
    page!("/r/long1", redirect(301, "/r/long2"));
    for i in 2..7 {
        unlinked!(
            format!("/r/long{i}"),
            redirect(301, &format!("/r/long{}", i + 1))
        );
    }
    unlinked!("/r/long7", redirect(301, "/redirect-target"));
    page!("/r/loop-a", redirect(301, "/r/loop-b"));
    unlinked!("/r/loop-b", redirect(301, "/r/loop-a"));
    page!("/r/to-404", redirect(301, "/status/404"));
    page!("/r/to-500", redirect(301, "/status/500"));
    page!("/r/to-drop", redirect(301, "/drop"));
    page!("/r/to-noindex", redirect(301, "/dir/noindex"));
    page!("/r/external", redirect(301, &format!("{e}/ext-ok")));
    page!("/r/no-location", Route::new(302, "text/html", ""));
    page!(
        "/r/meta-refresh",
        g("/r/meta-refresh", "Meta refresh page")
            .head(r#"<meta http-equiv="refresh" content="0; url=/redirect-target">"#)
            .route()
    );
    page!(
        "/r/meta-reload",
        g("/r/meta-reload", "Self reloading page")
            .head(r#"<meta http-equiv="refresh" content="30">"#)
            .route()
    );
    page!(
        "/r/js",
        g("/r/js", "Script redirect page")
            .body(r#"<script>window.location.href = "/redirect-target";</script>"#)
            .route()
    );
    page!(
        "/r/js-bare",
        g("/r/js-bare", "Bare script redirect")
            .body(
                r#"<script>if (navigator.onLine) { location.href = "/redirect-target"; }</script>"#
            )
            .route()
    );
    page!("/r/js-compare", g("/r/js-compare", "Script location check").body(r#"<script>if (window.location.href == "/x" || foo.location === 1) { console.log(1); }</script>"#).route());

    // ---- security
    page!("/sec/forms", g("/sec/forms", "Account sign in").body(
        r#"<form action="/login" method="post"><label for="u">User</label><input id="u" name="u"><label for="pw">Password</label><input id="pw" type="password" name="pw"><button>Sign in</button></form>"#).route());
    let mut bare = Route::new(
        200,
        "text/html; charset=utf-8",
        g("/sec/no-headers", "Headerless toad page").html(),
    )
    .gzip(true);
    bare.headers.retain(|(k, _)| k == "Content-Type");
    page!("/sec/no-headers", bare);
    page!(
        "/sec/weak",
        g("/sec/weak", "Weak header toad page")
            .route()
            .set(
                "Content-Security-Policy",
                "default-src 'self' 'unsafe-inline' 'unsafe-eval'"
            )
            .set("Referrer-Policy", "unsafe-url")
            .header("Server", "Apache/2.4.1 (Unix)")
            .header("X-Powered-By", "PHP/8.3")
            .header("Access-Control-Allow-Origin", "*")
            .header("Set-Cookie", "session=abc; Path=/")
    );
    page!("/sec/links", g("/sec/links", "Partner link page")
        .body(format!(r#"<a href="{e}/ext-ok" target="_blank">Partner toad shop</a> <a href="//localhost:{port}/ext-ok">Partner pond shop</a>"#)).route());

    // ---- URLs
    for (path, topic) in [
        ("/url/UpperCase", "Uppercase address"),
        ("/url/under_score", "Underscore address"),
        ("/url/caf%C3%A9", "Accented address"),
        ("/url/with%20space", "Spaced address"),
        ("/url//double", "Double slash address"),
        ("/url/rep/rep/page", "Repeated folder address"),
        ("/url/params?a=1", "Parameter address"),
        ("/url/many?a=1&b=2&c=3&d=4&e=5", "Many parameter address"),
        ("/search?q=toads", "Site search results"),
        ("/url/session?sid=abc123", "Session address"),
        ("/url/track?utm_source=news", "Tracking address"),
        ("/a/b/c/d/e/f/deep-page", "Deep folder address"),
        ("/url/unsafe(1)", "Unsafe character address"),
        ("/url/index.html", "Index file address"),
        ("/url/double%2520enc", "Double encoded address"),
        ("/url/slash", "Slash variant one"),
        ("/url/slash/", "Slash variant two"),
        ("/url/Case", "Case variant one"),
        ("/url/case", "Case variant two"),
        ("/2024/01/01/dated-post", "Dated blog post"),
    ] {
        page!(path, g(path, topic).route());
    }
    let long = format!("/url/{}page", "long-segment-".repeat(8));
    page!(long.clone(), g(&long, "Very long address").route());

    // ---- page titles (each also links to /dir/noindex, giving it 10+ inlinks)
    let noindex_link = r#"<p><a href="/dir/noindex">Members only toad page</a></p>"#;
    for (path, doc) in [
        (
            "/t/missing",
            g("/t/missing", "Missing title").drop("<title"),
        ),
        ("/t/empty", g("/t/empty", "Empty title").title("")),
        (
            "/t/multiple",
            g("/t/multiple", "Multiple titles")
                .head("<title>Second title for the multiple title page</title>"),
        ),
        (
            "/t/dup-a",
            g("/t/dup-a", "Duplicate title A").title("Shared duplicate title for toad pages"),
        ),
        (
            "/t/dup-b",
            g("/t/dup-b", "Duplicate title B").title("Shared duplicate title for toad pages"),
        ),
        (
            "/t/long",
            g("/t/long", "Long title").title(
                "This is an extremely long page title that keeps going well past sixty characters",
            ),
        ),
        ("/t/short", g("/t/short", "Short title").title("Short")),
        (
            "/t/same-h1",
            g("/t/same-h1", "Same heading").title("Same heading explained"),
        ),
        (
            "/t/outside",
            g("/t/outside", "Outside title")
                .drop("<title")
                .body("<title>Title placed in the body by mistake today</title>"),
        ),
        (
            "/t/same-desc",
            g("/t/same-desc", "Same description")
                .desc("Same description - Toad Shop Fixture Guide"),
        ),
        ("/t/generic", g("/t/generic", "Generic title").title("Home")),
        (
            "/t/stuffed",
            g("/t/stuffed", "Stuffed title").title("Toads toads toads for sale cheap toads"),
        ),
        (
            "/t/caps",
            g("/t/caps", "Caps title").title("BUY THE BEST POND TOADS ONLINE TODAY"),
        ),
        (
            "/t/placeholder",
            g("/t/placeholder", "Placeholder title").title("{{ page.title }} - Toad Shop"),
        ),
        (
            "/t/domain",
            g("/t/domain", "Domain title").title("127.0.0.1"),
        ),
        (
            "/t/entities",
            g("/t/entities", "Entity title")
                .title("Fish &amp;amp; Chips for Toads and Frogs Guide"),
        ),
    ] {
        page!(path, doc.body(noindex_link).route());
    }

    // ---- meta descriptions and keywords
    for (path, doc) in [
        ("/d/missing", g("/d/missing", "Missing description").drop("<meta name=\"description\"")),
        ("/d/empty", g("/d/empty", "Empty description").desc("")),
        ("/d/multiple", g("/d/multiple", "Multiple descriptions").head(r#"<meta name="description" content="A second description that nobody asked for at all, really.">"#)),
        ("/d/dup-a", g("/d/dup-a", "Duplicate description A").desc("A shared description used by two different toad pages on this fixture site.")),
        ("/d/dup-b", g("/d/dup-b", "Duplicate description B").desc("A shared description used by two different toad pages on this fixture site.")),
        ("/d/long", g("/d/long", "Long description").desc(&"This description rambles on about toads and ponds and lilies for far too long. ".repeat(3))),
        ("/d/short", g("/d/short", "Short description").desc("Short description.")),
        ("/d/outside", g("/d/outside", "Outside description").drop("<meta name=\"description\"").body(r#"<meta name="description" content="A description placed in the body where search engines ignore it.">"#)),
        ("/d/placeholder", g("/d/placeholder", "Placeholder description").desc("Description goes here. Replace this text with a summary of the page before launch.")),
        ("/d/caps", g("/d/caps", "Caps description").desc("BUY THE BEST POND TOADS ONLINE TODAY AT LOW PRICES WITH FREE DELIVERY")),
        ("/d/stuffed", g("/d/stuffed", "Stuffed description").desc("Toads toads toads and more toads for sale, the best toads in town for you")),
        ("/d/entities", g("/d/entities", "Entity description").desc("Fish &amp;amp; chips for toads, frogs and newts, served fresh every day of the week.")),
        ("/d/html", g("/d/html", "Markup description").desc("Fresh toads every day &lt;br&gt; delivered to your pond with care and a smile.")),
        ("/k/a", g("/k/a", "Keywords page A").head(r#"<meta name="keywords" content="toads, frogs">"#).head(r#"<meta name="keywords" content="ponds">"#)),
        ("/k/b", g("/k/b", "Keywords page B").head(r#"<meta name="keywords" content="toads, frogs">"#)),
    ] {
        page!(path, doc.route());
    }

    // ---- headings
    let more = format!(r#"<p class="more">{}</p>"#, prose("more words", 120));
    for (path, doc) in [
        ("/h/no-h1", g("/h/no-h1", "No main heading").drop("<h1")),
        ("/h/empty-h1", g("/h/empty-h1", "Empty main heading").h1("")),
        ("/h/dup-a", g("/h/dup-a", "Duplicate heading A").h1("Shared duplicate heading").drop("<h2").body("<h2>Shared duplicate subheading</h2>")),
        ("/h/dup-b", g("/h/dup-b", "Duplicate heading B").h1("Shared duplicate heading").drop("<h2").body("<h2>Shared duplicate subheading</h2>")),
        ("/h/multi-h1", g("/h/multi-h1", "Two main headings").body("<h1>Another main heading on this page</h1>")),
        ("/h/long-h1", g("/h/long-h1", "Long heading").h1("A very long main heading that goes on and on about toads, frogs and newts")),
        ("/h/img-h1", g("/h/img-h1", "Image heading").drop("<h1").prepend(r#"<h1><img src="/img/logo.png" alt="Toad logo heading" width="10" height="10"></h1>"#)),
        ("/h/repeat-h1", g("/h/repeat-h1", "Repeated heading").body("<h1>Repeated heading explained</h1>")),
        ("/h/no-h2", g("/h/no-h2", "No subheadings").drop("<h2").body(more.clone())),
        ("/h/empty-h2", g("/h/empty-h2", "Empty subheading").body("<h2></h2>")),
        ("/h/long-h2", g("/h/long-h2", "Long subheading").drop("<h2").body("<h2>A very long subheading that goes on and on about toads, frogs and newts</h2>")),
        ("/h/h2-same", g("/h/h2-same", "Same subheading").drop("<h2").body("<h2>Same subheading explained</h2>")),
        ("/h/skip", g("/h/skip", "Skipped heading level").body("<h4>Skipped level heading</h4>")),
        ("/h/first-h2", g("/h/first-h2", "Subheading first").drop("<h1")),
        ("/h/none", g("/h/none", "No headings at all").drop("<h1").drop("<h2")),
    ] {
        page!(path, doc.route());
    }

    // ---- content
    let dup_body = format!(
        r#"<h1>Duplicate content page</h1><p>{}</p>"#,
        prose("duplicate content", 220)
    );
    let near = prose("near duplicate", 600);
    let hard = "Comprehensive organisational infrastructure modernisation initiatives necessitate considerable interdepartmental collaboration, \
        particularly regarding authentication, authorisation, documentation, configuration, virtualisation, containerisation and \
        international regulatory interoperability requirements, notwithstanding unavoidable institutional, organisational, \
        technological and administrative complications, considerations, dependencies, responsibilities, accountabilities, \
        sustainability evaluations, profitability calculations, accessibility specifications, compatibility certifications, \
        reproducibility verifications, interoperability negotiations, transformational methodologies, institutionalised \
        bureaucratic procedures, multidimensional categorisation, comprehensive standardisation, systematic optimisation, \
        individualised personalisation, revolutionary industrialisation, environmental rehabilitation, unconventional \
        experimentation, philosophical contemplation, psychological characterisation, governmental decentralisation, \
        constitutional reinterpretation and indispensable organisational synchronisation";
    // Flesch reading ease in the 30-50 band.
    let medium = "Most garden toads hide under stones during the warm part of the day, emerging after sunset to hunt the slugs, beetles and other small creatures that gardeners usually consider troublesome pests in vegetable beds.         A healthy pond with shallow edges, floating vegetation and some nearby cover will usually attract them within a season or two, provided that nobody uses chemical treatments near the water, because amphibians absorb substances through their permeable skin remarkably easily.         Leave a pile of logs or several flat stones in a shady corner so the animals can shelter during dry weather, and in early spring the adults will return to the same breeding location to deposit long strings of eggs among the submerged vegetation.";
    for (path, doc) in [
        (
            "/c/thin",
            g("/c/thin", "Thin content")
                .drop("<p class=\"prose\"")
                .body("<p>Tiny page.</p>"),
        ),
        (
            "/c/low",
            g("/c/low", "Low content")
                .drop("<p class=\"prose\"")
                .body(format!("<p>{}</p>", prose("low content", 100))),
        ),
        (
            "/c/dup-a",
            g("/c/dup-a", "Duplicate content A")
                .drop("<h1")
                .drop("<h2")
                .drop("<p class=\"prose\"")
                .prepend(dup_body.clone()),
        ),
        (
            "/c/dup-b",
            g("/c/dup-b", "Duplicate content B")
                .drop("<h1")
                .drop("<h2")
                .drop("<p class=\"prose\"")
                .prepend(dup_body.clone()),
        ),
        (
            "/c/near-a",
            g("/c/near-a", "Near duplicate A")
                .h1("Near duplicate page")
                .drop("<h2")
                .drop("<p class=\"prose\"")
                .body(format!("<p>{near} pond</p>")),
        ),
        (
            "/c/near-b",
            g("/c/near-b", "Near duplicate B")
                .h1("Near duplicate page")
                .drop("<h2")
                .drop("<p class=\"prose\"")
                .body(format!("<p>{near} lake</p>")),
        ),
        (
            "/c/lorem",
            g("/c/lorem", "Placeholder copy")
                .body("<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>"),
        ),
        (
            "/c/soft404",
            g("/c/soft404", "Missing page")
                .title("Page Not Found - Toad Shop")
                .h1("Sorry, this page does not exist"),
        ),
        (
            "/c/very-hard",
            g("/c/very-hard", "Dense reading")
                .drop("<p class=\"prose\"")
                .body(format!("<p>{hard} {hard}.</p>")),
        ),
        (
            "/c/hard",
            g("/c/hard", "Harder reading")
                .drop("<p class=\"prose\"")
                .body(format!("<p>{medium}</p>")),
        ),
        (
            "/c/bloated",
            g("/c/bloated", "Bloated markup").body(format!("<!-- {} -->", "x".repeat(40_000))),
        ),
        (
            "/c/coming-soon",
            g("/c/coming-soon", "New range").h1("Coming soon: the new toad range"),
        ),
    ] {
        page!(path, doc.route());
    }

    // ---- images
    let alt120 = "A very detailed description of a green frog sitting on a wet lily pad in the early morning light near the edge of a pond";
    page!(
        "/img/page",
        g("/img/page", "Toad picture gallery")
            .body(r#"<img src="/img/red-toad-shoe.png">"#)
            .body(img("/img/green-frog.png", alt120))
            .body(img("/img/missing.png", "Missing frog picture"))
            .body(img("/img/old.png", "Old frog picture"))
            .body(img("/private-img/secret.png", "Secret frog picture"))
            .body(img("/img/pond.png", "image"))
            .body(img("/img/lily.png", "IMG_1234.jpg"))
            .body(img("/img/a1.png", "Same frog alt text"))
            .body(img("/img/a2.png", "Same frog alt text"))
            .body(img("/img/big.jpg", "Big jpeg photo of a toad"))
            .body(img("/img/huge.png", "Huge png photo of a toad"))
            .body(img("/img/old.bmp", "Legacy bitmap toad"))
            .body(img("/img/not-image.png", "Wrong type toad"))
            .body(img("/img/IMG_1234.jpg", "Camera named toad"))
            .route()
    );
    page!(
        "/img/decorative",
        g("/img/decorative", "Decorative image")
            .body(img("/img/red-toad-shoe.png", ""))
            .route()
    );
    let mut lazy = g("/img/lazy", "Lazy gallery");
    for i in 0..12 {
        let attr = if i < 5 { "" } else { r#" loading="lazy""# };
        lazy = lazy.body(format!(
            r#"<img src="/img/red-toad-shoe.png" alt="Toad number {i}" width="4" height="4"{attr}>"#
        ));
    }
    page!("/img/lazy", lazy.route());

    // ---- canonicals
    for (path, doc) in [
        (
            "/canon/missing",
            g("/canon/missing", "No canonical").drop("<link rel=\"canonical\""),
        ),
        (
            "/canon/multiple-same",
            g("/canon/multiple-same", "Repeated canonical").head(format!(
                r#"<link rel="canonical" href="{b}/canon/multiple-same">"#
            )),
        ),
        (
            "/canon/conflict",
            g("/canon/conflict", "Conflicting canonicals")
                .head(format!(r#"<link rel="canonical" href="{b}/perfect">"#)),
        ),
        (
            "/canon/to-other",
            g("/canon/to-other", "Canonicalised page").canonical(&format!("{b}/perfect")),
        ),
        (
            "/canon/relative",
            g("/canon/relative", "Relative canonical").canonical("/canon/relative"),
        ),
        (
            "/canon/outside",
            g("/canon/outside", "Body canonical")
                .drop("<link rel=\"canonical\"")
                .body(format!(
                    r#"<link rel="canonical" href="{b}/canon/outside">"#
                )),
        ),
        (
            "/canon/other-domain",
            g("/canon/other-domain", "Cross domain canonical").canonical(&format!("{e}/ext-ok")),
        ),
        (
            "/canon/to-redirect",
            g("/canon/to-redirect", "Canonical to redirect").canonical(&format!("{b}/status/301")),
        ),
        (
            "/canon/to-404",
            g("/canon/to-404", "Canonical to missing").canonical(&format!("{b}/status/404")),
        ),
        (
            "/canon/to-500",
            g("/canon/to-500", "Canonical to error").canonical(&format!("{b}/status/500")),
        ),
        (
            "/canon/to-noindex",
            g("/canon/to-noindex", "Canonical to noindex").canonical(&format!("{b}/dir/noindex")),
        ),
        (
            "/canon/chain-a",
            g("/canon/chain-a", "Canonical chain start").canonical(&format!("{b}/canon/chain-b")),
        ),
        (
            "/canon/to-blocked",
            g("/canon/to-blocked", "Canonical to blocked").canonical(&format!("{b}/blocked/page")),
        ),
        (
            "/canon/invalid",
            g("/canon/invalid", "Invalid canonical").canonical("ftp://files.example/toad"),
        ),
        (
            "/canon/fragment",
            g("/canon/fragment", "Fragment canonical")
                .canonical(&format!("{b}/canon/fragment#top")),
        ),
        (
            "/canon/to-unlinked",
            g("/canon/to-unlinked", "Canonical to unlinked")
                .canonical(&format!("{b}/canon/unlinked-target")),
        ),
    ] {
        page!(path, doc.route());
    }
    unlinked!(
        "/canon/chain-b",
        g("/canon/chain-b", "Canonical chain middle")
            .canonical(&format!("{b}/perfect"))
            .route()
    );
    unlinked!(
        "/canon/unlinked-target",
        g("/canon/unlinked-target", "Unlinked canonical target").route()
    );
    page!(
        "/canon/header-conflict",
        g("/canon/header-conflict", "Header canonical")
            .route()
            .header("Link", &format!("<{b}/perfect>; rel=\"canonical\""))
    );

    // ---- pagination
    page!(
        "/page/1",
        g("/page/1", "Toad list page one")
            .head(r#"<link rel="next" href="/page/2">"#)
            .body(r#"<a href="/page/2">Next page of toads</a>"#)
            .route()
    );
    unlinked!(
        "/page/2",
        g("/page/2", "Toad list page two")
            .canonical(&format!("{b}/page/1"))
            .head(r#"<link rel="prev" href="/page/1">"#)
            .head(r#"<link rel="next" href="/page/3">"#)
            .body(r#"<a href="/page/1">Previous page of toads</a>"#)
            .route()
    );
    unlinked!("/page/3", g("/page/3", "Toad list page three")
        .head(r#"<link rel="prev" href="/page/2">"#).head(r#"<link rel="next" href="/status/404">"#).head(r#"<link rel="next" href="/page/4">"#)
        .body(r#"<a href="/status/404">Missing page of toads</a> <a href="/page/4">Last page of toads</a>"#).route());
    unlinked!("/page/4", g("/page/4", "Toad list page four").route());
    page!(
        "/page/loop",
        g("/page/loop", "Looping pagination")
            .head(r#"<link rel="next" href="/page/loop">"#)
            .route()
    );

    // ---- directives
    unlinked!(
        "/dir/noindex",
        g("/dir/noindex", "Members only").robots("noindex").route()
    );
    linked.push("/dir/noindex".into());
    for (path, doc) in [
        ("/dir/nofollow", g("/dir/nofollow", "Nofollow page").robots("nofollow").body(r#"<a href="/dir/nofollow-child">Child of nofollow page</a>"#)),
        ("/dir/none", g("/dir/none", "None directive").robots("none")),
        ("/dir/misc", g("/dir/misc", "Snippet controls").robots("noarchive, nosnippet, max-snippet:50, max-image-preview:standard, max-video-preview:10, noimageindex, notranslate, unavailable_after: 2030-01-01")),
        ("/dir/conflict", g("/dir/conflict", "Conflicting directives").robots("index, follow")),
        ("/dir/outside", g("/dir/outside", "Body robots meta").body(r#"<meta name="robots" content="index, follow">"#)),
        ("/dir/multi", g("/dir/multi", "Two robots metas").robots("index").robots("follow")),
        ("/dir/noindex-canon", g("/dir/noindex-canon", "Noindex canonical").robots("noindex").canonical(&format!("{b}/perfect"))),
    ] {
        let route = doc.route();
        let route = if path == "/dir/conflict" { route.header("X-Robots-Tag", "noindex") } else { route };
        page!(path, route);
    }
    page!(
        "/dir/x-robots",
        g("/dir/x-robots", "Header noindex")
            .route()
            .header("X-Robots-Tag", "noindex")
    );
    page!(
        "/dir/preview",
        g("/dir/preview", "No image previews")
            .body(r#"<a href="/dir/preview-child">Child of preview page</a>"#)
            .route()
            .header("X-Robots-Tag", "max-image-preview:none")
    );
    unlinked!(
        "/dir/nofollow-child",
        g("/dir/nofollow-child", "Nofollow child").route()
    );
    unlinked!(
        "/dir/preview-child",
        g("/dir/preview-child", "Preview child").route()
    );

    // ---- hreflang
    page!(
        "/hl/en",
        g("/hl/en", "English toads")
            .head(hreflang("en", &format!("{b}/hl/en")))
            .head(hreflang("de", &format!("{b}/hl/de")))
            .head(hreflang("x-default", &format!("{b}/hl/en")))
            .route()
    );
    page!(
        "/hl/de",
        g("/hl/de", "Deutsche Kroeten")
            .lang(Some("de"))
            .head(hreflang("de", &format!("{b}/hl/de")))
            .head(hreflang("en", &format!("{b}/hl/en")))
            .head(hreflang("x-default", &format!("{b}/hl/en")))
            .route()
    );
    page!(
        "/hl/bad",
        g("/hl/bad", "Crapauds mal annotes")
            .lang(Some("fr"))
            .head(hreflang("en", &format!("{b}/hl/bad")))
            .head(hreflang("en-UK", &format!("{b}/hl/en")))
            .head(hreflang("de", &format!("{b}/canon/to-other")))
            .head(hreflang("it", &format!("{b}/dir/noindex")))
            .head(hreflang("es", &format!("{b}/status/301")))
            .head(hreflang("pt", &format!("{b}/status/404")))
            .head(hreflang("nl", &format!("{b}/hl/x1")))
            .head(hreflang("nl", &format!("{b}/hl/x2")))
            .route()
    );
    page!(
        "/hl/noself",
        g("/hl/noself", "Hreflang without self")
            .canonical(&format!("{b}/perfect"))
            .head(hreflang("de", &format!("{b}/hl/de")))
            .body(hreflang("fr", &format!("{b}/hl/fr-x")))
            .route()
    );
    page!(
        "/hl/inc-a",
        g("/hl/inc-a", "Inconsistent English")
            .head(hreflang("en", &format!("{b}/hl/inc-a")))
            .head(hreflang("fr", &format!("{b}/hl/inc-b")))
            .head(hreflang("x-default", &format!("{b}/hl/inc-a")))
            .route()
    );
    unlinked!(
        "/hl/inc-b",
        g("/hl/inc-b", "Crapauds incoherents")
            .lang(Some("fr"))
            .head(hreflang("fr", &format!("{b}/hl/inc-b")))
            .head(hreflang("en-GB", &format!("{b}/hl/inc-a")))
            .head(hreflang("x-default", &format!("{b}/hl/inc-a")))
            .route()
    );

    // ---- structured data
    let types = [
        "Product",
        "Offer",
        "LocalBusiness",
        "BreadcrumbList",
        "ListItem",
        "FAQPage",
        "QAPage",
        "Question",
        "Answer",
        "Event",
        "Recipe",
        "JobPosting",
        "VideoObject",
        "Review",
        "AggregateRating",
        "Course",
        "SoftwareApplication",
        "Dataset",
        "Movie",
        "Article",
        "Organization",
        "WebSite",
    ];
    let mut empty_entities: Vec<String> = types
        .iter()
        .map(|t| format!(r#"{{"@context":"https://schema.org","@type":"{t}"}}"#))
        .collect();
    // Offer price rules apply to a Product's offers.
    empty_entities.push(r#"{"@context":"https://schema.org","@type":"Product","name":"Bare offer","offers":{"@type":"Offer"}}"#.to_string());
    let no_ld = "<script type=\"application/ld+json\"";
    for (path, doc) in [
        ("/sd/broken", g("/sd/broken", "Broken JSON-LD").head(ld(r#"{"@context":"https://schema.org","@type":"Thing",}"#))),
        ("/sd/no-context", g("/sd/no-context", "Context free markup").drop(no_ld).head(ld(&format!(r#"{{"@type":"Organization","name":"Toad Shop","url":"{b}/","logo":"{b}/img/logo.png"}}"#)))),
        ("/sd/no-type", g("/sd/no-type", "Typeless markup").drop(no_ld).head(ld(r#"{"@context":"https://schema.org","name":"No type here"}"#))),
        ("/sd/vocab", g("/sd/vocab", "Old vocabulary").drop(no_ld).head(ld(r#"{"@context":"http://data-vocabulary.org","@type":"Breadcrumb","title":"Home"}"#))),
        ("/sd/none", g("/sd/none", "No markup").drop(no_ld)),
        ("/sd/bad-date", g("/sd/bad-date", "Badly dated article").drop(no_ld).head(ld(&format!(
            r#"{{"@context":"https://schema.org","@type":"Article","headline":"Dated","image":"{b}/img/logo.png","datePublished":"23/11/2026","dateModified":"2026-11-23","author":{{"@type":"Person","name":"Ada"}}}}"#)))),
        ("/sd/howto", g("/sd/howto", "How to build a pond").drop(no_ld).head(ld(r#"{"@context":"https://schema.org","@type":"HowTo","name":"Build a pond","step":[{"@type":"HowToStep","text":"Dig"}]}"#))),
        ("/sd/faq", g("/sd/faq", "Toad questions").drop(no_ld).head(ld(r#"{"@context":"https://schema.org","@type":"FAQPage","mainEntity":[{"@type":"Question","name":"Do toads bite?","acceptedAnswer":{"@type":"Answer","text":"No."}}]}"#))),
        ("/sd/props", g("/sd/props", "Incomplete markup").drop(no_ld).head(ld(&format!("[{}]", empty_entities.join(","))))),
        ("/sd/product-ok", g("/sd/product-ok", "Complete product").drop(no_ld).head(ld(&format!(
            r#"{{"@context":"https://schema.org","@type":"Product","name":"Toad house","image":"{b}/img/logo.png","description":"A cosy toad house.","offers":{{"@type":"Offer","price":"19.99","priceCurrency":"EUR"}},"aggregateRating":{{"@type":"AggregateRating","ratingValue":"4.5","reviewCount":"12"}},"review":{{"@type":"Review","author":{{"@type":"Person","name":"Ada"}},"reviewRating":{{"@type":"Rating","ratingValue":"5"}}}}}}"#)))),
    ] {
        page!(path, doc.route());
    }

    // ---- social
    page!(
        "/social/none",
        g("/social/none", "No social tags")
            .drop("<meta property=\"og:")
            .drop("<meta name=\"twitter:")
            .route()
    );
    page!(
        "/social/bad",
        g("/social/bad", "Bad social tags")
            .drop("<meta property=\"og:image\"")
            .drop("<meta property=\"og:url\"")
            .drop("<meta property=\"og:description\"")
            .drop("<meta name=\"twitter:")
            .head(r#"<meta property="og:image" content="/img/share.png">"#)
            .head(format!(
                r#"<meta property="og:url" content="{b}/somewhere-else">"#
            ))
            .head(r#"<meta property="og:title" content="A second open graph title">"#)
            .head(r#"<meta property="og:description" content="">"#)
            .head(r#"<meta name="twitter:card" content="large">"#)
            .route()
    );

    // ---- links
    page!(
        "/links/bad",
        g("/links/bad", "Link problems")
            .body(format!(
                r##"<a href="/status/404">Broken toad page</a>
<a href="{e}/status/404">Broken partner page</a>
<a href="/status/301">Moved toad page</a>
<a href="/perfect" rel="nofollow">Nofollow guide</a>
<a href="/perfect"></a>
<a href="/perfect">click here</a>
<a href="javascript:void(0)">Open menu</a>
<a href=" /perfect ">Spaced guide link</a>
<a href="/perfect#nope">Missing section link</a>
<a href="/perfect" rel="sponsored">Sponsored guide</a>
<a href="/blocked/page">Blocked toad page</a>
<a href="/perfect"><img src="/img/red-toad-shoe.png" width="1" height="1"></a>
<a href="/links/nofollow-target" rel="nofollow">Hidden target</a>"##
            ))
            .route()
    );
    page!("/links/good-bookmark", g("/links/good-bookmark", "Working bookmark").body(r#"<a href="/perfect#section-two">Second section of the guide</a> <a href="/perfect#legacy-anchor">Legacy anchor in the guide</a>"#).route());
    let mut many = g("/links/many", "Link heavy page");
    for _ in 0..160 {
        many = many.body(r#"<a href="/">Toad Shop home page</a>"#);
    }
    for _ in 0..51 {
        many = many.body(format!(r#"<a href="{e}/ext-ok">Partner toad shop</a>"#));
    }
    page!("/links/many", many.route());
    page!(
        "/links/dead-end",
        g("/links/dead-end", "Dead end page").drop("<nav").route()
    );
    unlinked!(
        "/links/nofollow-target",
        g("/links/nofollow-target", "Nofollow target").route()
    );
    page!(
        "/deep/1",
        g("/deep/1", "Depth one")
            .body(r#"<a href="/deep/2">Depth two page</a>"#)
            .route()
    );
    for i in 2..=7 {
        unlinked!(
            format!("/deep/{i}"),
            g(&format!("/deep/{i}"), &format!("Depth {i}"))
                .body(format!(
                    r#"<a href="/deep/{}">Depth {} page</a>"#,
                    i + 1,
                    i + 1
                ))
                .route()
        );
    }

    // ---- resources
    page!(
        "/res/broken",
        g("/res/broken", "Broken resources")
            .head(r#"<script src="/js/missing.js" defer></script>"#)
            .head(r#"<link rel="stylesheet" href="/css/missing.css">"#)
            .head(r#"<script src="/js/old.js" defer></script>"#)
            .route()
    );
    page!(
        "/res/spa",
        g("/res/spa", "Single page app")
            .drop("<h2")
            .drop("<p class=\"prose\"")
            .body(r#"<div id="app"></div>"#)
            .head(r#"<script src="/js/app.js" defer></script>"#)
            .head(r#"<script src="/js/vendor.js" defer></script>"#)
            .head(r#"<script src="/js/chunk.js" defer></script>"#)
            .route()
    );
    page!("/res/inline", g("/res/inline", "Inline code").body(format!(
        "<script>{}document.write(\"hi\");</script><style>{}@font-face{{font-family:Toad;src:url(/toad.woff2)}}</style>",
        "var hop = 1;\n".repeat(4500), "p{margin:0}\n".repeat(4600))).route());

    // ---- performance
    let mut heavy = g("/perf/heavy", "Heavy page");
    for _ in 0..21 {
        heavy = heavy.head(r#"<script src="/js/app.js" defer></script>"#);
    }
    for _ in 0..11 {
        heavy = heavy.head(r#"<link rel="stylesheet" href="/css/site.css">"#);
    }
    for i in 0..80 {
        heavy = heavy.body(format!(
            r#"<img src="/img/red-toad-shoe.png" alt="Heavy toad {i}" width="4" height="4">"#
        ));
    }
    page!("/perf/heavy", heavy.route());
    page!(
        "/perf/slow1",
        g("/perf/slow1", "Slow page").route().delay(1100)
    );
    page!(
        "/perf/slow3",
        g("/perf/slow3", "Very slow page").route().delay(3100)
    );
    page!(
        "/perf/large",
        g("/perf/large", "Large page")
            .body(format!("<!-- {} -->", "x".repeat(1100 * 1024)))
            .route()
            .gzip(false)
    );
    page!(
        "/perf/uncompressed",
        g("/perf/uncompressed", "Uncompressed page")
            .route()
            .gzip(false)
    );
    page!(
        "/perf/blocking",
        g("/perf/blocking", "Blocking script")
            .head(r#"<script src="/js/app.js"></script>"#)
            .route()
    );
    page!(
        "/perf/dom",
        g("/perf/dom", "Huge DOM")
            .body("<i>t</i>".repeat(1600))
            .route()
    );
    page!(
        "/perf/iframe",
        g("/perf/iframe", "Sizeless frame")
            .body(r#"<iframe src="/perfect" title="Guide"></iframe>"#)
            .route()
    );
    page!(
        "/perf/weight",
        g("/perf/weight", "Heavy assets")
            .body(img("/img/heavy.png", "Heavy toad poster"))
            .head(r#"<script src="/js/big.js" defer></script>"#)
            .head(r#"<link rel="stylesheet" href="/css/big.css">"#)
            .route()
    );

    // ---- mobile
    page!(
        "/m/no-viewport",
        g("/m/no-viewport", "No viewport")
            .drop("<meta name=\"viewport\"")
            .route()
    );
    page!(
        "/m/fixed",
        g("/m/fixed", "Fixed viewport")
            .drop("<meta name=\"viewport\"")
            .head(r#"<meta name="viewport" content="width=1024">"#)
            .route()
    );
    page!("/m/zoom", g("/m/zoom", "No zoom").drop("<meta name=\"viewport\"").head(r#"<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">"#).route());
    page!(
        "/m/plugins",
        g("/m/plugins", "Plugin content")
            .body(r#"<object data="/toad.swf" type="application/x-shockwave-flash"></object>"#)
            .route()
    );
    page!(
        "/m/frames",
        html(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>Framed toad site navigation page</title></head><frameset cols="50%,50%"><frame src="/perfect"><frame src="/redirect-target"></frameset></html>"#
        )
    );

    // ---- accessibility
    let mut a11y = g("/a11y/page", "Accessibility problems").lang(Some("english")).body(
        r#"<a href="/perfect"></a><button></button><input type="text" name="q">
<iframe src="/perfect" width="10" height="10"></iframe><span id="dup">a</span><span id="dup">b</span>
<a href="/perfect" tabindex="3">Tabbed guide link</a><video src="/v.mp4" autoplay></video>
<table><tr><td>1</td></tr></table>"#);
    a11y.main = false;
    page!("/a11y/page", a11y.route());
    page!(
        "/a11y/no-lang",
        g("/a11y/no-lang", "Language free").lang(None).route()
    );
    let mut hidden = g("/a11y/hidden", "Hidden body");
    hidden.body_attrs = r#" aria-hidden="true""#.into();
    page!("/a11y/hidden", hidden.route());

    // ---- validation
    let plain = |t: &str| {
        format!(
            r#"<p>{}</p><nav><a href="/">Toad Shop home page</a></nav>"#,
            prose(t, 220)
        )
    };
    let mut no_doctype = g("/v/no-doctype", "Quirks mode");
    no_doctype.doctype = false;
    page!("/v/no-doctype", no_doctype.route());
    page!(
        "/v/no-head",
        html(format!(
            r#"<!DOCTYPE html><html lang="en"><meta charset="utf-8"><title>Headless toad page for validation</title><body><h1>No head</h1>{}</body></html>"#,
            plain("no head")
        ))
    );
    page!(
        "/v/multi-head",
        html(format!(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>Two heads toad page for validation</title></head><head><meta name="author" content="Ada"></head><body><h1>Two heads</h1>{}</body></html>"#,
            plain("two heads")
        ))
    );
    page!(
        "/v/multi-body",
        html(format!(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>Two bodies toad page for validation</title></head><body><h1>Two bodies</h1>{}</body><body><p>Again</p></body></html>"#,
            plain("two bodies")
        ))
    );
    page!(
        "/v/div-in-head",
        g("/v/div-in-head", "Div in head")
            .head("<div>oops</div>")
            .route()
    );
    page!(
        "/v/no-charset",
        g("/v/no-charset", "No charset")
            .drop("<meta charset")
            .route()
            .set("Content-Type", "text/html")
    );
    page!(
        "/v/latin1",
        g("/v/latin1", "Latin charset")
            .drop("<meta charset")
            .head(r#"<meta charset="iso-8859-1">"#)
            .route()
    );
    page!(
        "/v/deprecated",
        g("/v/deprecated", "Old markup")
            .body("<center>Old</center><font>Old font</font>")
            .route()
    );
    page!(
        "/v/no-ct",
        g("/v/no-ct", "No content type")
            .route()
            .set("Content-Type", "")
    );
    page!(
        "/v/base",
        g("/v/base", "Base element")
            .head(format!(r#"<base href="{b}/">"#))
            .route()
    );
    page!(
        "/v/content-lang",
        g("/v/content-lang", "Language header")
            .route()
            .header("Content-Language", "de")
    );

    // ---- AMP
    page!(
        "/amp/article",
        g("/amp/article", "AMP article")
            .head(format!(
                r#"<link rel="amphtml" href="{b}/amp/article-amp">"#
            ))
            .route()
    );
    let mut amp =
        g("/amp/article-amp", "AMP article mobile").canonical(&format!("{b}/amp/article"));
    amp.html_attrs = r#" amp lang="en""#.into();
    unlinked!("/amp/article-amp", amp.route());
    page!(
        "/amp/broken-link",
        g("/amp/broken-link", "AMP broken link")
            .head(format!(r#"<link rel="amphtml" href="{b}/status/404">"#))
            .route()
    );
    let mut amp_nc =
        g("/amp/no-canonical", "AMP without canonical").drop("<link rel=\"canonical\"");
    amp_nc.html_attrs = r#" amp lang="en""#.into();
    page!("/amp/no-canonical", amp_nc.route());
    page!(
        "/amp/plain-target",
        g("/amp/plain-target", "AMP to plain")
            .head(format!(r#"<link rel="amphtml" href="{b}/amp/plain">"#))
            .route()
    );
    unlinked!("/amp/plain", g("/amp/plain", "Plain AMP target").route());

    // ---- AI link opportunity: mentions the perfect page's H1 without linking to it
    page!("/ai/mention", g("/ai/mention", "Mention page").drop("<nav").body(r#"<p>Read about perfect pond toad care explained by our staff.</p><a href="/">Toad Shop home page</a>"#).route());

    // ---- home: links to everything, no favicon or touch icon
    let anchors: String = linked
        .iter()
        .map(|u| format!("<li><a href=\"{u}\">Visit {u}</a></li>\n"))
        .collect();
    s.insert(
        "/".into(),
        g("/", "Toad Shop home")
            .drop("<link rel=\"icon\"")
            .drop("<link rel=\"apple-touch-icon\"")
            .body(format!("<ul>{anchors}</ul>"))
            .route(),
    );
    s
}

/// A tiny site for robots.txt / sitemap variants, with one good start page.
fn variant_site(b: &str, robots: Option<Route>, extra: Vec<(&str, Route)>) -> Site {
    let mut s = Site::new();
    s.insert("/".into(), Doc::good(b, "/", "Variant home page").route());
    if let Some(r) = robots {
        s.insert("/robots.txt".into(), r);
    }
    for (p, r) in extra {
        s.insert(p.into(), r);
    }
    s
}

fn run_variant(build: impl FnOnce(&str) -> Site, crawl_sitemaps: bool) -> (Server, Audited) {
    let srv = Server::start(|b, _, _| build(b));
    let cfg = shouting_toad::crawler::Config {
        crawl_sitemaps,
        ..config()
    };
    let a = Audited::new(crawl(&[srv.url("/")], false, cfg), &srv.base);
    (srv, a)
}

#[test]
fn fixture_site_audit() {
    let srv = Server::start(main_site);
    let a = Audited::new(crawl(&[srv.url("/")], false, config()), &srv.base);
    let e = srv.ext.clone();
    let closed_gone = a
        .crawl
        .pages
        .iter()
        .find(|p| p.url.ends_with("/gone") && p.url.contains("127.0.0.1"))
        .map(|p| p.url.clone())
        .unwrap_or_default();

    // ---- the perfect page: only checks explained by the fixture being plain HTTP/1.1
    let http_only = ["security.http_url", "performance.no_http2"];
    a.only("/perfect", &http_only);
    a.hit("security.http_url", "/perfect");
    a.hit("performance.no_http2", "/perfect");
    a.expect(
        a.page("/perfect").content_encoding == "gzip"
            && a.page("/perfect").transfer_size < a.page("/perfect").size,
        "/perfect should be fetched gzipped and decompressed",
    );
    a.expect(
        a.page("/perfect").word_count() > 250,
        "gzipped /perfect should be parsed after decompression",
    );
    // Site-level checks only attach to the start page; the http:// and www probes must not invent findings.
    for id in [
        "security.http_not_redirected",
        "url.www_not_consolidated",
        "links.local_or_private",
        "robots.missing",
        "robots.no_sitemap",
        "sitemaps.none",
        "sitemaps.not_in_robots",
    ] {
        a.expect(
            a.hits(id).is_empty(),
            &format!("{id} should not fire anywhere (fires on {:?})", a.hits(id)),
        );
    }

    // ---- response codes
    for code in REDIRECT_CODES.iter().chain(STATUS_CODES.iter()).copied() {
        let id = match code {
            418 => "other_4xx".to_string(),
            599 => "other_5xx".to_string(),
            c => c.to_string(),
        };
        a.hit(
            &format!("response.internal_{id}"),
            &format!("/status/{code}"),
        );
        a.hit(
            &format!("response.external_{id}"),
            &format!("{e}/status/{code}"),
        );
        a.no_hit(
            &format!("response.internal_{id}"),
            &format!("{e}/status/{code}"),
        );
    }
    a.hit("response.internal_other_3xx", "/status/300");
    a.hit("response.external_other_3xx", &format!("{e}/status/300"));
    a.hit("response.internal_no_response_timeout", "/hang");
    a.hit("response.internal_no_response_other", "/drop");
    a.hit("response.internal_no_response_connect", &closed_gone);
    a.hit(
        "response.internal_no_response_tls",
        &format!("https://127.0.0.1:{}/tls", srv.port),
    );
    a.hit(
        "response.external_no_response_timeout",
        &format!("{e}/hang"),
    );
    a.hit("response.external_no_response_other", &format!("{e}/drop"));
    a.hit(
        "response.external_no_response_tls",
        &format!("https://localhost:{}/tls", srv.port),
    );
    a.expect(
        a.hits("response.external_no_response_connect")
            .iter()
            .any(|u| u.ends_with("/gone")),
        "external connect failure",
    );
    a.asserted
        .borrow_mut()
        .insert("response.external_no_response_connect".into());
    a.hit("response.internal_blocked_robots", "/blocked/page");
    a.hit(
        "response.internal_blocked_resource",
        "/private-img/secret.png",
    );
    a.no_hit("response.internal_404", "/sitemap-missing.xml");
    a.hit("response.internal_404", "/gone-from-sitemap"); // only listed in a sitemap
    a.hit("sitemaps.client_error", "/gone-from-sitemap");
    a.expect(
        a.page("/sitemap-missing.xml").status == 404,
        "declared sitemap that 404s is recorded",
    );

    // ---- redirects
    a.hit("redirects.chain", "/r/chain1");
    a.hit("redirects.chain", "/r/chain2");
    a.no_hit("redirects.chain", "/r/chain3");
    a.no_hit("redirects.chain", "/status/301");
    a.hit("redirects.chain_too_long", "/r/long1");
    a.no_hit("redirects.chain_too_long", "/r/chain1");
    a.hit("redirects.loop", "/r/loop-a");
    a.hit("redirects.loop", "/r/loop-b");
    a.no_hit("redirects.chain", "/r/loop-a");
    a.hit("redirects.to_4xx", "/r/to-404");
    a.hit("redirects.to_5xx", "/r/to-500");
    a.hit("redirects.to_no_response", "/r/to-drop");
    a.hit("redirects.to_non_indexable", "/r/to-noindex");
    a.hit("redirects.external", "/r/external");
    a.no_hit("redirects.external", "/status/301");
    a.hit("redirects.missing_location", "/r/no-location");
    a.hit("redirects.meta_refresh", "/r/meta-refresh");
    a.hit("redirects.meta_refresh_reload", "/r/meta-reload");
    a.no_hit("redirects.meta_refresh", "/r/meta-reload");
    a.hit("redirects.javascript", "/r/js");
    a.hit("redirects.javascript", "/r/js-bare");
    a.no_hit("redirects.javascript", "/r/js-compare");

    // ---- security
    a.hit("security.http_url", "/");
    a.hit("security.form_http_action", "/sec/forms");
    a.hit("security.password_on_http", "/sec/forms");
    for id in [
        "security.missing_csp",
        "security.missing_nosniff",
        "security.missing_frame_protection",
        "security.missing_referrer_policy",
        "security.missing_permissions_policy",
        "security.missing_coop",
    ] {
        a.hit(id, "/sec/no-headers");
        a.no_hit(id, "/sec/weak");
    }
    for id in [
        "security.csp_unsafe_inline",
        "security.csp_unsafe_eval",
        "security.unsafe_referrer_policy",
        "security.server_version",
        "security.powered_by",
        "security.cookie_not_httponly",
        "security.cookie_no_samesite",
        "security.cors_wildcard",
    ] {
        a.hit(id, "/sec/weak");
    }
    a.no_hit("security.cookie_not_secure", "/sec/weak"); // only meaningful on HTTPS pages
    a.hit("security.unsafe_cross_origin_links", "/sec/links");
    a.hit("security.protocol_relative_links", "/sec/links");

    // ---- URLs
    for (id, path) in [
        ("url.uppercase", "/url/UpperCase"),
        ("url.underscores", "/url/under_score"),
        ("url.non_ascii", "/url/caf%C3%A9"),
        ("url.spaces", "/url/with%20space"),
        ("url.multiple_slashes", "/url//double"),
        ("url.repetitive_path", "/url/rep/rep/page"),
        ("url.parameters", "/url/params?a=1"),
        ("url.many_parameters", "/url/many?a=1&b=2&c=3&d=4&e=5"),
        ("url.internal_search", "/search?q=toads"),
        ("url.session_id", "/url/session?sid=abc123"),
        ("url.tracking_parameters", "/url/track?utm_source=news"),
        ("url.deep_folders", "/a/b/c/d/e/f/deep-page"),
        ("url.unsafe_characters", "/url/unsafe(1)"),
        ("url.index_file", "/url/index.html"),
        ("url.double_encoded", "/url/double%2520enc"),
        ("url.trailing_slash_duplicate", "/url/slash"),
        ("url.trailing_slash_duplicate", "/url/slash/"),
        ("url.case_duplicate", "/url/Case"),
        ("url.case_duplicate", "/url/case"),
    ] {
        a.hit(id, path);
    }
    a.hit(
        "url.over_115",
        &format!("/url/{}page", "long-segment-".repeat(8)),
    );
    a.no_hit("url.many_parameters", "/url/params?a=1");
    a.no_hit("url.repetitive_path", "/2024/01/01/dated-post"); // date folders are not a crawl trap
    a.no_hit("url.uppercase", "/url/case");

    // ---- page titles
    for (id, path) in [
        ("titles.missing", "/t/missing"),
        ("titles.empty", "/t/empty"),
        ("titles.multiple", "/t/multiple"),
        ("titles.duplicate", "/t/dup-a"),
        ("titles.duplicate", "/t/dup-b"),
        ("titles.over_60", "/t/long"),
        ("titles.over_580px", "/t/long"),
        ("titles.below_30", "/t/short"),
        ("titles.below_200px", "/t/short"),
        ("titles.same_as_h1", "/t/same-h1"),
        ("titles.outside_head", "/t/outside"),
        ("titles.same_as_description", "/t/same-desc"),
        ("titles.generic", "/t/generic"),
        ("titles.keyword_stuffing", "/t/stuffed"),
        ("titles.all_caps", "/t/caps"),
        ("titles.placeholder", "/t/placeholder"),
        ("titles.domain_only", "/t/domain"),
        ("titles.double_encoded", "/t/entities"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("titles.empty", "/t/missing");
    a.no_hit("titles.missing", "/t/empty");
    a.no_hit("titles.below_30", "/t/empty");

    // ---- meta description and keywords
    for (id, path) in [
        ("description.missing", "/d/missing"),
        ("description.empty", "/d/empty"),
        ("description.multiple", "/d/multiple"),
        ("description.duplicate", "/d/dup-a"),
        ("description.duplicate", "/d/dup-b"),
        ("description.over_155", "/d/long"),
        ("description.over_990px", "/d/long"),
        ("description.below_70", "/d/short"),
        ("description.below_400px", "/d/short"),
        ("description.outside_head", "/d/outside"),
        ("description.placeholder", "/d/placeholder"),
        ("description.all_caps", "/d/caps"),
        ("description.keyword_stuffing", "/d/stuffed"),
        ("description.double_encoded", "/d/entities"),
        ("description.contains_html", "/d/html"),
        ("keywords.present", "/k/a"),
        ("keywords.present", "/k/b"),
        ("keywords.multiple", "/k/a"),
        ("keywords.duplicate", "/k/a"),
        ("keywords.duplicate", "/k/b"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("keywords.multiple", "/k/b");
    a.no_hit("description.missing", "/dir/noindex"); // not indexable, so not an opportunity

    // ---- headings
    for (id, path) in [
        ("h1.missing", "/h/no-h1"),
        ("h1.empty", "/h/empty-h1"),
        ("h1.duplicate", "/h/dup-a"),
        ("h1.duplicate", "/h/dup-b"),
        ("h1.multiple", "/h/multi-h1"),
        ("h1.over_70", "/h/long-h1"),
        ("h1.image_only", "/h/img-h1"),
        ("h1.repeated_on_page", "/h/repeat-h1"),
        ("h2.missing", "/h/no-h2"),
        ("h2.empty", "/h/empty-h2"),
        ("h2.duplicate", "/h/dup-a"),
        ("h2.over_70", "/h/long-h2"),
        ("h2.same_as_h1", "/h/h2-same"),
        ("headings.non_sequential", "/h/skip"),
        ("headings.first_not_h1", "/h/first-h2"),
        ("headings.empty", "/h/empty-h2"),
        ("headings.none", "/h/none"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("h1.missing", "/h/img-h1");
    a.no_hit("h1.repeated_on_page", "/h/multi-h1");
    a.no_hit("headings.first_not_h1", "/h/empty-h1");

    // ---- content
    for (id, path) in [
        ("content.thin", "/c/thin"),
        ("content.low", "/c/low"),
        ("content.exact_duplicate", "/c/dup-a"),
        ("content.exact_duplicate", "/c/dup-b"),
        ("content.near_duplicate", "/c/near-a"),
        ("content.near_duplicate", "/c/near-b"),
        ("content.lorem_ipsum", "/c/lorem"),
        ("content.soft_404", "/c/soft404"),
        ("content.readability_very_difficult", "/c/very-hard"),
        ("content.readability_difficult", "/c/hard"),
        ("content.low_text_ratio", "/c/bloated"),
        ("content.under_construction", "/c/coming-soon"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("content.near_duplicate", "/c/dup-a");
    a.no_hit("content.exact_duplicate", "/c/near-a");
    a.no_hit("content.soft_404", "/status/404");

    // ---- images
    for (id, path) in [
        ("images.missing_alt_attribute", "/img/page"),
        ("images.alt_over_100", "/img/page"),
        ("images.missing_dimensions", "/img/page"),
        ("images.broken", "/img/page"),
        ("images.redirected", "/img/page"),
        ("images.blocked", "/img/page"),
        ("images.generic_alt", "/img/page"),
        ("images.filename_alt", "/img/page"),
        ("images.duplicate_alt", "/img/page"),
        ("images.empty_alt", "/img/decorative"),
        ("images.not_lazy", "/perf/heavy"),
        ("images.over_100kb", "/img/big.jpg"),
        ("images.over_500kb", "/img/huge.png"),
        ("images.next_gen", "/img/big.jpg"),
        ("images.next_gen", "/img/huge.png"),
        ("images.legacy_format", "/img/old.bmp"),
        ("images.wrong_content_type", "/img/not-image.png"),
        ("images.non_descriptive_filename", "/img/IMG_1234.jpg"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("images.missing_alt_attribute", "/img/decorative");
    a.no_hit("images.not_lazy", "/img/lazy");
    a.no_hit("images.over_100kb", "/img/huge.png");
    a.no_hit(
        "images.non_descriptive_filename",
        "/img/photography-tips.jpg",
    );
    a.no_hit("images.non_descriptive_filename", "/img/red-toad-shoe.png");

    // ---- canonicals
    for (id, path) in [
        ("canonicals.missing", "/canon/missing"),
        ("canonicals.multiple", "/canon/multiple-same"),
        ("canonicals.conflicting", "/canon/conflict"),
        ("canonicals.canonicalised", "/canon/to-other"),
        ("canonicals.relative", "/canon/relative"),
        ("canonicals.outside_head", "/canon/outside"),
        ("canonicals.other_domain", "/canon/other-domain"),
        ("canonicals.to_redirect", "/canon/to-redirect"),
        ("canonicals.to_4xx", "/canon/to-404"),
        ("canonicals.to_5xx", "/canon/to-500"),
        ("canonicals.to_noindex", "/canon/to-noindex"),
        ("canonicals.chain", "/canon/chain-a"),
        ("canonicals.blocked", "/canon/to-blocked"),
        ("canonicals.invalid", "/canon/invalid"),
        ("canonicals.fragment", "/canon/fragment"),
        ("canonicals.header_conflict", "/canon/header-conflict"),
        ("canonicals.unlinked", "/canon/to-unlinked"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("canonicals.canonicalised", "/canon/relative");
    a.no_hit("canonicals.canonicalised", "/canon/fragment");
    a.no_hit("canonicals.multiple", "/canon/conflict");
    a.no_hit("canonicals.missing", "/dir/noindex");
    a.no_hit("canonicals.unlinked", "/canon/to-other");

    // ---- pagination
    a.hit("pagination.non_indexable", "/page/1");
    a.hit("pagination.canonical_to_first", "/page/2");
    a.hit("pagination.unlinked", "/page/2");
    a.hit("pagination.multiple", "/page/3");
    a.hit("pagination.non_200", "/page/3");
    a.hit("pagination.loop", "/page/loop");
    for id in [
        "pagination.unlinked",
        "pagination.multiple",
        "pagination.loop",
        "pagination.non_200",
        "pagination.canonical_to_first",
    ] {
        a.no_hit(id, "/page/1");
    }

    // ---- directives
    for (id, path) in [
        ("directives.noindex", "/dir/noindex"),
        ("directives.nofollow", "/dir/nofollow"),
        ("directives.none", "/dir/none"),
        ("directives.noarchive", "/dir/misc"),
        ("directives.nosnippet", "/dir/misc"),
        ("directives.max_snippet", "/dir/misc"),
        ("directives.max_image_preview", "/dir/misc"),
        ("directives.max_video_preview", "/dir/misc"),
        ("directives.noimageindex", "/dir/misc"),
        ("directives.notranslate", "/dir/misc"),
        ("directives.unavailable_after", "/dir/misc"),
        ("directives.x_robots_noindex", "/dir/x-robots"),
        ("directives.noindex", "/dir/x-robots"),
        ("directives.conflicting", "/dir/conflict"),
        ("directives.outside_head", "/dir/outside"),
        ("directives.multiple_meta", "/dir/multi"),
        ("directives.noindex_with_canonical", "/dir/noindex-canon"),
        ("directives.noindex_linked_heavily", "/dir/noindex"),
        ("directives.max_image_preview", "/dir/preview"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("directives.noindex", "/dir/misc");
    a.no_hit("directives.conflicting", "/dir/multi");
    // max-image-preview:none is a preview limit, not "none" (= noindex, nofollow)
    for id in [
        "directives.noindex",
        "directives.none",
        "directives.nofollow",
        "directives.x_robots_noindex",
    ] {
        a.no_hit(id, "/dir/preview");
    }
    a.expect(
        srv.requested("/dir/preview-child"),
        "links on a max-image-preview:none page must be followed",
    );
    a.expect(
        !srv.requested("/dir/nofollow-child"),
        "links on a meta nofollow page must not be followed",
    );

    // ---- robots.txt (site level, on the start page)
    for id in [
        "robots.crawl_delay",
        "robots.invalid_lines",
        "robots.noindex_rule",
        "robots.relative_sitemap",
    ] {
        a.hit(id, "/");
        a.no_hit(id, "/perfect");
    }
    a.expect(
        !srv.requested("/blocked/page"),
        "robots.txt Disallow: /blocked/ must prevent fetching /blocked/page",
    );
    a.expect(
        !srv.requested("/private-img/secret.png"),
        "robots.txt must block disallowed images too",
    );
    let robots = a.crawl.site.robots.clone().unwrap_or_default();
    a.expect(
        robots.status == 200
            && robots.crawl_delay == Some(2.0)
            && robots.invalid_lines == 1
            && robots.noindex_lines == 1,
        &format!("robots info: {robots:?}"),
    );

    // ---- sitemaps
    for id in [
        "sitemaps.error",
        "sitemaps.invalid_xml",
        "sitemaps.invalid_lastmod",
        "sitemaps.other_host",
        "sitemaps.empty",
    ] {
        a.hit(id, "/");
    }
    for (id, path) in [
        ("sitemaps.redirect", "/status/301"),
        ("sitemaps.client_error", "/status/404"),
        ("sitemaps.server_error", "/status/500"),
        ("sitemaps.noindex", "/dir/noindex"),
        ("sitemaps.canonicalised", "/canon/to-other"),
        ("sitemaps.blocked", "/blocked/page"),
        ("sitemaps.orphan", "/orphan"),
        ("sitemaps.orphan", "/orphan-gz"),
        ("sitemaps.missing_indexable", "/t/short"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("sitemaps.missing_indexable", "/perfect");
    a.no_hit("sitemaps.orphan", "/perfect");
    let pages_sm = a
        .crawl
        .site
        .sitemaps
        .iter()
        .find(|s| s.url.ends_with("/sitemap-pages.xml"));
    a.expect(
        pages_sm
            .is_some_and(|s| s.invalid_lastmod == 1 && s.url_count == 12 && s.other_host_urls == 1),
        &format!("sitemap-pages info: {pages_sm:?}"),
    );
    a.expect(
        a.crawl.site.sitemap_urls.contains(&srv.url("/orphan-gz")),
        "gzipped child sitemap is decompressed and read",
    );
    a.expect(
        a.page("/orphan").source == Some(shouting_toad::model::Source::Sitemap),
        "orphan is found through the sitemap",
    );

    // ---- hreflang
    for (id, path) in [
        ("hreflang.missing_return", "/hl/bad"),
        ("hreflang.inconsistent_return", "/hl/inc-a"),
        ("hreflang.non_canonical", "/hl/bad"),
        ("hreflang.noindex", "/hl/bad"),
        ("hreflang.to_redirect", "/hl/bad"),
        ("hreflang.to_error", "/hl/bad"),
        ("hreflang.invalid_code", "/hl/bad"),
        ("hreflang.duplicate_code", "/hl/bad"),
        ("hreflang.missing_self", "/hl/noself"),
        ("hreflang.missing_x_default", "/hl/bad"),
        ("hreflang.outside_head", "/hl/noself"),
        ("hreflang.on_non_canonical", "/hl/noself"),
        ("hreflang.lang_mismatch", "/hl/bad"),
    ] {
        a.hit(id, path);
    }
    for path in ["/hl/en", "/hl/de"] {
        a.only(
            path,
            &[
                "security.http_url",
                "performance.no_http2",
                "links.single_inlink",
                "sitemaps.missing_indexable",
            ],
        );
    }
    a.no_hit("hreflang.inconsistent_return", "/hl/inc-b");

    // ---- structured data
    for (id, path) in [
        ("sd.parse_error", "/sd/broken"),
        ("sd.missing_context", "/sd/no-context"),
        ("sd.missing_type", "/sd/no-type"),
        ("sd.non_schema_context", "/sd/vocab"),
        ("sd.data_vocabulary", "/sd/vocab"),
        ("sd.none", "/sd/none"),
        ("sd.invalid_date", "/sd/bad-date"),
        ("sd.howto_retired", "/sd/howto"),
        ("sd.faq_limited", "/sd/faq"),
    ] {
        a.hit(id, path);
    }
    let sd_props: Vec<String> = catalogue()
        .iter()
        .filter(|c| c.category == "Structured Data" && c.name.ends_with(" Missing"))
        .map(|c| c.id.clone())
        .collect();
    for id in &sd_props {
        a.hit(id, "/sd/props");
        a.no_hit(id, "/perfect");
        a.no_hit(id, "/sd/faq");
        if id.starts_with("sd.product")
            || id.starts_with("sd.offer")
            || id.starts_with("sd.aggregaterating")
            || id.starts_with("sd.review")
        {
            a.no_hit(id, "/sd/product-ok");
        }
    }
    a.no_hit("sd.none", "/sd/broken");
    a.no_hit("sd.invalid_date", "/perfect");

    // ---- social
    for key in [
        "og_title",
        "og_description",
        "og_image",
        "og_url",
        "og_type",
        "og_site_name",
    ] {
        a.hit(&format!("social.missing_{key}"), "/social/none");
    }
    for (id, path) in [
        ("social.missing_twitter_card", "/social/none"),
        ("social.og_image_relative", "/social/bad"),
        ("social.og_url_mismatch", "/social/bad"),
        ("social.og_multiple_titles", "/social/bad"),
        ("social.empty_tags", "/social/bad"),
        ("social.invalid_twitter_card", "/social/bad"),
    ] {
        a.hit(id, path);
    }

    // ---- links
    for (id, path) in [
        ("links.internal_broken", "/links/bad"),
        ("links.internal_broken", "/"),
        ("links.external_broken", "/links/bad"),
        ("links.internal_redirects", "/links/bad"),
        ("links.internal_nofollow", "/links/bad"),
        ("links.no_anchor_text", "/links/bad"),
        ("links.non_descriptive_anchor", "/links/bad"),
        ("links.javascript_href", "/links/bad"),
        ("links.whitespace_href", "/links/bad"),
        ("links.broken_bookmark", "/links/bad"),
        ("links.internal_sponsored_ugc", "/links/bad"),
        ("links.to_blocked", "/links/bad"),
        ("links.image_link_no_alt", "/links/bad"),
        ("links.too_many", "/links/many"),
        ("links.many_external", "/links/many"),
        ("links.no_internal_outlinks", "/links/dead-end"),
        ("links.no_inlinks", "/orphan"),
        ("links.no_inlinks", "/orphan-gz"),
        ("links.no_inlinks", "/canon/unlinked-target"),
        ("links.single_inlink", "/t/short"),
        ("links.only_nofollow_inlinks", "/links/nofollow-target"),
        ("links.depth_4_5", "/deep/4"),
        ("links.depth_4_5", "/deep/5"),
        ("links.depth_over_5", "/deep/6"),
        ("links.depth_over_5", "/deep/7"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("links.broken_bookmark", "/links/good-bookmark");
    a.no_hit("links.depth_4_5", "/deep/3");
    a.no_hit("links.no_inlinks", "/");
    a.no_hit("links.single_inlink", "/perfect");
    a.expect(
        a.page("/deep/6").depth == 6,
        "depth counts clicks from the start page",
    );

    // ---- resources
    for (id, path) in [
        ("resources.broken_js", "/res/broken"),
        ("resources.broken_css", "/res/broken"),
        ("resources.redirected", "/res/broken"),
        ("resources.blocked", "/img/page"),
        ("resources.many_scripts", "/perf/heavy"),
        ("resources.many_stylesheets", "/perf/heavy"),
        ("resources.large_inline_js", "/res/inline"),
        ("resources.large_inline_css", "/res/inline"),
        ("resources.document_write", "/res/inline"),
        ("resources.js_rendered", "/res/spa"),
        ("ai.js_only_content", "/res/spa"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("resources.js_rendered", "/res/inline");

    // ---- performance
    for (id, path) in [
        ("performance.slow_1s", "/perf/slow1"),
        ("performance.slow_3s", "/perf/slow3"),
        ("performance.large_html", "/perf/large"),
        ("performance.uncompressed", "/perf/uncompressed"),
        ("performance.uncompressed", "/js/big.js"),
        ("performance.no_cache_headers", "/img/pond.png"),
        ("performance.short_cache", "/css/site.css"),
        ("performance.no_validators", "/img/pond.png"),
        ("performance.render_blocking_js", "/perf/blocking"),
        ("performance.many_render_blocking_css", "/perf/heavy"),
        ("performance.dom_size", "/perf/dom"),
        ("performance.many_requests", "/perf/heavy"),
        ("performance.iframe_no_size", "/perf/iframe"),
        ("performance.font_display", "/res/inline"),
        ("performance.page_weight", "/perf/weight"),
        ("performance.large_js", "/js/big.js"),
        ("performance.large_css", "/css/big.css"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("performance.slow_1s", "/perf/slow3");
    a.no_hit("performance.uncompressed", "/js/app.js");
    a.no_hit("performance.no_cache_headers", "/img/red-toad-shoe.png");
    a.no_hit("performance.render_blocking_js", "/res/spa");

    // ---- mobile, accessibility, validation, AMP
    for (id, path) in [
        ("mobile.missing_viewport", "/m/no-viewport"),
        ("mobile.viewport_not_device_width", "/m/fixed"),
        ("mobile.zoom_disabled", "/m/zoom"),
        ("mobile.plugins", "/m/plugins"),
        ("mobile.frames", "/m/frames"),
        ("mobile.no_touch_icon", "/"),
        ("mobile.no_favicon", "/"),
        ("a11y.missing_lang", "/a11y/no-lang"),
        ("a11y.invalid_lang", "/a11y/page"),
        ("a11y.links_without_text", "/a11y/page"),
        ("a11y.buttons_without_name", "/a11y/page"),
        ("a11y.inputs_without_label", "/a11y/page"),
        ("a11y.iframe_title", "/a11y/page"),
        ("a11y.duplicate_ids", "/a11y/page"),
        ("a11y.positive_tabindex", "/a11y/page"),
        ("a11y.missing_main", "/a11y/page"),
        ("a11y.autoplay", "/a11y/page"),
        ("a11y.video_captions", "/a11y/page"),
        ("a11y.table_headers", "/a11y/page"),
        ("a11y.aria_hidden_body", "/a11y/hidden"),
        ("validation.missing_doctype", "/v/no-doctype"),
        ("validation.missing_head", "/v/no-head"),
        ("validation.multiple_head", "/v/multi-head"),
        ("validation.multiple_body", "/v/multi-body"),
        ("validation.invalid_in_head", "/v/div-in-head"),
        ("validation.missing_charset", "/v/no-charset"),
        ("validation.non_utf8", "/v/latin1"),
        ("validation.charset_mismatch", "/v/latin1"),
        ("validation.deprecated_tags", "/v/deprecated"),
        ("validation.missing_content_type", "/v/no-ct"),
        ("validation.base_href", "/v/base"),
        ("validation.content_language_mismatch", "/v/content-lang"),
        ("amp.link_not_200", "/amp/broken-link"),
        ("amp.missing_canonical", "/amp/no-canonical"),
        ("amp.target_not_amp", "/amp/plain-target"),
    ] {
        a.hit(id, path);
    }
    a.no_hit("mobile.no_favicon", "/perfect");
    a.no_hit("a11y.buttons_without_name", "/sec/forms");
    a.no_hit("a11y.inputs_without_label", "/sec/forms");
    a.no_hit("validation.missing_charset", "/v/latin1");
    for id in [
        "amp.link_not_200",
        "amp.missing_canonical",
        "amp.target_not_amp",
    ] {
        a.no_hit(id, "/amp/article");
        a.no_hit(id, "/amp/article-amp");
    }

    // ---- offline AI helpers
    let opps = shouting_toad::ai::link_opportunities(&a.crawl);
    a.expect(
        opps.iter()
            .any(|o| o.source_url == srv.url("/ai/mention") && o.target_url == srv.url("/perfect")),
        "link opportunity from /ai/mention to /perfect",
    );
    let brand = shouting_toad::ai::site_name(&a.crawl);
    a.expect(
        brand == "Toad Shop",
        &format!("brand inferred as {brand:?}"),
    );
    for (i, _) in shouting_toad::ai::candidates(&a.crawl) {
        let p = &a.crawl.pages[i];
        let (t, d) = shouting_toad::ai::offline_suggestion(p, &brand);
        a.expect(
            !t.is_empty() && t.chars().count() <= 60,
            &format!("offline title for {}: {t:?}", p.url),
        );
        a.expect(
            !d.is_empty() && d.chars().count() <= 156 && d == d.trim(),
            &format!("offline description for {}: {d:?}", p.url),
        );
    }

    // ---- robots.txt / sitemap variants (site-level checks on small sites)
    let (_, v1) = run_variant(
        |b| {
            variant_site(
                b,
                None,
                vec![(
                    "/sitemap.xml",
                    Route::new(
                        200,
                        "application/xml",
                        format!(
                            r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{b}/</loc></url></urlset>"#
                        ),
                    ),
                )],
            )
        },
        true,
    );
    v1.hit("robots.missing", "/");
    v1.hit("sitemaps.not_in_robots", "/");
    v1.no_hit("sitemaps.none", "/");
    let (_, v2) = run_variant(|b| variant_site(b, Some(error(500)), vec![]), true);
    v2.hit("robots.server_error", "/");
    v2.hit("sitemaps.none", "/");
    let (_, v3) = run_variant(
        |b| {
            variant_site(
                b,
                Some(Route {
                    drop: true,
                    ..Default::default()
                }),
                vec![],
            )
        },
        true,
    );
    v3.hit("robots.unreachable", "/");
    let padding = "# keep crawlers polite\n".repeat(25_000);
    let (v4_srv, v4) = run_variant(
        |b| {
            variant_site(
                b,
                Some(redirect(301, "/robots-real.txt")),
                vec![(
                    "/robots-real.txt",
                    Route::new(
                        200,
                        "text/plain",
                        format!("User-agent: *\nDisallow: /\n{padding}"),
                    ),
                )],
            )
        },
        true,
    );
    for id in [
        "robots.redirected",
        "robots.disallow_all",
        "robots.too_large",
        "robots.no_sitemap",
        "response.internal_blocked_robots",
    ] {
        v4.hit(id, "/");
    }
    v4.expect(
        !v4_srv.requested("/"),
        "a site-wide Disallow must stop the start page being fetched",
    );
    let big: String = (0..50_001)
        .map(|i| format!("<url><loc>{{b}}/p{i}</loc></url>"))
        .collect();
    let (_, v5) = run_variant(
        |b| {
            variant_site(
                b,
                Some(Route::new(
                    200,
                    "text/plain",
                    format!("User-agent: *\nAllow: /\nSitemap: {b}/big.xml\n"),
                )),
                vec![(
                    "/big.xml",
                    Route::new(
                        200,
                        "application/xml",
                        format!(
                            r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{}</urlset>"#,
                            big.replace("{b}", b)
                        ),
                    ),
                )],
            )
        },
        false,
    );
    v5.hit("sitemaps.over_50k", "/");
    v5.no_hit("sitemaps.not_in_robots", "/");

    // ---- coverage report
    let variants = [&v1, &v2, &v3, &v4, &v5];
    let mut asserted: BTreeSet<String> = a.asserted.borrow().clone();
    let mut triggered = a.triggered();
    for v in variants {
        asserted.extend(v.asserted.borrow().iter().cloned());
        triggered.extend(v.triggered());
    }
    let cat = catalogue();
    let never: Vec<&str> = cat
        .iter()
        .map(|c| c.id.as_str())
        .filter(|id| !triggered.contains(*id))
        .collect();
    eprintln!(
        "\n{} checks in catalogue; {} triggered by the fixtures; {} positively asserted.",
        cat.len(),
        triggered.len(),
        asserted.len()
    );
    eprintln!(
        "Never triggered by the fixtures ({}):\n  {}",
        never.len(),
        never.join("\n  ")
    );
    for c in CATEGORIES {
        let n = cat
            .iter()
            .filter(|x| x.category == *c && asserted.contains(&x.id))
            .count();
        a.expect(n > 0, &format!("category {c} has no asserted check"));
    }
    a.expect(
        asserted.len() >= 150,
        &format!("only {} distinct check ids asserted", asserted.len()),
    );

    for v in variants {
        a.failures
            .borrow_mut()
            .extend(v.failures.borrow().iter().cloned());
    }
    a.finish();
}
