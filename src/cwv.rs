//! Core Web Vitals field data from the Chrome UX Report (CrUX) API, with the user's own free
//! Google API key. The key is never written to disk.
//!
//! After a crawl, the origin and each indexable URL (up to a cap) are queried for the phone and
//! desktop form factors. The free quota is 150 queries per minute per Google Cloud project, so
//! queries are spaced to stay under it, and a 429 backs off and retries.

use crate::model::{Crawl, Page};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_ENDPOINT: &str = "https://chromeuxreport.googleapis.com/v1/records:queryRecord";

#[derive(Clone, Debug)]
pub struct CruxConfig {
    pub api_key: String,
    /// URLs to query besides the origin.
    pub max_urls: usize,
    /// Queries per minute.
    pub per_minute: u32,
    pub endpoint: String,
}

impl Default for CruxConfig {
    fn default() -> Self {
        CruxConfig {
            api_key: env_key(),
            max_urls: 500,
            per_minute: 150,
            endpoint: DEFAULT_ENDPOINT.into(),
        }
    }
}

/// `CRUX_API_KEY` from the environment, or empty.
pub fn env_key() -> String {
    std::env::var("CRUX_API_KEY")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 75th percentile values. `None` when CrUX has no value for that metric.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub lcp_ms: Option<f64>,
    pub inp_ms: Option<f64>,
    pub cls: Option<f64>,
    pub fcp_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
}

/// CrUX data for one URL or origin. A form factor of `None` means CrUX has no data for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CwvRecord {
    pub phone: Option<Metrics>,
    pub desktop: Option<Metrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CwvRecord {
    pub fn has_data(&self) -> bool {
        self.phone.is_some() || self.desktop.is_some()
    }

    /// Worst rating of a metric across form factors.
    pub fn worst(&self, m: Metric) -> Option<Rating> {
        [&self.phone, &self.desktop]
            .into_iter()
            .flatten()
            .filter_map(|x| m.value(x).map(|v| m.rate(v)))
            .max()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rating {
    Good,
    NeedsImprovement,
    Poor,
}

impl Rating {
    pub fn label(self) -> &'static str {
        match self {
            Rating::Good => "Good",
            Rating::NeedsImprovement => "Needs improvement",
            Rating::Poor => "Poor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Lcp,
    Inp,
    Cls,
    Fcp,
    Ttfb,
}

pub const METRICS: [Metric; 5] = [
    Metric::Lcp,
    Metric::Inp,
    Metric::Cls,
    Metric::Fcp,
    Metric::Ttfb,
];

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Metric::Lcp => "LCP",
            Metric::Inp => "INP",
            Metric::Cls => "CLS",
            Metric::Fcp => "FCP",
            Metric::Ttfb => "TTFB",
        }
    }

    /// Google's published thresholds: (good up to, poor above).
    pub fn thresholds(self) -> (f64, f64) {
        match self {
            Metric::Lcp => (2500.0, 4000.0),
            Metric::Inp => (200.0, 500.0),
            Metric::Cls => (0.1, 0.25),
            Metric::Fcp => (1800.0, 3000.0),
            Metric::Ttfb => (800.0, 1800.0),
        }
    }

    pub fn rate(self, v: f64) -> Rating {
        let (good, poor) = self.thresholds();
        if v <= good {
            Rating::Good
        } else if v <= poor {
            Rating::NeedsImprovement
        } else {
            Rating::Poor
        }
    }

    pub fn value(self, m: &Metrics) -> Option<f64> {
        match self {
            Metric::Lcp => m.lcp_ms,
            Metric::Inp => m.inp_ms,
            Metric::Cls => m.cls,
            Metric::Fcp => m.fcp_ms,
            Metric::Ttfb => m.ttfb_ms,
        }
    }

    pub fn format(self, v: f64) -> String {
        match self {
            Metric::Cls => format!("{v:.2}"),
            _ if v >= 1000.0 => format!("{:.1} s", v / 1000.0),
            _ => format!("{v:.0} ms"),
        }
    }
}

