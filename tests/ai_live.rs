//! Live AI provider test. Skipped by default; run with a local OpenAI-compatible server:
//!   SHOUTINGTOAD_LIVE_AI_URL=http://localhost:11434/v1 SHOUTINGTOAD_LIVE_AI_MODEL=qwen3.5:4b \
//!   cargo test --test ai_live -- --ignored --nocapture

use shouting_toad::ai::{self, AiConfig, Provider};
use shouting_toad::model::Page;
use shouting_toad::parse::parse_html;

#[test]
#[ignore = "needs SHOUTINGTOAD_LIVE_AI_URL and SHOUTINGTOAD_LIVE_AI_MODEL"]
fn openai_compatible_writes_title_and_description() {
    let (Ok(base), Ok(model)) = (std::env::var("SHOUTINGTOAD_LIVE_AI_URL"), std::env::var("SHOUTINGTOAD_LIVE_AI_MODEL")) else {
        return;
    };
    let url = url::Url::parse("https://example.com/hiking-boots").unwrap();
    let html = "<html lang=en><head><title>Boots</title></head><body><h1>Waterproof hiking boots for wide feet</h1>\
        <p>Our leather hiking boots are fully waterproof, come in wide fittings and are resoled for free for five years. \
        Each pair is handmade in Portugal and weighs under one kilogram.</p></body></html>";
    let page = Page { url: url.to_string(), status: 200, is_internal: true, html: Some(Box::new(parse_html(html, &url))), ..Default::default() };
    let cfg = AiConfig { provider: Provider::OpenAiCompatible, model, base_url: base, api_key: String::new() };
    let s = ai::suggest(&cfg, &page, "Title under 30 chars", "Example Boots");
    eprintln!("{s:#?}");
    assert!(s.error.is_none(), "provider error: {:?}", s.error);
    assert!(s.source.starts_with("AI"));
    assert!(!s.title.is_empty() && !s.description.is_empty());
}

/// Prints offline suggestions and link opportunities for a saved crawl:
///   SHOUTINGTOAD_CRAWL_FILE=path/to/crawl.shoutingtoad cargo test --test ai_live saved_crawl -- --ignored --nocapture
#[test]
#[ignore = "needs SHOUTINGTOAD_CRAWL_FILE"]
fn saved_crawl_suggestions_and_links() {
    let Ok(path) = std::env::var("SHOUTINGTOAD_CRAWL_FILE") else { return };
    let crawl = shouting_toad::model::Crawl::load(std::path::Path::new(&path)).expect("load crawl");
    let brand = ai::site_name(&crawl);
    eprintln!("site name: {brand}");
    let offline = AiConfig { provider: Provider::Offline, ..Default::default() };
    for (i, reason) in ai::candidates(&crawl).into_iter().take(8) {
        let s = ai::suggest(&offline, &crawl.pages[i], &reason, &brand);
        eprintln!("\n{}\n  why:   {}\n  title: {} ({})\n  desc:  {} ({})", s.url, s.reason, s.title, s.title.chars().count(), s.description, s.description.chars().count());
        assert!(s.title.chars().count() <= 70 && !s.description.is_empty());
    }
    let opps = ai::link_opportunities(&crawl);
    eprintln!("\n{} link opportunities", opps.len());
    for o in opps.iter().take(12) {
        eprintln!("  {} --[{}]--> {}\n     {}", o.source_url, o.phrase, o.target_url, o.context);
    }
}
