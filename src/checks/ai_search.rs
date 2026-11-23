//! "AI Search" checks: AI crawler access (from the post-crawl access test), content that needs
//! JavaScript, bot-specific noindex and section quotability.

use super::*;
use crate::ai_search::{noindex_for, AiBotReport, Bot, BotResult, Outcome, Purpose};

/// Access-test rows for this page (none when it was not one of the tested pages).
fn rows<'a>(p: &Page, x: &'a Ctx) -> Vec<(&'static Bot, &'a BotResult)> {
    let Some(r) = &x.crawl.site.ai_bots else {
        return Vec::new();
    };
    match r.page_index(&p.url) {
        Some(i) => r.rows_for(i).collect(),
        None => Vec::new(),
    }
}

/// Training-bot rows of the start page (the first tested page).
fn training_rows<'a>(p: &Page, x: &'a Ctx) -> Vec<(&'static Bot, &'a BotResult)> {
    let first = x.crawl.site.ai_bots.as_ref().and_then(|r| r.pages.first());
    if first.is_none_or(|f| f.url != p.url) {
        return Vec::new();
    }
    rows(p, x)
        .into_iter()
        .filter(|(b, _)| b.purpose == Purpose::Training)
        .collect()
}

/// Meta tag or X-Robots-Tag naming an AI crawler carries noindex / none.
fn bot_noindex(p: &Page) -> bool {
    let meta = p.html.as_deref().is_some_and(|h| {
        h.ai.bot_meta
            .iter()
            .any(|(_, content)| noindex_for(content, None))
    });
    meta || p.headers_all("x-robots-tag").any(|v| {
        v.split_once(':').is_some_and(|(k, _)| {
            crate::ai_search::bot(k).is_some() && noindex_for(v, Some(k.trim()))
        })
    })
}

