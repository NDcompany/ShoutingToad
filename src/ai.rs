//! Title / meta description writer (bring-your-own-key AI or offline rules)
//! and the internal linking opportunity finder.

use crate::audit::{ihd, indexable};
use crate::model::{Crawl, Page};
use aho_corasick::{AhoCorasick, MatchKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Offline,
    OpenAiCompatible,
    Anthropic,
    Gemini,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Offline => "Offline rules (no AI)",
            Provider::OpenAiCompatible => "OpenAI-compatible (OpenAI, OpenRouter, Ollama, LM Studio…)",
            Provider::Anthropic => "Anthropic Claude",
            Provider::Gemini => "Google Gemini",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Offline => "",
            Provider::OpenAiCompatible => "gpt-4.1-mini",
            Provider::Anthropic => "claude-opus-5-5",
            Provider::Gemini => "gemini-2.5-flash",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub provider: Provider,
    pub model: String,
    pub base_url: String,
    /// Never written to disk.
    #[serde(skip)]
    pub api_key: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        let mut c = AiConfig {
            provider: Provider::Offline,
            model: String::new(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
        };
        c.load_env_key();
        c
    }
}

impl AiConfig {
    /// Pick up a key from the environment for the selected provider (or choose one).
    pub fn load_env_key(&mut self) {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let found = match self.provider {
            Provider::Anthropic => env("ANTHROPIC_API_KEY").map(|k| (Provider::Anthropic, k)),
            Provider::OpenAiCompatible => env("OPENAI_API_KEY").map(|k| (Provider::OpenAiCompatible, k)),
            Provider::Gemini => env("GEMINI_API_KEY").map(|k| (Provider::Gemini, k)),
            Provider::Offline => env("ANTHROPIC_API_KEY")
                .map(|k| (Provider::Anthropic, k))
                .or_else(|| env("OPENAI_API_KEY").map(|k| (Provider::OpenAiCompatible, k)))
                .or_else(|| env("GEMINI_API_KEY").map(|k| (Provider::Gemini, k))),
        };
        if let Some((p, k)) = found {
            if self.provider == Provider::Offline {
                self.provider = p;
            }
            self.api_key = k;
        }
        if self.model.trim().is_empty() {
            self.model = self.provider.default_model().to_string();
        }
    }