/// Parse a queryRecord response body.
pub fn parse_record(body: &Value) -> Metrics {
    let m = &body["record"]["metrics"];
    let p75 = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| match &m[*n]["percentiles"]["p75"] {
                Value::Number(x) => x.as_f64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })
    };
    Metrics {
        lcp_ms: p75(&["largest_contentful_paint"]),
        inp_ms: p75(&["interaction_to_next_paint"]),
        cls: p75(&["cumulative_layout_shift"]),
        fcp_ms: p75(&["first_contentful_paint"]),
        ttfb_ms: p75(&["experimental_time_to_first_byte", "time_to_first_byte"]),
    }
}

enum Answer {
    Data(Metrics),
    NoData,
    /// Stop querying: bad key, API not enabled, quota exhausted.
    Fatal(String),
}

/// Paces queries to the configured rate and retries rate-limited ones.
struct Client {
    http: reqwest::blocking::Client,
    cfg: CruxConfig,
    next: Instant,
    pub queries: usize,
}

impl Client {
    fn query(&mut self, target: (&str, &str), form: &str, stop: &AtomicBool) -> Answer {
        let interval = Duration::from_secs_f64(60.0 / self.cfg.per_minute.max(1) as f64);
        let mut body = json!({"formFactor": form});
        body[target.0] = json!(target.1);
        for attempt in 0..4 {
            if !wait_until(self.next, stop) {
                return Answer::Fatal("Stopped".into());
            }
            self.next = Instant::now() + interval;
            self.queries += 1;
            let resp = self
                .http
                .post(&self.cfg.endpoint)
                .query(&[("key", self.cfg.api_key.as_str())])
                .json(&body)
                .send();
            let r = match resp {
                Ok(r) => r,
                Err(_) if attempt < 2 => continue,
                Err(e) => return Answer::Fatal(format!("CrUX API unreachable: {e}")),
            };
            let status = r.status().as_u16();
            let retry_after = r
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let v: Value = r.json().unwrap_or_default();
            let msg = v["error"]["message"].as_str().unwrap_or("").to_string();
            match status {
                200 => return Answer::Data(parse_record(&v)),
                404 => return Answer::NoData,
                429 if attempt < 3 => {
                    // per-minute quota: wait for the window to roll over
                    self.next = Instant::now() + Duration::from_secs(retry_after.unwrap_or(60));
                }
                429 => {
                    return Answer::Fatal(
                        "CrUX API quota exceeded (429); try again later or lower the URL cap"
                            .into(),
                    )
                }
                400 | 401 | 403 => {
                    return Answer::Fatal(format!(
                        "CrUX API refused the request ({status}): {}",
                        if msg.is_empty() {
                            "check the API key and that the Chrome UX Report API is enabled"
                        } else {
                            &msg
                        }
                    ))
                }
                s if s >= 500 && attempt < 2 => {}
                s => return Answer::Fatal(format!("CrUX API error {s}: {msg}")),
            }
        }
        Answer::Fatal("CrUX API kept failing".into())
    }

    fn record(&mut self, target: (&str, &str), stop: &AtomicBool) -> Result<CwvRecord, String> {
        let mut rec = CwvRecord::default();
        for (form, slot) in [("PHONE", 0), ("DESKTOP", 1)] {
            let m = match self.query(target, form, stop) {
                Answer::Data(m) => Some(m),
                Answer::NoData => None,
                Answer::Fatal(e) => return Err(e),
            };
            if slot == 0 {
                rec.phone = m;
            } else {
                rec.desktop = m;
            }
        }
        Ok(rec)
    }
}

/// Sleep until `t`, waking to check `stop`. False when stopped.
fn wait_until(t: Instant, stop: &AtomicBool) -> bool {
    while Instant::now() < t {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep((t - Instant::now()).min(Duration::from_millis(100)));
    }
    !stop.load(Ordering::Relaxed)
}

