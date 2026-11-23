//! Core Web Vitals (CrUX) key dialog, background fetch and inspector tab.

use super::{App, State, GOOD, ISSUE, WARNING};
use crate::cwv::{self, CruxConfig, Metric, Metrics, Rating, Results, METRICS};
use crate::model::{Crawl, Page};
use eframe::egui::{self, RichText};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

enum Msg {
    Progress(usize, usize),
    Done(Box<Results>),
}

/// Not saved: the API key must never reach disk.
pub(super) struct CwvState {
    pub show: bool,
    cfg: CruxConfig,
    auto: bool,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    progress: (usize, usize),
    status: String,
}

impl Default for CwvState {
    fn default() -> Self {
        CwvState {
            show: false,
            cfg: CruxConfig::default(),
            auto: true,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: (0, 0),
            status: String::new(),
        }
    }
}

impl App {
    fn start_cwv(&mut self) {
        let (origin, urls) = cwv::targets(&self.crawl, self.cwv.cfg.max_urls);
        if origin.is_empty() {
            return;
        }
        let cfg = self.cwv.cfg.clone();
        let (tx, rx) = channel();
        self.cwv.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cwv.cancel.clone();
        self.cwv.progress = (0, urls.len() + 1);
        self.cwv.status = format!("Querying CrUX for {origin} and {} URLs…", urls.len());
        std::thread::spawn(move || {
            let r = cwv::fetch(&cfg, &origin, &urls, &cancel, |d, t| {
                let _ = tx.send(Msg::Progress(d, t));
            });
            let _ = tx.send(Msg::Done(Box::new(r)));
        });
        self.cwv.rx = Some(rx);
    }

    /// After a crawl: fetch field data when a key is set and auto-fetch is on.
    pub(super) fn after_crawl_cwv(&mut self) {
        if self.cwv.auto
            && !self.cwv.cfg.api_key.trim().is_empty()
            && self.cwv.rx.is_none()
            && self.state == State::Done
        {
            self.start_cwv();
        }
    }

    pub(super) fn pump_cwv(&mut self, ctx: &egui::Context) {
        let mut done = None;
        if let Some(rx) = &self.cwv.rx {
            ctx.request_repaint_after(Duration::from_millis(250));
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(d, t) => self.cwv.progress = (d, t),
                    Msg::Done(r) => done = Some(r),
                }
            }
        }
        let Some(r) = done else { return };
        self.cwv.rx = None;
        cwv::apply(&mut self.crawl, &r);
        self.run_audit();
        let with = r.urls.iter().filter(|(_, c)| c.has_data()).count();
        self.cwv.status = format!(
            "{with} of {} URLs have field data; origin {}. {} queries.{}",
            r.urls.len(),
            if r.origin.as_ref().is_some_and(|o| o.has_data()) {
                "has data"
            } else {
                "has no data"
            },
            r.queries,
            r.error
                .as_ref()
                .map(|e| format!(" Stopped: {e}"))
                .unwrap_or_default()
        );
        match &r.error {
            Some(e) => self.error(format!("Core Web Vitals: {e}")),
            None => self.toast(format!("Core Web Vitals: {}", self.cwv.status)),
        }
    }

    pub(super) fn cwv_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.cwv.show;
        let mut fetch = false;
        let has = self.has_data() && !matches!(self.state, State::Crawling | State::Paused);
        egui::Window::new("Core Web Vitals (Chrome UX Report)")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Real-user field data (LCP, INP, CLS, FCP, TTFB) for the site and each indexable URL, from Google's free Chrome UX Report API.");
                ui.label(RichText::new("Create a key at console.cloud.google.com (enable the Chrome UX Report API). The key stays in memory only and is never saved; CRUX_API_KEY in the environment is picked up too.").small().weak());
                ui.add_space(4.0);
                let s = &mut self.cwv;
                egui::Grid::new("crux").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("API key");
                    ui.add(egui::TextEdit::singleline(&mut s.cfg.api_key).password(true).desired_width(320.0));
                    ui.end_row();
                    ui.label("URLs to query");
                    ui.add(egui::DragValue::new(&mut s.cfg.max_urls).range(0..=100_000).speed(10));
                    ui.end_row();
                });
                ui.checkbox(&mut s.auto, "Fetch automatically after each crawl");
                ui.label(RichText::new(format!(
                    "Free quota: 150 queries a minute. Each URL takes 2 queries (phone and desktop), so {} URLs take about {} min.",
                    s.cfg.max_urls,
                    (s.cfg.max_urls * 2 + 2).div_ceil(150)
                )).small().weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if s.rx.is_some() {
                        let (d, t) = s.progress;
                        ui.add(egui::ProgressBar::new(d as f32 / t.max(1) as f32).text(format!("{d}/{t}")).desired_width(240.0));
                        if ui.button("Cancel").clicked() {
                            s.cancel.store(true, Ordering::Relaxed);
                        }
                    } else if ui
                        .add_enabled(has && !s.cfg.api_key.trim().is_empty(), egui::Button::new("Fetch now"))
                        .clicked()
                    {
                        fetch = true;
                    }
                });
                if !s.status.is_empty() {
                    ui.label(RichText::new(&s.status).small());
                }
            });
        self.cwv.show = open;
        if fetch {
            self.start_cwv();
        }
    }
}

fn metric_cell(ui: &mut egui::Ui, m: Metric, f: Option<&Metrics>) {
    match f.and_then(|f| m.value(f)) {
        Some(v) => {
            let r = m.rate(v);
            let color = match r {
                Rating::Good => GOOD,
                Rating::NeedsImprovement => WARNING,
                Rating::Poor => ISSUE,
            };
            ui.label(RichText::new(format!("{}  {}", m.format(v), r.label())).color(color))
        }
        None => ui.label(RichText::new("no data").weak()),
    };
}

fn table(ui: &mut egui::Ui, id: &str, phone: Option<&Metrics>, desktop: Option<&Metrics>) {
    egui::Grid::new(id)
        .num_columns(3)
        .striped(true)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            for t in ["p75", "Phone", "Desktop"] {
                ui.label(RichText::new(t).strong());
            }
            ui.end_row();
            for m in METRICS {
                ui.label(m.label());
                metric_cell(ui, m, phone);
                metric_cell(ui, m, desktop);
                ui.end_row();
            }
        });
}

pub(super) fn vitals_tab(ui: &mut egui::Ui, p: &Page, crawl: &Crawl) {
    match p.cwv.as_deref() {
        None => {
            ui.label(RichText::new("Not queried. Add a Chrome UX Report API key in Configuration → Core Web Vitals (CrUX)… and fetch.").weak());
        }
        Some(c) if !c.has_data() => {
            ui.label("No field data for this URL: it has too few Chrome visitors for the Chrome UX Report.");
        }
        Some(c) => {
            ui.label(RichText::new("This URL (75th percentile, last 28 days)").strong());
            table(ui, "cwv_url", c.phone.as_ref(), c.desktop.as_ref());
        }
    }
    if let Some(o) = &crawl.site.cwv_origin {
        ui.add_space(8.0);
        ui.label(RichText::new("Whole origin").strong());
        if o.has_data() {
            table(ui, "cwv_origin", o.phone.as_ref(), o.desktop.as_ref());
        } else {
            ui.label("No field data for the origin.");
        }
    }
}