    pub fn ready(&self) -> bool {
        match self.provider {
            Provider::Offline => true,
            // Local OpenAI-compatible servers (Ollama, LM Studio) need no key.
            Provider::OpenAiCompatible => !self.api_key.trim().is_empty() || !self.base_url.contains("api.openai.com"),
            _ => !self.api_key.trim().is_empty(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Suggestion {
    pub url: String,
    pub reason: String,
    pub current_title: String,
    pub title: String,
    pub current_description: String,
    pub description: String,
    pub source: String,
    pub error: Option<String>,
}

/// Indexable pages whose title or description needs work, with the reason.
pub fn candidates(crawl: &Crawl) -> Vec<(usize, String)> {
    crawl
        .pages
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let h = ihd(p)?;
            let t = h.titles.first().map(|s| s.chars().count()).unwrap_or(0);
            let d = h.meta_descriptions.first().map(|s| s.chars().count()).unwrap_or(0);
            let mut why = Vec::new();
            match t {
                0 => why.push("Missing title"),
                n if n > 60 => why.push("Title over 60 chars"),
                n if n < 30 => why.push("Title under 30 chars"),
                _ => {}
            }
            match d {
                0 => why.push("Missing description"),
                n if n > 155 => why.push("Description over 155 chars"),
                n if n < 70 => why.push("Description under 70 chars"),
                _ => {}
            }
            (!why.is_empty()).then(|| (i, why.join(", ")))
        })
        .collect()
}

const SEPARATORS: &[&str] = &[" | ", " - ", " – ", " — ", " :: ", " · ", " • "];

/// The site's brand name, inferred from title suffixes, og:site_name or the host.
pub fn site_name(crawl: &Crawl) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in &crawl.pages {
        let Some(h) = ihd(p) else { continue };
        if let Some(t) = h.titles.first() {
            for sep in SEPARATORS {
                if let Some((_, suffix)) = t.rsplit_once(sep) {
                    let s = suffix.trim();
                    if !s.is_empty() && s.chars().count() <= 40 {
                        *counts.entry(s.to_string()).or_insert(0) += 1;
                    }
                    break;
                }
            }
        }
        if let Some((_, v)) = h.og.iter().find(|(k, _)| k == "og:site_name") {
            *counts.entry(v.trim().to_string()).or_insert(0) += 2;
        }
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .max_by_key(|(_, c)| *c)
        .map(|(s, _)| s)
        .unwrap_or_else(|| {
            url::Url::parse(&crawl.start_url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
                .unwrap_or_default()
        })
}

fn strip_brand(title: &str, brand: &str) -> String {
    for sep in SEPARATORS {
        if let Some(rest) = title.strip_suffix(&format!("{sep}{brand}")) {
            return rest.trim().to_string();
        }
    }
    title.trim().to_string()
}

fn cut_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for w in s.split_whitespace() {
        if out.chars().count() + w.chars().count() + 1 > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    // Prefer ending at a natural break (comma, colon, dash) if it keeps most of the text.
    if let Some(pos) = out.rfind([',', ':', ';', '–', '—', '|']) {
        if pos >= max * 3 / 5 {
            out.truncate(pos);
        }
    }
    let mut words: Vec<&str> = out.split_whitespace().collect();
    const DANGLING: &[&str] = &["and", "or", "for", "of", "the", "a", "an", "to", "in", "with", "on", "at", "by", "from", "&", "small", "large", "best", "your", "our"];
    while words.len() > 2 && words.last().is_some_and(|w| DANGLING.contains(&w.to_lowercase().as_str())) {
        words.pop();
    }
    words.join(" ").trim_end_matches([',', ';', ':', '-', '–', '|']).to_string()
}

fn topic(p: &Page, brand: &str) -> String {
    let h = p.html.as_deref();
    let h1 = h.and_then(|h| h.h1.first()).filter(|s| !s.is_empty());
    let title = h.and_then(|h| h.titles.first()).map(|t| strip_brand(t, brand)).filter(|s| !s.is_empty());
    h1.cloned().or(title).unwrap_or_else(|| {
        let seg = p.url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("");
        let words = seg.replace(['-', '_'], " ");
        let mut c = words.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => brand.to_string(),
        }
    })
}

/// Deterministic rewrite used when no AI provider is configured.
pub fn offline_suggestion(p: &Page, brand: &str) -> (String, String) {
    let h = p.html.as_deref();
    let topic = topic(p, brand);
    let cur_title = h.and_then(|h| h.titles.first()).cloned().unwrap_or_default();
    let title_len = cur_title.chars().count();
    let title = if (30..=60).contains(&title_len) {
        cur_title
    } else {
        let room = 60usize.saturating_sub(brand.chars().count() + 3);
        let core = cut_words(&topic, room.max(20));
        if brand.is_empty() || core.to_lowercase().contains(&brand.to_lowercase()) || room < 15 {
            cut_words(&core, 60)
        } else {
            format!("{core} | {brand}")
        }
    };

    let cur_desc = h.and_then(|h| h.meta_descriptions.first()).cloned().unwrap_or_default();
    let desc_len = cur_desc.chars().count();
    let description = if (70..=155).contains(&desc_len) {
        cur_desc
    } else if desc_len > 155 {
        let mut d = cut_words(&cur_desc, 150);
        d.push('…');
        d
    } else {
        // Build from the page's lead paragraph, falling back to its opening text.
        let text = h.map(|h| if h.lead.is_empty() { h.text.as_str() } else { h.lead.as_str() }).unwrap_or("");
        let mut d = String::new();
        for sentence in text.split_inclusive(['.', '!', '?']) {
            let s = sentence.trim();
            if s.split_whitespace().count() < 4 {
                continue;
            }
            if d.chars().count() + s.chars().count() > 150 {
                break;
            }
            if !d.is_empty() {
                d.push(' ');
            }
            d.push_str(s);
        }
        if d.chars().count() < 70 {
            d = cut_words(format!("{topic}. {text}").trim(), 150);
        }
        let mut chars = d.chars();
        match chars.next() {
            Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            None => format!("Learn more about {topic} on {brand}."),
        }
    };
    (title, description)
}

fn prompt(p: &Page, brand: &str) -> String {
    let h = p.html.as_deref();
    let excerpt: String = h.map(|h| h.text.chars().take(1800).collect()).unwrap_or_default();
    format!(
        "Write an SEO page title (at most 60 characters) and a meta description (120 to 155 characters) for the web page below. \
Use the page's own language. Be specific and accurate to the content, put the main topic first, and do not invent facts, prices or claims. \
Reply with JSON only, exactly: {{\"title\": \"...\", \"description\": \"...\"}}\n\n\
Site name: {brand}\nURL: {}\nCurrent title: {}\nH1: {}\nCurrent meta description: {}\nPage text excerpt:\n{excerpt}",
        p.url,
        h.and_then(|h| h.titles.first()).map(|s| s.as_str()).unwrap_or("(none)"),
        h.and_then(|h| h.h1.first()).map(|s| s.as_str()).unwrap_or("(none)"),
        h.and_then(|h| h.meta_descriptions.first()).map(|s| s.as_str()).unwrap_or("(none)"),
    )
}

fn client() -> Result<reqwest::blocking::Client, String> {
    // Local models on a CPU can take minutes per answer; this runs in the background.
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent(crate::crawler::DEFAULT_UA)
        .build()
        .map_err(|e| e.to_string())
}

fn net_error(e: reqwest::Error) -> String {
    use std::error::Error;
    let mut msg = if e.is_timeout() { "The request timed out".to_string() } else { e.to_string() };
    let mut src = e.source();
    let mut last = None;
    while let Some(s) = src {
        last = Some(s.to_string());
        src = s.source();
    }
    if let Some(l) = last {
        msg = format!("{msg}: {l}");
    }
    msg
}

fn api_error(status: reqwest::StatusCode, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.pointer("/message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    format!("HTTP {}: {msg}", status.as_u16())
}

/// Models that accept Anthropic's server-side refusal fallback.
fn anthropic_fallbacks(model: &str) -> bool {
    ["claude-fable-5-2", "claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"].contains(&model)
}

/// Send one prompt to the configured provider and return the reply text.
pub fn complete(cfg: &AiConfig, system: &str, user: &str) -> Result<String, String> {
    let http = client()?;
    match cfg.provider {
        Provider::Offline => Err("No AI provider selected".into()),
        Provider::OpenAiCompatible => {
            let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
            let mut req = http.post(url).json(&json!({
                "model": cfg.model,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            }));
            if !cfg.api_key.trim().is_empty() {
                req = req.bearer_auth(cfg.api_key.trim());
            }
            let resp = req.send().map_err(net_error)?;
            let status = resp.status();
            let body = resp.text().map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(api_error(status, &body));
            }
            let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            v.pointer("/choices/0/message/content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| "The response contained no text".into())
        }
        Provider::Anthropic => {
            let mut body = json!({
                "model": cfg.model,
                "max_tokens": 4000,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            });
            let mut req = http
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", cfg.api_key.trim())
                .header("anthropic-version", "2026-10-21");
            if anthropic_fallbacks(&cfg.model) {
                body["fallbacks"] = json!("default");
                req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
            }
            let resp = req.json(&body).send().map_err(net_error)?;
            let status = resp.status();
            let text = resp.text().map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(api_error(status, &text));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if v.get("stop_reason").and_then(|s| s.as_str()) == Some("refusal") {
                return Err("The model declined this request".into());
            }
            let out: String = v
                .get("content")
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if out.trim().is_empty() {
                Err("The response contained no text".into())
            } else {
                Ok(out)
            }
        }
        Provider::Gemini => {
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", cfg.model.trim());
            let resp = http
                .post(url)
                .header("x-goog-api-key", cfg.api_key.trim())
                .json(&json!({
                    "systemInstruction": {"parts": [{"text": system}]},
                    "contents": [{"role": "user", "parts": [{"text": user}]}],
                    "generationConfig": {"responseMimeType": "application/json"},
                }))
                .send()
                .map_err(net_error)?;
            let status = resp.status();
            let text = resp.text().map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(api_error(status, &text));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let out: String = v
                .pointer("/candidates/0/content/parts")
                .and_then(|p| p.as_array())
                .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""))
                .unwrap_or_default();
            if out.trim().is_empty() {
                Err("The response contained no text".into())
            } else {
                Ok(out)
            }
        }
    }
}

fn parse_pair(text: &str) -> Result<(String, String), String> {
    let start = text.find('{').ok_or("The model did not return JSON")?;
    let end = text.rfind('}').ok_or("The model did not return JSON")?;
    let v: Value = serde_json::from_str(&text[start..=end]).map_err(|e| format!("Could not read the model's JSON: {e}"))?;
    let get = |k: &str| v.get(k).and_then(|s| s.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match (get("title"), get("description")) {
        (Some(t), Some(d)) => Ok((t, d)),
        _ => Err("The model's JSON lacked a title or description".into()),
    }
}

/// Produce a suggestion for one page, using AI when configured.
pub fn suggest(cfg: &AiConfig, p: &Page, reason: &str, brand: &str) -> Suggestion {
    let mut s = Suggestion {
        url: p.url.clone(),
        reason: reason.to_string(),
        current_title: p.title().to_string(),
        current_description: p.description().to_string(),
        ..Default::default()
    };
    if cfg.provider != Provider::Offline && cfg.ready() {
        let system = "You are an expert SEO copywriter. You write accurate, compelling page titles and meta descriptions.";
        match complete(cfg, system, &prompt(p, brand)).and_then(|t| parse_pair(&t)) {
            Ok((t, d)) => {
                s.title = t;
                s.description = d;
                s.source = format!("AI ({})", cfg.model);
                return s;
            }
            Err(e) => s.error = Some(e),
        }
    }
    let (t, d) = offline_suggestion(p, brand);
    s.title = t;
    s.description = d;
    s.source = if s.error.is_some() { "Offline (AI failed)".into() } else { "Offline rules".into() };
    s
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LinkOpportunity {
    pub source_url: String,
    pub target_url: String,
    pub target_title: String,
    pub phrase: String,
    pub mentions: usize,
    pub context: String,
}

const GENERIC_PHRASES: &[&str] = &[
    "home", "homepage", "about", "about us", "contact", "contact us", "blog", "news", "services", "products", "shop",
    "privacy policy", "terms and conditions", "terms of service", "cookie policy", "login", "sign in", "sign up", "faq",
    "frequently asked questions", "our team", "careers", "search", "read more", "learn more", "get started",
];

fn clean_phrase(s: &str) -> String {
    let lower = s.to_lowercase();
    let trimmed = lower.trim_matches(|c: char| !c.is_alphanumeric());
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Find pages that mention another page's topic (its H1 or title) in their text
/// without linking to it.
pub fn link_opportunities(crawl: &Crawl) -> Vec<LinkOpportunity> {
    let brand = site_name(crawl);
    let pages = &crawl.pages;
    // phrase -> target index; phrases shared by several pages are ambiguous and dropped.
    let mut phrase_target: HashMap<String, Option<usize>> = HashMap::new();
    for (i, p) in pages.iter().enumerate() {
        let Some(h) = ihd(p) else { continue };
        // Archive pages (tags, categories, authors) make weak link targets.
        let path = url::Url::parse(&p.url).map(|u| u.path().to_lowercase()).unwrap_or_default();
        if path.split('/').any(|seg| ["tag", "tags", "category", "categories", "author", "authors", "page", "search", "feed"].contains(&seg)) {
            continue;
        }
        let mut phrases = Vec::new();
        if let Some(h1) = h.h1.first() {
            phrases.push(clean_phrase(h1));
        }
        if let Some(t) = h.titles.first() {
            phrases.push(clean_phrase(&strip_brand(t, &brand)));
        }
        phrases.sort();
        phrases.dedup();
        for ph in phrases {
            let words = ph.split(' ').count();
            if !(2..=8).contains(&words) || ph.len() < 8 || GENERIC_PHRASES.contains(&ph.as_str()) || ph == clean_phrase(&brand) {
                continue;
            }
            phrase_target
                .entry(ph)
                .and_modify(|t| {
                    if *t != Some(i) {
                        *t = None
                    }
                })
                .or_insert(Some(i));
        }
    }
    let list: Vec<(String, usize)> = phrase_target.into_iter().filter_map(|(ph, t)| t.map(|t| (ph, t))).collect();
    if list.is_empty() {
        return Vec::new();
    }
    let Ok(ac) = AhoCorasick::builder()
        .match_kind(MatchKind::LeftmostLongest)
        .build(list.iter().map(|(p, _)| p.as_str()))
    else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (si, src) in pages.iter().enumerate() {
        if !indexable(src) {
            continue;
        }
        let Some(h) = src.html.as_deref() else { continue };
        let lowered = h.text.to_lowercase();
        let text = lowered.as_str();
        let bytes = text.as_bytes();
        let mut found: HashMap<usize, (usize, usize)> = HashMap::new(); // pattern -> (count, first offset)
        for m in ac.find_iter(text) {
            let before_ok = m.start() == 0 || !(bytes[m.start() - 1] as char).is_alphanumeric();
            let after_ok = m.end() == bytes.len() || !(bytes[m.end()] as char).is_alphanumeric();
            if before_ok && after_ok {
                let e = found.entry(m.pattern().as_usize()).or_insert((0, m.start()));
                e.0 += 1;
            }
        }
        for (pi, (count, at)) in found {
            let (phrase, ti) = &list[pi];
            if *ti == si {
                continue;
            }
            let target = &pages[*ti];
            if h.links.iter().any(|l| l.url == target.url) {
                continue;
            }
            let mut a = at.saturating_sub(80);
            while !text.is_char_boundary(a) {
                a -= 1;
            }
            let mut b = (at + phrase.len() + 80).min(text.len());
            while !text.is_char_boundary(b) {
                b += 1;
            }
            out.push(LinkOpportunity {
                source_url: src.url.clone(),
                target_url: target.url.clone(),
                target_title: target.title().to_string(),
                phrase: phrase.clone(),
                mentions: count,
                context: format!("…{}…", text[a..b].trim()),
            });
        }
    }
    out.sort_by(|a, b| b.mentions.cmp(&a.mentions).then(a.source_url.cmp(&b.source_url)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_json_with_noise() {
        let (t, d) = parse_pair("Sure!\n```json\n{\"title\": \"A\", \"description\": \"B\"}\n```").unwrap();
        assert_eq!((t.as_str(), d.as_str()), ("A", "B"));
        assert!(parse_pair("no json").is_err());
    }

    #[test]
    fn cut_words_respects_limit() {
        let s = cut_words("one two three four five six seven", 15);
        assert!(s.chars().count() <= 15);
        assert_eq!(s, "one two three");
        assert_eq!(cut_words("Brand identity design for startups and small businesses", 44), "Brand identity design for startups");
        assert_eq!(cut_words("Web design, SEO and branding for local shops", 30), "Web design, SEO and branding");
    }
}