pub(super) fn ai_search(v: &mut Vec<Check>) {
    const CAT: &str = "AI Search";
    add!(v, "ai.search_bot_firewall_blocked", CAT, Issue, High, "AI Search Crawler Blocked by Firewall or CDN",
        "robots.txt allows an AI search crawler (such as OAI-SearchBot, Claude-SearchBot or PerplexityBot), but the server or CDN refused it: a 4xx/5xx status, a bot challenge or a captcha, while a normal request got the page. The crawler cannot read the page, so it cannot show or cite it in AI search answers, even though robots.txt says it may. This usually comes from a bot-protection setting (for example a \"block AI bots\" switch or a WAF rule) rather than a deliberate choice. Tested with the crawler's published user agent; the real crawler also comes from its vendor's IP ranges, which some firewalls verify, so confirm in your CDN or firewall logs.",
        "In the CDN or firewall settings, allow (or verify by IP and allow) the AI search crawlers you want to appear in. If you do not want them, disallow them in robots.txt instead so the policy is explicit.",
        |p, x| rows(p, x).iter().any(|(b, r)| b.purpose == Purpose::Search && r.robots_allowed && r.outcome.refused()));
    add!(v, "ai.search_bot_robots_blocked", CAT, Issue, Medium, "AI Search Crawler Disallowed in Robots.txt",
        "robots.txt disallows this page for an AI search or answer crawler (such as OAI-SearchBot, Claude-SearchBot, PerplexityBot or Applebot). These crawlers build the indexes AI search engines answer from, so the page will not be cited or linked there. Search crawlers are separate from training crawlers: you can block training and still allow search.",
        "If you want to appear in AI search answers, remove the Disallow for these user agents (or add an Allow group for them). Keep training crawlers such as GPTBot and ClaudeBot blocked separately if that is your policy.",
        |p, x| rows(p, x).iter().any(|(b, r)| b.purpose == Purpose::Search && !r.robots_allowed));
    add!(v, "ai.user_fetcher_blocked", CAT, Warning, Medium, "AI User-Triggered Fetcher Blocked",
        "A fetcher that an AI assistant sends when a person asks about this page (ChatGPT-User, Claude-User, Perplexity-User and similar) was refused by the server or CDN, or is disallowed in robots.txt by a vendor that obeys it. People who paste your link into an assistant will get an answer without your content.",
        "Allow these user agents in your CDN or firewall, and in robots.txt unless you intend to keep assistants away from the page.",
        |p, x| rows(p, x).iter().any(|(b, r)| b.purpose == Purpose::User && AiBotReport::blocked(b, r)));
    add!(v, "ai.training_bot_firewall_only", CAT, Warning, Low, "AI Training Crawler Blocked by Firewall but Allowed in Robots.txt",
        "An AI training crawler (such as GPTBot, ClaudeBot or CCBot) is refused by the server or CDN while robots.txt allows it. Blocking training crawlers is a valid choice, but the two signals disagree: crawlers that respect robots.txt keep trying and getting errors, and anyone reading your robots.txt sees a policy you do not apply.",
        "Decide on a policy. To block training, also disallow these user agents in robots.txt. To allow it, relax the CDN or firewall rule.",
        |p, x| rows(p, x).iter().any(|(b, r)| b.purpose == Purpose::Training && r.robots_allowed && r.outcome.refused()));
    add!(v, "ai.different_content", CAT, Warning, Medium, "AI Crawler Gets Much Less Content",
        "With an AI crawler's user agent the page answered 200 but with less than half the words of a normal request. The crawler indexes or quotes the thinner version. This can be bot-specific rendering, a consent or paywall layer shown only to bots, or a misconfigured cache.",
        "Serve AI crawlers the same main content as visitors (or decide deliberately what they may see), then test again.",
        |p, x| rows(p, x).iter().any(|(_, r)| r.outcome == Outcome::LessContent));
    add!(v, "ai.bot_noindex", CAT, Warning, Medium, "Noindex for a Specific AI Crawler",
        "The page has a robots meta tag or X-Robots-Tag header that names an AI crawler (for example <meta name=\"GPTBot\" content=\"noindex\"> or \"X-Robots-Tag: PerplexityBot: noindex\"), or that crawler was served a noindex a normal request did not get. Crawlers that honour it will leave the page out of their index. Not every AI vendor documents support for bot-specific meta tags.",
        "Remove the bot-specific noindex if you want the page in that AI product; keep it if exclusion is intended.",
        |p, x| (ih(p) && bot_noindex(p)) || rows(p, x).iter().any(|(_, r)| r.noindex));
    add!(v, "ai.js_only_content", CAT, Warning, Medium, "Content Invisible to AI Crawlers Without JavaScript",
        "The raw HTML has fewer than 50 words but loads heavy scripts, so the content probably appears only after JavaScript runs. Most AI crawlers (GPTBot, ClaudeBot, PerplexityBot and others) do not run JavaScript, so they see an almost empty page.",
        "Server-render or pre-render the main content so it is in the HTML response.",
        |p, _| ihd(p).is_some_and(js_rendered));
    add!(v, "ai.low_quotability", CAT, Opportunity, Low, "Few Self-Contained Sections",
        "Fewer than half of this page's main-content sections (3 or more, each a heading and at least 40 words) stand alone. AI answers quote passages, and a passage is easiest to quote when its first sentence answers a question heading directly and is not over 40 words, it does not open with \"This\" or \"It\", it is at most 300 words long, and it contains concrete facts such as numbers or names. The weakest section is shown in the AI Search view.",
        "Rewrite weak sections: put a short, direct answer that names the topic in the first sentence after each heading, replace opening pronouns with the noun, add specifics, and split very long sections.",
        |p, _| ihd(p).is_some_and(|h| h.ai.sections >= 3 && (h.ai.quotable as usize) * 2 < h.ai.sections as usize));
    add!(v, "ai.training_all_blocked", CAT, Opportunity, Low, "All AI Training Crawlers Blocked",
        "Every AI training crawler tested (GPTBot, ClaudeBot, Google-Extended, Applebot-Extended, CCBot, Meta-ExternalAgent, Bytespider, Amazonbot) is disallowed or refused on the start page. This is a policy choice, not an error: it keeps your content out of future model training. It does not by itself remove you from AI search answers, which use separate search crawlers.",
        "No action needed if this is intended. Check that AI search crawlers are still allowed if you want to be cited in AI answers.",
        |p, x| { let r = training_rows(p, x); !r.is_empty() && r.iter().all(|(b, r)| AiBotReport::blocked(b, r)) });
}