/// The origin and the indexable URLs to query (up to `max`).
pub fn targets(crawl: &Crawl, max: usize) -> (String, Vec<String>) {
    let origin = url::Url::parse(&crawl.start_url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_default();
    let urls = crawl
        .pages
        .iter()
        .filter(|p| crate::audit::indexable(p))
        .map(|p| p.url.clone())
        .take(max)
        .collect();
    (origin, urls)
}

#[derive(Clone, Debug, Default)]
pub struct Results {
    pub origin: Option<CwvRecord>,
    pub urls: Vec<(String, CwvRecord)>,
    /// Set when querying stopped early; what was fetched before is kept.
    pub error: Option<String>,
    pub queries: usize,
}

/// Query CrUX for the origin and `urls`. `progress(done, total)` is called after each URL.
pub fn fetch(
    cfg: &CruxConfig,
    origin: &str,
    urls: &[String],
    stop: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Results {
    let mut out = Results::default();
    if cfg.api_key.trim().is_empty() {
        out.error = Some("No CrUX API key".into());
        return out;
    }
    let http = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    {
        Ok(h) => h,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    let mut c = Client {
        http,
        cfg: cfg.clone(),
        next: Instant::now(),
        queries: 0,
    };
    let total = urls.len() + 1;
    match c.record(("origin", origin), stop) {
        Ok(r) => out.origin = Some(r),
        Err(e) => {
            out.error = Some(e);
            out.queries = c.queries;
            return out;
        }
    }
    progress(1, total);
    for (i, u) in urls.iter().enumerate() {
        match c.record(("url", u), stop) {
            Ok(r) => out.urls.push((u.clone(), r)),
            Err(e) => {
                out.error = Some(e);
                break;
            }
        }
        progress(i + 2, total);
    }
    out.queries = c.queries;
    out
}

/// Store fetched data on the crawl (replacing earlier CrUX data).
pub fn apply(crawl: &mut Crawl, r: &Results) {
    if r.origin.is_some() {
        crawl.site.cwv_origin = r.origin.clone();
    }
    let by_url: std::collections::HashMap<&str, &CwvRecord> =
        r.urls.iter().map(|(u, rec)| (u.as_str(), rec)).collect();
    for p in &mut crawl.pages {
        if let Some(rec) = by_url.get(p.url.as_str()) {
            p.cwv = Some(Box::new((*rec).clone()));
        }
    }
}

// ---------------------------------------------------------------- exports

pub const CSV_HEADERS: [&str; 10] = [
    "LCP p75 Phone (ms)",
    "INP p75 Phone (ms)",
    "CLS p75 Phone",
    "FCP p75 Phone (ms)",
    "TTFB p75 Phone (ms)",
    "LCP p75 Desktop (ms)",
    "INP p75 Desktop (ms)",
    "CLS p75 Desktop",
    "FCP p75 Desktop (ms)",
    "TTFB p75 Desktop (ms)",
];

/// Cells for [`CSV_HEADERS`]: empty when not queried or no data.
pub fn csv_cells(p: &Page) -> Vec<String> {
    let mut out = Vec::with_capacity(10);
    for form in [
        p.cwv.as_ref().and_then(|c| c.phone.as_ref()),
        p.cwv.as_ref().and_then(|c| c.desktop.as_ref()),
    ] {
        for m in METRICS {
            out.push(
                form.and_then(|f| m.value(f))
                    .map(|v| {
                        if m == Metric::Cls {
                            format!("{v:.2}")
                        } else {
                            format!("{v:.0}")
                        }
                    })
                    .unwrap_or_default(),
            );
        }
    }
    out
}

fn cell(m: Metric, form: Option<&Metrics>) -> String {
    match form.and_then(|f| m.value(f)) {
        Some(v) => {
            let cls = match m.rate(v) {
                Rating::Good => "good",
                Rating::NeedsImprovement => "ni",
                Rating::Poor => "poor",
            };
            format!("<td class=\"num cwv-{cls}\">{}</td>", m.format(v))
        }
        None => "<td class=\"num muted\">no data</td>".into(),
    }
}

/// report.html section; empty when CrUX was not queried.
pub fn report_section(crawl: &Crawl) -> String {
    use crate::report::esc;
    use std::fmt::Write;
    let Some(origin) = &crawl.site.cwv_origin else {
        return String::new();
    };
    let urls: Vec<&Page> = crawl.pages.iter().filter(|p| p.cwv.is_some()).collect();
    let head = "<tr><th>Form factor</th><th class=num>LCP</th><th class=num>INP</th><th class=num>CLS</th><th class=num>FCP</th><th class=num>TTFB</th></tr>";
    let row = |name: &str, f: Option<&Metrics>| {
        let mut r = format!("<tr><td>{name}</td>");
        for m in METRICS {
            r.push_str(&cell(m, f));
        }
        r.push_str("</tr>");
        r
    };
    let mut s = String::from(
        "<style>.cwv-good{color:var(--good)}.cwv-ni{color:var(--warning)}.cwv-poor{color:var(--issue);font-weight:600}</style>\
         <h2>Core Web Vitals (field data, Chrome UX Report)</h2>\
         <p class=muted>75th percentile of real Chrome users over the last 28 days. Thresholds: LCP 2.5 s / 4 s, INP 200 ms / 500 ms, CLS 0.1 / 0.25. Pages with too little traffic have no data.</p>",
    );
    let _ = write!(
        s,
        "<h3>Whole origin</h3><table><thead>{head}</thead><tbody>{}{}</tbody></table>",
        row("Phone", origin.phone.as_ref()),
        row("Desktop", origin.desktop.as_ref())
    );
    let with_data: Vec<&&Page> = urls
        .iter()
        .filter(|p| p.cwv.as_ref().is_some_and(|c| c.has_data()))
        .collect();
    let _ = write!(
        s,
        "<p class=muted>{} URLs queried, {} with URL-level data.</p>",
        urls.len(),
        with_data.len()
    );
    if !with_data.is_empty() {
        let mut sorted = with_data.clone();
        let worst = |p: &Page| {
            let c = p.cwv.as_deref().unwrap();
            [Metric::Lcp, Metric::Inp, Metric::Cls]
                .iter()
                .filter_map(|m| c.worst(*m))
                .max()
        };
        sorted.sort_by_key(|p| std::cmp::Reverse(worst(p)));
        let _ = write!(s, "<h3>URLs (phone)</h3><table><thead><tr><th>URL</th><th class=num>LCP</th><th class=num>INP</th><th class=num>CLS</th><th class=num>FCP</th><th class=num>TTFB</th></tr></thead><tbody>");
        for p in sorted.iter().take(50) {
            let c = p.cwv.as_deref().unwrap();
            let _ = write!(s, "<tr><td><a href=\"{u}\">{u}</a></td>", u = esc(&p.url));
            for m in METRICS {
                s.push_str(&cell(m, c.phone.as_ref()));
            }
            s.push_str("</tr>");
        }
        s.push_str("</tbody></table>");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_thresholds() {
        assert_eq!(Metric::Lcp.rate(2500.0), Rating::Good);
        assert_eq!(Metric::Lcp.rate(2501.0), Rating::NeedsImprovement);
        assert_eq!(Metric::Lcp.rate(4000.0), Rating::NeedsImprovement);
        assert_eq!(Metric::Lcp.rate(4001.0), Rating::Poor);
        assert_eq!(Metric::Inp.rate(200.0), Rating::Good);
        assert_eq!(Metric::Inp.rate(350.0), Rating::NeedsImprovement);
        assert_eq!(Metric::Inp.rate(501.0), Rating::Poor);
        assert_eq!(Metric::Cls.rate(0.1), Rating::Good);
        assert_eq!(Metric::Cls.rate(0.11), Rating::NeedsImprovement);
        assert_eq!(Metric::Cls.rate(0.26), Rating::Poor);
        let rec = CwvRecord {
            phone: Some(Metrics {
                lcp_ms: Some(5000.0),
                ..Default::default()
            }),
            desktop: Some(Metrics {
                lcp_ms: Some(1200.0),
                ..Default::default()
            }),
            error: None,
        };
        assert_eq!(rec.worst(Metric::Lcp), Some(Rating::Poor));
        assert_eq!(rec.worst(Metric::Inp), None);
    }

    #[test]
    fn parses_crux_numbers_and_strings() {
        let v: Value = serde_json::from_str(
            r#"{"record":{"metrics":{
              "largest_contentful_paint":{"percentiles":{"p75":2300}},
              "cumulative_layout_shift":{"percentiles":{"p75":"0.05"}},
              "experimental_time_to_first_byte":{"percentiles":{"p75":640}}}}}"#,
        )
        .unwrap();
        let m = parse_record(&v);
        assert_eq!(m.lcp_ms, Some(2300.0));
        assert_eq!(m.cls, Some(0.05));
        assert_eq!(m.ttfb_ms, Some(640.0));
        assert_eq!(m.inp_ms, None);
    }
}
