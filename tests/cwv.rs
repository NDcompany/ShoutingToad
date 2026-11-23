//! Core Web Vitals client against a mock Chrome UX Report API: good, poor, no data, quota
//! errors and a bad key, then the checks on the stored data.

use shouting_toad::cwv::{self, CruxConfig};
use shouting_toad::model::{Crawl, Page, Source};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const GOOD: &str = r#"{"record":{"metrics":{"largest_contentful_paint":{"percentiles":{"p75":1800}},"interaction_to_next_paint":{"percentiles":{"p75":120}},"cumulative_layout_shift":{"percentiles":{"p75":"0.02"}},"first_contentful_paint":{"percentiles":{"p75":900}},"experimental_time_to_first_byte":{"percentiles":{"p75":300}}}}}"#;
const POOR: &str = r#"{"record":{"metrics":{"largest_contentful_paint":{"percentiles":{"p75":5200}},"interaction_to_next_paint":{"percentiles":{"p75":340}},"cumulative_layout_shift":{"percentiles":{"p75":"0.31"}}}}}"#;
const NOT_FOUND: &str =
    r#"{"error":{"code":404,"message":"chrome ux report data not found","status":"NOT_FOUND"}}"#;

/// Answers by what the request body asks for. `quota_first` 429s come before any real answer.
fn mock(quota_first: usize) -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let n = h.fetch_add(1, Ordering::SeqCst);
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            // headers, then Content-Length bytes of body
            let body = loop {
                let k = s.read(&mut chunk).unwrap_or(0);
                if k == 0 {
                    break String::new();
                }
                buf.extend_from_slice(&chunk[..k]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len: usize = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if buf.len() >= i + 4 + len {
                        break text[i + 4..].to_string();
                    }
                }
            };
            let head = String::from_utf8_lossy(&buf).to_string();
            let (status, json) = if !head.contains("key=test-key") {
                (
                    400,
                    r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key."}}"#,
                )
            } else if n < quota_first || body.contains("quota") {
                (429, r#"{"error":{"code":429,"message":"Quota exceeded"}}"#)
            } else if body.contains("nodata") {
                (404, NOT_FOUND)
            } else if body.contains("poor") {
                (200, POOR)
            } else {
                (200, GOOD)
            };
            let reason = match status {
                200 => "OK",
                404 => "Not Found",
                429 => "Too Many Requests",
                _ => "Bad Request",
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nRetry-After: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                )
                .as_bytes(),
            );
        }
    });
    (
        format!("http://127.0.0.1:{port}/v1/records:queryRecord"),
        hits,
    )
}

fn cfg(endpoint: &str, key: &str) -> CruxConfig {
    CruxConfig {
        api_key: key.into(),
        max_urls: 500,
        per_minute: 60_000,
        endpoint: endpoint.into(),
    }
}

fn crawl(origin_kind: &str) -> Crawl {
    let page = |id: usize, path: &str| Page {
        id,
        url: format!("https://{origin_kind}.test{path}"),
        source: Some(if id == 1 { Source::Seed } else { Source::Link }),
        is_internal: true,
        status: 200,
        content_type: "text/html".into(),
        html: Some(Box::default()),
        ..Default::default()
    };
    Crawl {
        start_url: format!("https://{origin_kind}.test/"),
        pages: vec![
            page(1, "/"),
            page(2, "/good"),
            page(3, "/poor"),
            page(4, "/nodata"),
        ],
        ..Default::default()
    }
}

fn fires(c: &Crawl, id: &str) -> Vec<String> {
    let a = shouting_toad::audit::run(c);
    let i = shouting_toad::checks::find(id).unwrap();
    a.hits[i].iter().map(|&p| c.pages[p].url.clone()).collect()
}

#[test]
fn good_poor_and_no_data() {
    let (endpoint, hits) = mock(0);
    let mut c = crawl("site");
    let (origin, urls) = cwv::targets(&c, 500);
    assert_eq!(origin, "https://site.test");
    assert_eq!(urls.len(), 4);
    let mut progress = Vec::new();
    let r = cwv::fetch(
        &cfg(&endpoint, "test-key"),
        &origin,
        &urls,
        &AtomicBool::new(false),
        |d, t| progress.push((d, t)),
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.queries, 10, "origin + 4 URLs, phone and desktop");
    assert_eq!(hits.load(Ordering::SeqCst), 10);
    assert_eq!(progress.last(), Some(&(5, 5)));
    cwv::apply(&mut c, &r);

    let good = c.pages[1].cwv.as_deref().unwrap();
    assert_eq!(good.phone.as_ref().unwrap().lcp_ms, Some(1800.0));
    assert_eq!(good.desktop.as_ref().unwrap().cls, Some(0.02));
    let none = c.pages[3].cwv.as_deref().unwrap();
    assert!(!none.has_data(), "404 means no data");
    assert!(c.site.cwv_origin.as_ref().unwrap().has_data());

    let poor = vec!["https://site.test/poor".to_string()];
    assert_eq!(fires(&c, "cwv.lcp_poor"), poor);
    assert_eq!(fires(&c, "cwv.cls_poor"), poor);
    assert_eq!(fires(&c, "cwv.inp_needs_improvement"), poor);
    assert!(fires(&c, "cwv.inp_poor").is_empty());
    assert!(
        fires(&c, "cwv.origin_lcp_poor").is_empty(),
        "origin is good"
    );

    // CSV columns and the report section
    let csv = shouting_toad::report::internal_csv(&c, &shouting_toad::audit::run(&c));
    assert!(csv.lines().next().unwrap().contains("LCP p75 Phone (ms)"));
    assert!(csv
        .lines()
        .any(|l| l.starts_with("https://site.test/poor,") && l.contains(",5200,")));
    let html = cwv::report_section(&c);
    assert!(html.contains("Core Web Vitals") && html.contains("5.2 s"));
}

#[test]
fn poor_origin_fires_on_the_start_url() {
    let (endpoint, _) = mock(0);
    let mut c = crawl("poor");
    let (origin, urls) = cwv::targets(&c, 1);
    assert_eq!(urls.len(), 1, "URL cap");
    let r = cwv::fetch(
        &cfg(&endpoint, "test-key"),
        &origin,
        &urls,
        &AtomicBool::new(false),
        |_, _| {},
    );
    cwv::apply(&mut c, &r);
    assert_eq!(fires(&c, "cwv.origin_lcp_poor"), vec!["https://poor.test/"]);
    assert_eq!(
        fires(&c, "cwv.origin_inp_needs_improvement"),
        vec!["https://poor.test/"]
    );
}

#[test]
fn rate_limits_are_retried_and_quota_errors_stop() {
    // two 429s, then normal answers: retried transparently
    let (endpoint, hits) = mock(2);
    let c = crawl("site");
    let (origin, urls) = cwv::targets(&c, 1);
    let r = cwv::fetch(
        &cfg(&endpoint, "test-key"),
        &origin,
        &urls,
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(hits.load(Ordering::SeqCst), 6);
    assert!(r.origin.unwrap().has_data());

    // a quota that never recovers: stops with a clear error, keeps what it had
    let (endpoint, _) = mock(0);
    let c = crawl("quota");
    let (origin, urls) = cwv::targets(&c, 10);
    let r = cwv::fetch(
        &cfg(&endpoint, "test-key"),
        &origin,
        &urls,
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(
        r.error.as_deref().unwrap().contains("quota"),
        "{:?}",
        r.error
    );
    assert!(r.urls.is_empty());

    // a bad key stops at once
    let (endpoint, hits) = mock(0);
    let r = cwv::fetch(
        &cfg(&endpoint, "wrong"),
        &origin,
        &urls,
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(r.error.as_deref().unwrap().contains("API key not valid"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // no key: nothing is sent
    let r = cwv::fetch(
        &cfg(&endpoint, ""),
        &origin,
        &urls,
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(r.error.is_some() && r.queries == 0);
}
