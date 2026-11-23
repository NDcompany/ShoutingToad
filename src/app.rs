//! Desktop interface.

use crate::ai::{self, AiConfig, LinkOpportunity, Provider, Suggestion};
use crate::audit::{self, indexability_label, pixel_width, Audit, Kind, Priority};
use crate::checks::{catalogue, CATEGORIES};
use crate::crawler::{self, Config, Event, Handle, DEFAULT_UA};
use crate::model::{Crawl, Page};
use crate::report;
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod ai_search;
mod compare;
mod migrate;

const ISSUE: Color32 = Color32::from_rgb(232, 72, 72);
const WARNING: Color32 = Color32::from_rgb(222, 160, 40);
const OPPORTUNITY: Color32 = Color32::from_rgb(70, 145, 235);
const GOOD: Color32 = Color32::from_rgb(70, 185, 105);
const TOAD: Color32 = Color32::from_rgb(88, 190, 110);

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Issue => ISSUE,
        Kind::Warning => WARNING,
        Kind::Opportunity => OPPORTUNITY,
    }
}

fn status_color(code: u16) -> Color32 {
    match code {
        200..=299 => GOOD,
        300..=399 => OPPORTUNITY,
        400..=599 => ISSUE,
        _ => Color32::GRAY,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Settings {
    cfg: Config,
    ai: AiConfig,
    last_url: String,
    list_text: String,
    theme: ThemeChoice,
    client_name: String,
    prepared_by: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Crawling,
    Paused,
    Done,
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Urls,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quick {
    All,
    Internal,
    External,
    Html,
    Images,
    Css,
    Js,
    Non200,
    Indexable,
    NonIndexable,
}

const QUICKS: &[(Quick, &str)] = &[
    (Quick::All, "All"),
    (Quick::Internal, "Internal"),
    (Quick::External, "External"),
    (Quick::Html, "HTML"),
    (Quick::Images, "Images"),
    (Quick::Css, "CSS"),
    (Quick::Js, "JavaScript"),
    (Quick::Non200, "Not 200"),
    (Quick::Indexable, "Indexable"),
    (Quick::NonIndexable, "Non-Indexable"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Id,
    Url,
    Status,
    Indexability,
    Title,
    TitleLen,
    Words,
    Depth,
    Inlinks,
    Time,
    Size,
    Issues,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Insp {
    Details,
    Issues,
    Inlinks,
    Outlinks,
    Images,
    Headers,
    Serp,
    Schema,
}

enum AiMsg {
    Progress(usize, usize),
    Item(Box<Suggestion>),
    Done,
}

pub struct App {
    settings: Settings,
    url_input: String,
    crawl: Crawl,
    url_index: std::collections::HashMap<String, usize>,
    audit: Audit,
    audit_dirty: bool,
    last_audit: Instant,
    handle: Option<Handle>,
    state: State,
    crawled: usize,
    queued: usize,
    started: Option<Instant>,
    elapsed: Duration,

    view: View,
    quick: Quick,
    selected_check: Option<usize>,
    show_kinds: [bool; 3],
    show_all_checks: bool,
    search: String,
    sort: (Col, bool),
    rows: Vec<usize>,
    rows_dirty: bool,
    last_rows: Instant,
    selected: Option<usize>,
    insp: Insp,
    mobile_serp: bool,

    show_config: bool,
    show_ai_settings: bool,
    show_ai_writer: bool,
    show_links: bool,
    show_report: bool,
    show_ci: bool,
    show_catalogue: bool,
    show_about: bool,
    show_list: bool,
    show_ai_search: bool,

    ai_rx: Option<Receiver<AiMsg>>,
    ai_cancel: Arc<AtomicBool>,
    ai_progress: (usize, usize),
    suggestions: Vec<Suggestion>,
    ai_test_rx: Option<Receiver<Result<String, String>>>,
    ai_test_result: Option<Result<String, String>>,
    opportunities: Vec<LinkOpportunity>,
    ci_url: String,
    ci_level: Priority,
    catalogue_search: String,
    catalogue_selected: Option<usize>,

    toast: Option<(String, Instant, bool)>,
    icon: Option<egui::TextureHandle>,
    screenshot: Option<String>,
    frames: usize,
    overview_stats: OverviewStats,
    compare_ui: compare::CompareUi,
    migrate_ui: migrate::MigrateUi,
}

/// Page counts for the overview, recomputed only when the crawl changes (a pass over 50k pages
/// costs ~15 ms, too much for every frame).
#[derive(Default)]
struct OverviewStats {
    key: (usize, usize, String),
    at: Option<Instant>,
    internal_html: usize,
    indexable: usize,
    /// 2xx, 3xx, 4xx, 5xx, no response
    status: [usize; 5],
    depth: std::collections::BTreeMap<usize, usize>,
    kinds: std::collections::BTreeMap<&'static str, usize>,
}

/// Free a replaced crawl off the UI thread: dropping millions of small allocations takes seconds.
fn drop_in_background<T: Send + 'static>(old: T) {
    std::thread::spawn(move || drop(old));
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        screenshot: Option<String>,
        open: Option<std::path::PathBuf>,
    ) -> Self {
        let mut app = Self::blank(cc, screenshot);
        if let Some(path) = open {
            app.load_project(&path);
        }
        app
    }

    fn blank(cc: &eframe::CreationContext, screenshot: Option<String>) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, "settings"))
            .unwrap_or_default();
        settings.ai.load_env_key();
        if settings.cfg.user_agent.trim().is_empty() {
            settings.cfg.user_agent = DEFAULT_UA.into();
        }
        apply_theme(&cc.egui_ctx, settings.theme);
        cc.egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 5.0);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
        });
        let icon = image::load_from_memory(include_bytes!("../assets/icon_256.png"))
            .ok()
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                cc.egui_ctx.load_texture(
                    "toad",
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    Default::default(),
                )
            });
        App {
            url_input: settings.last_url.clone(),
            settings,
            crawl: Crawl::default(),
            url_index: Default::default(),
            audit: Audit::default(),
            audit_dirty: false,
            last_audit: Instant::now(),
            handle: None,
            state: State::Idle,
            crawled: 0,
            queued: 0,
            started: None,
            elapsed: Duration::ZERO,
            view: View::Overview,
            quick: Quick::All,
            selected_check: None,
            show_kinds: [true, true, true],
            show_all_checks: false,
            search: String::new(),
            sort: (Col::Id, true),
            rows: Vec::new(),
            rows_dirty: true,
            last_rows: Instant::now(),
            selected: None,
            insp: Insp::Details,
            mobile_serp: false,
            show_config: false,
            show_ai_settings: false,
            show_ai_writer: false,
            show_links: false,
            show_report: false,
            show_ci: false,
            show_catalogue: false,
            show_about: false,
            show_list: false,
            show_ai_search: false,
            ai_rx: None,
            ai_cancel: Arc::new(AtomicBool::new(false)),
            ai_progress: (0, 0),
            suggestions: Vec::new(),
            ai_test_rx: None,
            ai_test_result: None,
            opportunities: Vec::new(),
            ci_url: String::new(),
            ci_level: Priority::High,
            catalogue_search: String::new(),
            catalogue_selected: None,
            toast: None,
            icon,
            screenshot,
            frames: 0,
            overview_stats: OverviewStats::default(),
            compare_ui: Default::default(),
            migrate_ui: Default::default(),
        }
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now(), true));
    }

    fn has_data(&self) -> bool {
        !self.crawl.pages.is_empty()
    }

    fn start(&mut self, seeds: Vec<String>, list: bool) {
        self.stop();
        match crawler::start(seeds.clone(), list, self.settings.cfg.clone()) {
            Ok(h) => {
                drop_in_background(std::mem::replace(
                    &mut self.crawl,
                    Crawl {
                        start_url: crawler::normalize_url(&seeds[0])
                            .map(|u| u.to_string())
                            .unwrap_or_default(),
                        started: chrono::Local::now().to_rfc3339(),
                        ..Default::default()
                    },
                ));
                if !list {
                    self.settings.last_url = seeds[0].clone();
                }
                self.audit = Audit::default();
                self.url_index.clear();
                self.handle = Some(h);
                self.state = State::Crawling;
                self.crawled = 0;
                self.queued = 0;
                self.started = Some(Instant::now());
                self.elapsed = Duration::ZERO;
                self.selected = None;
                self.selected_check = None;
                self.suggestions.clear();
                self.opportunities.clear();
                self.rows_dirty = true;
                self.view = View::Overview;
            }
            Err(e) => self.error(e),
        }
    }

    fn stop(&mut self) {
        if let Some(h) = &self.handle {
            h.stop.store(true, Ordering::Relaxed);
            h.pause.store(false, Ordering::Relaxed);
            if self.state == State::Crawling || self.state == State::Paused {
                self.state = State::Stopped;
            }
        }
    }

    fn toggle_pause(&mut self) {
        if let Some(h) = &self.handle {
            let paused = !h.pause.load(Ordering::Relaxed);
            h.pause.store(paused, Ordering::Relaxed);
            self.state = if paused {
                State::Paused
            } else {
                State::Crawling
            };
        }
    }

    fn clear(&mut self) {
        self.stop();
        self.handle = None;
        drop_in_background(std::mem::take(&mut self.crawl));
        self.url_index.clear();
        drop_in_background(std::mem::take(&mut self.audit));
        self.state = State::Idle;
        self.selected = None;
        self.selected_check = None;
        self.suggestions.clear();
        self.opportunities.clear();
        self.rows_dirty = true;
        self.view = View::Overview;
    }

    fn run_audit(&mut self) {
        self.audit = audit::run(&self.crawl);
        self.audit_dirty = false;
        self.last_audit = Instant::now();
        self.rows_dirty = true;
    }

    fn pump_events(&mut self) {
        let mut finished = false;
        if let Some(h) = &self.handle {
            for _ in 0..5000 {
                match h.rx.try_recv() {
                    Ok(Event::Page(p)) => {
                        self.url_index.insert(p.url.clone(), self.crawl.pages.len());
                        self.crawl.pages.push(*p);
                        self.audit_dirty = true;
                    }
                    Ok(Event::Site(s)) => self.crawl.site = *s,
                    Ok(Event::Progress {
                        crawled, queued, ..
                    }) => {
                        self.crawled = crawled;
                        self.queued = queued;
                    }
                    Ok(Event::Log(_)) => {}
                    Ok(Event::Finished) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                }
            }
        }
        if let Some(t) = self.started {
            if matches!(self.state, State::Crawling | State::Paused) {
                self.elapsed = t.elapsed();
            }
        }
        if finished {
            self.handle = None;
            self.crawl.finished = chrono::Local::now().to_rfc3339();
            if self.state != State::Stopped {
                self.state = State::Done;
            }
            self.run_audit();
            let n = self.crawl.pages.len();
            self.toast(format!(
                "Crawl finished: {n} URLs audited against {} checks",
                catalogue().len()
            ));
        } else if self.audit_dirty
            && self.last_audit.elapsed() > Duration::from_secs(3)
            && self.crawl.pages.len() <= 8000
        {
            // ponytail: live audit only up to 8k URLs; bigger crawls audit once at the end to keep the UI smooth.
            self.run_audit();
        }
        if self.rows_dirty
            && (self.state != State::Crawling
                || self.last_rows.elapsed() > Duration::from_millis(600))
        {
            self.rebuild_rows();
        }
    }

    fn rebuild_rows(&mut self) {
        let pages = &self.crawl.pages;
        let q = self.search.to_lowercase();
        let base: Vec<usize> = match self.selected_check {
            Some(c) => self.audit.hits.get(c).cloned().unwrap_or_default(),
            None => (0..pages.len()).collect(),
        };
        let quick = if self.selected_check.is_some() {
            Quick::All
        } else {
            self.quick
        };
        let mut rows: Vec<usize> = base
            .into_iter()
            .filter(|&i| {
                let p = &pages[i];
                let ok = match quick {
                    Quick::All => true,
                    Quick::Internal => p.is_internal,
                    Quick::External => !p.is_internal,
                    Quick::Html => p.kind() == "HTML",
                    Quick::Images => p.kind() == "Image",
                    Quick::Css => p.kind() == "CSS",
                    Quick::Js => p.kind() == "JavaScript",
                    Quick::Non200 => p.status != 200,
                    Quick::Indexable => audit::indexable(p),
                    Quick::NonIndexable => {
                        p.is_internal && p.kind() == "HTML" && !audit::indexable(p)
                    }
                };
                ok && (q.is_empty()
                    || p.url.to_lowercase().contains(&q)
                    || p.title().to_lowercase().contains(&q))
            })
            .collect();
        let (col, asc) = self.sort;
        let inl = |i: usize| self.audit.inlinks.get(i).map(|v| v.len()).unwrap_or(0);
        let iss = |i: usize| self.audit.page_issues.get(i).map(|v| v.len()).unwrap_or(0);
        // Keys that cost more than a field read are computed once per row (this runs every
        // 600 ms during a crawl). Stable sorts with a reversed key keep the same tie order.
        match (col, asc) {
            (Col::Indexability, true) => {
                rows.sort_by_cached_key(|&i| indexability_label(&pages[i]))
            }
            (Col::Indexability, false) => {
                rows.sort_by_cached_key(|&i| std::cmp::Reverse(indexability_label(&pages[i])))
            }
            (Col::TitleLen, true) => rows.sort_by_cached_key(|&i| pages[i].title().chars().count()),
            (Col::TitleLen, false) => {
                rows.sort_by_cached_key(|&i| std::cmp::Reverse(pages[i].title().chars().count()))
            }
            _ => {}
        }
        rows.sort_by(|&a, &b| {
            let (pa, pb) = (&pages[a], &pages[b]);
            let o = match col {
                Col::Id => a.cmp(&b),
                Col::Url => pa.url.cmp(&pb.url),
                Col::Status => pa.status.cmp(&pb.status),
                Col::Indexability | Col::TitleLen => std::cmp::Ordering::Equal, // sorted above
                Col::Title => pa.title().cmp(pb.title()),
                Col::Words => pa.word_count().cmp(&pb.word_count()),
                Col::Depth => pa.depth.cmp(&pb.depth),
                Col::Inlinks => inl(a).cmp(&inl(b)),
                Col::Time => pa.response_ms.cmp(&pb.response_ms),
                Col::Size => pa.size.cmp(&pb.size),
                Col::Issues => iss(a).cmp(&iss(b)),
            };
            if asc {
                o
            } else {
                o.reverse()
            }
        });
        self.rows = rows;
        self.rows_dirty = false;
        self.last_rows = Instant::now();
    }

    fn select_check(&mut self, c: Option<usize>) {
        self.selected_check = c;
        self.view = View::Urls;
        self.rows_dirty = true;
    }

    fn save_file(&mut self, name: &str, ext: &str, data: impl FnOnce(&App) -> String) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter(ext.to_uppercase(), &[ext])
            .save_file()
        else {
            return;
        };
        let contents = data(self);
        match std::fs::write(&path, contents) {
            Ok(_) => self.toast(format!("Saved {}", path.display())),
            Err(e) => self.error(format!("Could not save {}: {e}", path.display())),
        }
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ShoutingToad crawl", &["shoutingtoad", "json"])
            .pick_file()
        else {
            return;
        };
        self.load_project(&path);
    }

    fn load_project(&mut self, path: &std::path::Path) {
        match Crawl::load(path) {
            Ok(c) => {
                self.clear();
                self.url_input = c.start_url.clone();
                self.crawl = c;
                self.url_index = self
                    .crawl
                    .pages
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.url.clone(), i))
                    .collect();
                self.state = State::Done;
                self.run_audit();
                if self.screenshot.is_none() {
                    self.toast(format!(
                        "Opened {} ({} URLs)",
                        path.display(),
                        self.crawl.pages.len()
                    ));
                }
            }
            Err(e) => self.error(e),
        }
    }

    fn save_project(&mut self) {
        let name = format!("{}.shoutingtoad", host_slug(&self.crawl.start_url));
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter("ShoutingToad crawl", &["shoutingtoad"])
            .save_file()
        else {
            return;
        };
        match self.crawl.save(&path) {
            Ok(_) => self.toast(format!("Saved crawl to {}", path.display())),
            Err(e) => self.error(e),
        }
    }

    fn export_all(&mut self) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let files: Vec<(&str, String)> = vec![
            (
                "report.html",
                report::html_report(
                    &self.crawl,
                    &self.audit,
                    &self.settings.client_name,
                    &self.settings.prepared_by,
                ),
            ),
            ("issues.csv", report::issues_csv(&self.crawl, &self.audit)),
            (
                "issue_urls.csv",
                report::issue_urls_csv(&self.crawl, &self.audit),
            ),
            (
                "internal.csv",
                report::internal_csv(&self.crawl, &self.audit),
            ),
            ("inlinks.csv", report::inlinks_csv(&self.crawl, &self.audit)),
            ("outlinks.csv", report::outlinks_csv(&self.crawl)),
            ("images.csv", report::images_csv(&self.crawl)),
            ("redirects.csv", report::redirects_csv(&self.crawl)),
        ];
        for (name, data) in files {
            if let Err(e) = std::fs::write(dir.join(name), data) {
                self.error(format!("Could not write {name}: {e}"));
                return;
            }
        }
        self.toast(format!("Exported 8 files to {}", dir.display()));
    }

    fn start_ai(&mut self) {
        let cands = ai::candidates(&self.crawl);
        if cands.is_empty() {
            self.toast("No indexable pages need new titles or descriptions");
            return;
        }
        let pages: Vec<(Page, String)> = cands
            .into_iter()
            .map(|(i, r)| (self.crawl.pages[i].clone(), r))
            .collect();
        let brand = ai::site_name(&self.crawl);
        let cfg = self.settings.ai.clone();
        let (tx, rx) = channel();
        self.ai_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.ai_cancel.clone();
        self.suggestions.clear();
        self.ai_progress = (0, pages.len());
        std::thread::spawn(move || {
            let total = pages.len();
            for (n, (p, reason)) in pages.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let s = ai::suggest(&cfg, &p, &reason, &brand);
                if tx.send(AiMsg::Item(Box::new(s))).is_err() {
                    return;
                }
                let _ = tx.send(AiMsg::Progress(n + 1, total));
            }
            let _ = tx.send(AiMsg::Done);
        });
        self.ai_rx = Some(rx);
    }

    fn pump_ai(&mut self) {
        let mut done = false;
        if let Some(rx) = &self.ai_rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    AiMsg::Progress(a, b) => self.ai_progress = (a, b),
                    AiMsg::Item(s) => self.suggestions.push(*s),
                    AiMsg::Done => done = true,
                }
            }
        }
        if done {
            self.ai_rx = None;
        }
        if let Some(rx) = &self.ai_test_rx {
            if let Ok(r) = rx.try_recv() {
                self.ai_test_result = Some(r);
                self.ai_test_rx = None;
            }
        }
    }

    fn icon(&self, ui: &mut egui::Ui, size: f32) {
        if let Some(t) = &self.icon {
            ui.add(
                egui::Image::new(t)
                    .fit_to_exact_size(egui::vec2(size, size))
                    .rounding(4.0),
            );
        }
    }
}

fn host_slug(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.replace('.', "-")))
        .unwrap_or_else(|| "crawl".into())
}

fn apply_theme(ctx: &egui::Context, t: ThemeChoice) {
    ctx.set_theme(match t {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
    });
}

fn fmt_bytes(b: usize) -> String {
    if b >= 1024 * 1024 {
        format!("{:.1} MB", b as f64 / 1048576.0)
    } else if b >= 1024 {
        format!("{:.0} KB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn truncate_px(s: &str, max_px: usize, font: f32) -> String {
    if pixel_width(s, font) <= max_px {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        out.push(ch);
        if pixel_width(&out, font) + pixel_width(" ...", font) > max_px {
            out.pop();
            break;
        }
    }
    format!("{} ...", out.trim_end())
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.18))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(6.0, 1.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().strong().color(color));
        });
}

fn kv(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    ui.label(RichText::new(k).strong());
    ui.add(egui::Label::new(v.into()).wrap());
    ui.end_row();
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_events();
        self.pump_ai();
        if matches!(self.state, State::Crawling | State::Paused)
            || self.ai_rx.is_some()
            || self.ai_test_rx.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        self.screenshot_tick(ctx);

        self.menu_bar(ctx);
        self.control_bar(ctx);
        self.status_bar(ctx);
        if self.has_data() {
            self.issues_panel(ctx);
            if self.view == View::Urls {
                self.inspector(ctx);
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            if !self.has_data() {
                self.welcome(ui);
            } else {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            self.view == View::Overview,
                            RichText::new("Overview").strong(),
                        )
                        .clicked()
                    {
                        self.view = View::Overview;
                    }
                    if ui
                        .selectable_label(self.view == View::Urls, RichText::new("URLs").strong())
                        .clicked()
                    {
                        self.view = View::Urls;
                    }
                });
                ui.separator();
                match self.view {
                    View::Overview => self.overview(ui),
                    View::Urls => self.url_table(ui),
                }
            }
        });
        self.dialogs(ctx);
    }
}

impl App {
    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(target) = self.screenshot.clone() else {
            return;
        };
        ctx.request_repaint();
        self.frames += 1;
        // Docs helper: SHOUTINGTOAD_SCREENSHOT_VIEW=urls shows the URL table with the first page inspected.
        if self.frames == 1
            && std::env::var("SHOUTINGTOAD_SCREENSHOT_VIEW").as_deref() == Ok("urls")
            && self.has_data()
        {
            self.view = View::Urls;
            self.selected = self.crawl.pages.iter().position(audit::ih);
            self.insp = Insp::Serp;
            self.rows_dirty = true;
        }
        if self.frames == 1 && std::env::var("SHOUTINGTOAD_SCREENSHOT_VIEW").as_deref() == Ok("ai")
        {
            self.show_ai_search = true;
        }
        if self.frames == 60 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
                .collect()
        });
        if let Some(img) = shots.first() {
            let _ = image::save_buffer(
                &target,
                img.as_raw(),
                img.width() as u32,
                img.height() as u32,
                image::ExtendedColorType::Rgba8,
            );
            std::process::exit(0);
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New crawl").clicked() {
                        self.clear();
                        ui.close_menu();
                    }
                    if ui.button("Open crawl…").clicked() {
                        ui.close_menu();
                        self.open_project();
                    }
                    if ui
                        .add_enabled(self.has_data(), egui::Button::new("Save crawl…"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.save_project();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Mode", |ui| {
                    if ui.button("Spider: crawl a whole site").clicked() {
                        ui.close_menu();
                    }
                    if ui.button("List: audit a list of URLs…").clicked() {
                        self.show_list = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Configuration", |ui| {
                    if ui.button("Spider settings…").clicked() {
                        self.show_config = true;
                        ui.close_menu();
                    }
                    if ui.button("AI provider & API key…").clicked() {
                        self.show_ai_settings = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(RichText::new("Theme").small().weak());
                    for (t, name) in [
                        (ThemeChoice::System, "Follow system"),
                        (ThemeChoice::Dark, "Dark"),
                        (ThemeChoice::Light, "Light"),
                    ] {
                        if ui.radio(self.settings.theme == t, name).clicked() {
                            self.settings.theme = t;
                            apply_theme(ctx, t);
                            ui.close_menu();
                        }
                    }
                });
                let has = self.has_data();
                ui.menu_button("Export", |ui| {
                    ui.add_enabled_ui(has, |ui| {
                        type Exporter = fn(&App) -> String;
                        let items: [(&str, &str, Exporter); 8] = [
                            ("All URLs (internal.csv)…", "internal.csv", |a| {
                                report::internal_csv(&a.crawl, &a.audit)
                            }),
                            ("Issues summary…", "issues.csv", |a| {
                                report::issues_csv(&a.crawl, &a.audit)
                            }),
                            ("Every issue with its URLs…", "issue_urls.csv", |a| {
                                report::issue_urls_csv(&a.crawl, &a.audit)
                            }),
                            ("All inlinks…", "inlinks.csv", |a| {
                                report::inlinks_csv(&a.crawl, &a.audit)
                            }),
                            ("All outlinks…", "outlinks.csv", |a| {
                                report::outlinks_csv(&a.crawl)
                            }),
                            ("All images…", "images.csv", |a| {
                                report::images_csv(&a.crawl)
                            }),
                            ("Redirect chains…", "redirects.csv", |a| {
                                report::redirects_csv(&a.crawl)
                            }),
                            ("Current table view…", "view.csv", |a| {
                                let mut c = Crawl {
                                    start_url: a.crawl.start_url.clone(),
                                    ..Default::default()
                                };
                                c.pages =
                                    a.rows.iter().map(|&i| a.crawl.pages[i].clone()).collect();
                                report::internal_csv(&c, &audit::run(&c))
                            }),
                        ];
                        for (label, file, f) in items {
                            if ui.button(label).clicked() {
                                ui.close_menu();
                                self.save_file(file, "csv", f);
                            }
                        }
                        ui.separator();
                        if ui.button("Everything to a folder…").clicked() {
                            ui.close_menu();
                            self.export_all();
                        }
                    });
                });
                ui.menu_button("Reports", |ui| {
                    if ui
                        .add_enabled(has, egui::Button::new("HTML audit report…"))
                        .clicked()
                    {
                        self.show_report = true;
                        ui.close_menu();
                    }
                    if ui.button("CI workflow (GitHub Actions)…").clicked() {
                        if self.ci_url.is_empty() {
                            self.ci_url = if self.crawl.start_url.is_empty() {
                                self.url_input.clone()
                            } else {
                                self.crawl.start_url.clone()
                            };
                        }
                        self.show_ci = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui
                        .add_enabled(has, egui::Button::new("Title & description writer…"))
                        .clicked()
                    {
                        self.show_ai_writer = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(has, egui::Button::new("Internal link finder…"))
                        .clicked()
                    {
                        self.opportunities = ai::link_opportunities(&self.crawl);
                        self.show_links = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(has, egui::Button::new("AI search readiness…"))
                        .clicked()
                    {
                        self.show_ai_search = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has && self.state != State::Crawling,
                            egui::Button::new("Re-run audit"),
                        )
                        .clicked()
                    {
                        self.run_audit();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has, egui::Button::new("Compare with a saved crawl…"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.compare_pick();
                    }
                    if ui
                        .add_enabled(has, egui::Button::new("Migration redirect mapper…"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.migrate_pick();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui
                        .button(format!("All {} checks…", catalogue().len()))
                        .clicked()
                    {
                        self.show_catalogue = true;
                        ui.close_menu();
                    }
                    ui.hyperlink_to("Documentation", report::REPO);
                    ui.hyperlink_to("Report a problem", format!("{}/issues", report::REPO));
                    ui.separator();
                    if ui.button("About ShoutingToad").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .weak(),
                    );
                    ui.label(RichText::new("ShoutingToad").strong().color(TOAD));
                    self.icon(ui, 16.0);
                });
            });
        });
    }

    fn control_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let busy = matches!(self.state, State::Crawling | State::Paused);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.url_input)
                        .hint_text("Enter a website URL, e.g. https://example.com")
                        .desired_width((ui.available_width() - 420.0).max(260.0)),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(RichText::new("▶  Start").strong().color(TOAD)),
                    )
                    .clicked()
                    || enter)
                    && !busy
                {
                    let u = self.url_input.clone();
                    self.start(vec![u], false);
                }
                let pause_label = if self.state == State::Paused {
                    "Resume"
                } else {
                    "Pause"
                };
                if ui
                    .add_enabled(busy, egui::Button::new(pause_label))
                    .clicked()
                {
                    self.toggle_pause();
                }
                if ui.add_enabled(busy, egui::Button::new("Stop")).clicked() {
                    self.stop();
                }
                if ui
                    .add_enabled(self.has_data() && !busy, egui::Button::new("Clear"))
                    .clicked()
                {
                    self.clear();
                }
                let total = (self.crawled + self.queued).max(1);
                let frac = match self.state {
                    State::Done => 1.0,
                    State::Idle => 0.0,
                    _ => self.crawled as f32 / total as f32,
                };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(120.0)
                        .show_percentage()
                        .animate(self.state == State::Crawling),
                );
            });
            ui.add_space(4.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (txt, col) = match self.state {
                    State::Idle => ("Ready", Color32::GRAY),
                    State::Crawling => ("Crawling", TOAD),
                    State::Paused => ("Paused", WARNING),
                    State::Done => ("Complete", GOOD),
                    State::Stopped => ("Stopped", WARNING),
                };
                dot(ui, col);
                ui.label(RichText::new(txt).strong());
                ui.separator();
                ui.label(format!("{} URLs", self.crawl.pages.len()));
                if matches!(self.state, State::Crawling | State::Paused) {
                    ui.separator();
                    ui.label(format!("{} queued", self.queued));
                    let secs = self.elapsed.as_secs_f32().max(0.1);
                    ui.separator();
                    ui.label(format!("{:.1} URLs/s", self.crawled as f32 / secs));
                    ui.separator();
                    ui.label(format!("{} threads", self.settings.cfg.threads));
                }
                if self.elapsed > Duration::ZERO {
                    ui.separator();
                    let s = self.elapsed.as_secs();
                    ui.label(format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60));
                }
                if self.has_data() {
                    ui.separator();
                    ui.label(format!("{} checks", catalogue().len()));
                }
                if let Some((msg, at, err)) = &self.toast {
                    if at.elapsed() < Duration::from_secs(6) {
                        ui.separator();
                        ui.label(RichText::new(msg).color(if *err { ISSUE } else { GOOD }));
                        ctx.request_repaint_after(Duration::from_millis(500));
                    }
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.18).max(20.0));
            self.icon(ui, 96.0);
            ui.add_space(8.0);
            ui.label(RichText::new("ShoutingToad SEO Spider").size(28.0).strong());
            ui.label(RichText::new(format!(
                "Crawl a website and audit it for over 300 SEO and UX issues ({} checks across {} categories).",
                catalogue().len(),
                CATEGORIES.len()
            )).weak());
            ui.add_space(18.0);
            ui.label("Type a URL in the box above and press Start, or:");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let w = 520.0;
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                if ui.button("Audit a list of URLs…").clicked() {
                    self.show_list = true;
                }
                if ui.button("Open a saved crawl…").clicked() {
                    self.open_project();
                }
                if ui.button("Spider settings…").clicked() {
                    self.show_config = true;
                }
                if ui.button("Browse all checks…").clicked() {
                    self.show_catalogue = true;
                }
            });
            ui.add_space(24.0);
            ui.label(RichText::new("Respects robots.txt by default · Free and open source under the AGPL-3.0").small().weak());
        });
    }

    fn issues_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("issues")
            .resizable(true)
            .default_width(330.0)
            .width_range(250.0..=520.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("Issues");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut self.show_all_checks, "All checks")
                            .on_hover_text("Also list checks that passed");
                    });
                });
                ui.horizontal(|ui| {
                    for (i, (k, name)) in [
                        (Kind::Issue, "Issues"),
                        (Kind::Warning, "Warnings"),
                        (Kind::Opportunity, "Opportunities"),
                    ]
                    .iter()
                    .enumerate()
                    {
                        let n = self
                            .audit
                            .triggered()
                            .filter(|(c, _)| catalogue()[*c].kind == *k)
                            .count();
                        let text =
                            RichText::new(format!("{name} {n}")).color(if self.show_kinds[i] {
                                kind_color(*k)
                            } else {
                                Color32::GRAY
                            });
                        if ui.selectable_label(self.show_kinds[i], text).clicked() {
                            self.show_kinds[i] = !self.show_kinds[i];
                        }
                    }
                });
                if self.audit_dirty
                    && self.state == State::Crawling
                    && self.crawl.pages.len() > 8000
                {
                    ui.label(
                        RichText::new("Large crawl: the audit updates when the crawl finishes.")
                            .small()
                            .weak(),
                    );
                }
                ui.separator();
                let cat = catalogue();
                let mut clicked: Option<usize> = None;
                egui::ScrollArea::vertical()
                    .id_salt("issues_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for category in CATEGORIES {
                            let mut items: Vec<(usize, usize)> = cat
                                .iter()
                                .enumerate()
                                .filter(|(_, c)| {
                                    c.category == *category && self.show_kinds[c.kind as usize]
                                })
                                .map(|(i, _)| {
                                    (i, self.audit.hits.get(i).map(|h| h.len()).unwrap_or(0))
                                })
                                .filter(|(_, n)| self.show_all_checks || *n > 0)
                                .collect();
                            if items.is_empty() {
                                continue;
                            }
                            items.sort_by_key(|(i, n)| {
                                (cat[*i].kind, cat[*i].priority, std::cmp::Reverse(*n))
                            });
                            let found = items.iter().filter(|(_, n)| *n > 0).count();
                            egui::CollapsingHeader::new(
                                RichText::new(format!("{category}  ({found})")).strong(),
                            )
                            .id_salt(category)
                            .default_open(found > 0)
                            .show(ui, |ui| {
                                for (i, n) in items {
                                    let c = &cat[i];
                                    ui.horizontal(|ui| {
                                        dot(
                                            ui,
                                            if n > 0 {
                                                kind_color(c.kind)
                                            } else {
                                                Color32::DARK_GRAY
                                            },
                                        );
                                        let sel = self.selected_check == Some(i);
                                        let label = RichText::new(&c.name).color(if n > 0 {
                                            ui.visuals().text_color()
                                        } else {
                                            Color32::GRAY
                                        });
                                        let r =
                                            ui.selectable_label(sel, label).on_hover_text(format!(
                                                "{} · {} priority\n\n{}",
                                                c.kind.label(),
                                                c.priority.label(),
                                                c.description
                                            ));
                                        if r.clicked() {
                                            clicked = Some(i);
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(n.to_string()).monospace().color(
                                                        if n > 0 {
                                                            kind_color(c.kind)
                                                        } else {
                                                            Color32::GRAY
                                                        },
                                                    ),
                                                );
                                            },
                                        );
                                    });
                                }
                            });
                        }
                    });
                if let Some(i) = clicked {
                    self.select_check(Some(i));
                }
            });
    }

    fn refresh_overview_stats(&mut self) {
        let pages = &self.crawl.pages;
        let key = (
            pages.len(),
            pages.as_ptr() as usize,
            self.crawl.started.clone(),
        );
        let st = &mut self.overview_stats;
        let fresh = st
            .at
            .is_some_and(|t| self.state == State::Crawling && t.elapsed() < Duration::from_secs(1));
        if st.key == key || fresh {
            return;
        }
        let mut new = OverviewStats {
            key,
            at: Some(Instant::now()),
            ..Default::default()
        };
        for p in pages {
            new.status[match p.status {
                200..=299 => 0,
                300..=399 => 1,
                400..=499 => 2,
                500..=599 => 3,
                0 => 4,
                _ => continue,
            }] += 1;
        }
        for p in pages {
            if audit::ih(p) {
                new.internal_html += 1;
                *new.depth.entry(p.depth.min(10)).or_insert(0) += 1;
            }
            if audit::indexable(p) {
                new.indexable += 1;
            }
            *new.kinds.entry(p.kind()).or_insert(0) += 1;
        }
        *st = new;
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        self.refresh_overview_stats();
        let cat = catalogue();
        let pages = &self.crawl.pages;
        let st = &self.overview_stats;
        let (internal_html, indexable) = (st.internal_html, st.indexable);
        let score = self.audit.health_score(&self.crawl);
        let counts = [Kind::Issue, Kind::Warning, Kind::Opportunity].map(|k| {
            self.audit
                .triggered()
                .filter(|(i, _)| cat[*i].kind == k)
                .count()
        });
        let mut jump: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("overview")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let sc = if score >= 85 {
                    GOOD
                } else if score >= 60 {
                    WARNING
                } else {
                    ISSUE
                };
                let text = ui.visuals().text_color();
                let cards: [(&str, String, Color32); 7] = [
                    ("Health score", format!("{score}/100"), sc),
                    ("URLs crawled", pages.len().to_string(), text),
                    ("Internal HTML pages", internal_html.to_string(), text),
                    ("Indexable pages", indexable.to_string(), text),
                    ("Issues", counts[0].to_string(), ISSUE),
                    ("Warnings", counts[1].to_string(), WARNING),
                    ("Opportunities", counts[2].to_string(), OPPORTUNITY),
                ];
                let per_row =
                    ((ui.available_width() / 170.0).floor() as usize).clamp(1, cards.len());
                for chunk in cards.chunks(per_row) {
                    ui.columns(per_row, |cols| {
                        for (col, (label, value, color)) in cols.iter_mut().zip(chunk) {
                            egui::Frame::group(col.style())
                                .rounding(8.0)
                                .inner_margin(12.0)
                                .show(col, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.label(RichText::new(*label).small().weak());
                                    ui.label(
                                        RichText::new(value.as_str())
                                            .size(26.0)
                                            .strong()
                                            .color(*color),
                                    );
                                });
                        }
                    });
                }
                ui.add_space(12.0);
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.heading("Top findings");
                    let mut list: Vec<(usize, &Vec<usize>)> = self.audit.triggered().collect();
                    list.sort_by_key(|(i, h)| {
                        (cat[*i].kind, cat[*i].priority, std::cmp::Reverse(h.len()))
                    });
                    if list.is_empty() {
                        ui.label(if self.state == State::Crawling {
                            "Collecting data…"
                        } else {
                            "No issues found."
                        });
                    }
                    for (i, hits) in list.iter().take(18) {
                        let c = &cat[*i];
                        ui.horizontal(|ui| {
                            badge(ui, c.kind.label(), kind_color(c.kind));
                            if ui.link(format!("{}: {}", c.category, c.name)).clicked() {
                                jump = Some(*i);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new(hits.len().to_string()).monospace());
                                },
                            );
                        });
                    }

                    let ui = &mut cols[1];
                    ui.heading("Response codes");
                    let buckets = [
                        ("2xx Success", GOOD),
                        ("3xx Redirect", OPPORTUNITY),
                        ("4xx Client error", ISSUE),
                        ("5xx Server error", ISSUE),
                    ];
                    let total = pages.len().max(1) as f32;
                    for (k, (label, col)) in buckets.into_iter().enumerate() {
                        let n = st.status[k];
                        bar(ui, label, n, n as f32 / total, col);
                    }
                    let none = st.status[4];
                    bar(
                        ui,
                        "No response / blocked",
                        none,
                        none as f32 / total,
                        Color32::GRAY,
                    );
                    ui.add_space(10.0);
                    ui.heading("Crawl depth (internal HTML)");
                    let max = st.depth.values().copied().max().unwrap_or(1) as f32;
                    for (&d, &n) in &st.depth {
                        bar(
                            ui,
                            &if d == 10 {
                                "10+".to_string()
                            } else {
                                d.to_string()
                            },
                            n,
                            n as f32 / max,
                            TOAD,
                        );
                    }
                    ui.add_space(10.0);
                    ui.heading("Content types");
                    for (&k, &n) in &st.kinds {
                        bar(ui, k, n, n as f32 / total, OPPORTUNITY);
                    }
                });
            });
        if let Some(i) = jump {
            self.select_check(Some(i));
        }
    }

    fn url_table(&mut self, ui: &mut egui::Ui) {
        if let Some(ci) = self.selected_check {
            let c = &catalogue()[ci];
            let mut clear = false;
            egui::Frame::group(ui.style()).rounding(6.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    badge(ui, c.kind.label(), kind_color(c.kind));
                    badge(
                        ui,
                        &format!("{} priority", c.priority.label()),
                        Color32::GRAY,
                    );
                    ui.label(RichText::new(format!("{}: {}", c.category, c.name)).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Show all URLs").clicked() {
                            clear = true;
                        }
                    });
                });
                ui.label(&c.description);
                ui.label(RichText::new(format!("How to fix: {}", c.fix)).italics());
            });
            if clear {
                self.selected_check = None;
                self.rows_dirty = true;
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                for (q, name) in QUICKS {
                    if ui.selectable_label(self.quick == *q, *name).clicked() {
                        self.quick = *q;
                        self.rows_dirty = true;
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Filter by URL or title")
                        .desired_width(280.0),
                )
                .changed()
            {
                self.rows_dirty = true;
            }
            ui.label(
                RichText::new(format!(
                    "{} of {} URLs",
                    self.rows.len(),
                    self.crawl.pages.len()
                ))
                .weak(),
            );
        });
        ui.add_space(2.0);

        let mut new_sort: Option<Col> = None;
        let mut new_sel: Option<usize> = None;
        let cols: [(Col, &str); 12] = [
            (Col::Id, "#"),
            (Col::Url, "Address"),
            (Col::Status, "Status"),
            (Col::Indexability, "Indexability"),
            (Col::Title, "Title"),
            (Col::TitleLen, "Len"),
            (Col::Words, "Words"),
            (Col::Depth, "Depth"),
            (Col::Inlinks, "Inlinks"),
            (Col::Time, "Time"),
            (Col::Size, "Size"),
            (Col::Issues, "Issues"),
        ];
        let (sort_col, asc) = self.sort;
        let rows = &self.rows;
        let pages = &self.crawl.pages;
        let audit = &self.audit;
        let selected = self.selected;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(40.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::exact(50.0))
            .column(Column::initial(100.0).clip(true))
            .column(Column::initial(220.0).at_least(80.0).clip(true))
            .column(Column::exact(36.0))
            .column(Column::exact(50.0))
            .column(Column::exact(44.0))
            .column(Column::exact(52.0))
            .column(Column::exact(62.0))
            .column(Column::exact(62.0))
            .column(Column::exact(46.0))
            .header(22.0, |mut h| {
                for (c, name) in cols {
                    h.col(|ui| {
                        if ui
                            .add(
                                egui::Label::new(RichText::new(name).strong())
                                    .sense(egui::Sense::click()),
                            )
                            .clicked()
                        {
                            new_sort = Some(c);
                        }
                        if sort_col == c {
                            sort_triangle(ui, asc);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let p = &pages[i];
                    row.set_selected(selected == Some(i));
                    row.col(|ui| {
                        ui.label(RichText::new((row_num(i)).to_string()).weak());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&p.url).truncate());
                    });
                    row.col(|ui| {
                        let txt = if p.status == 0 {
                            "—".to_string()
                        } else {
                            p.status.to_string()
                        };
                        ui.label(RichText::new(txt).strong().color(status_color(p.status)));
                    });
                    row.col(|ui| {
                        let l = indexability_label(p);
                        ui.label(RichText::new(l).color(if l == "Indexable" {
                            GOOD
                        } else {
                            Color32::GRAY
                        }));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(p.title()).truncate());
                    });
                    row.col(|ui| {
                        ui.label(p.title().chars().count().to_string());
                    });
                    row.col(|ui| {
                        ui.label(p.word_count().to_string());
                    });
                    row.col(|ui| {
                        ui.label(p.depth.to_string());
                    });
                    row.col(|ui| {
                        ui.label(
                            audit
                                .inlinks
                                .get(i)
                                .map(|v| v.len())
                                .unwrap_or(0)
                                .to_string(),
                        );
                    });
                    row.col(|ui| {
                        ui.label(format!("{} ms", p.response_ms));
                    });
                    row.col(|ui| {
                        ui.label(fmt_bytes(p.size));
                    });
                    row.col(|ui| {
                        let n = audit.page_issues.get(i).map(|v| v.len()).unwrap_or(0);
                        ui.label(RichText::new(n.to_string()).color(if n > 0 {
                            WARNING
                        } else {
                            Color32::GRAY
                        }));
                    });
                    if row.response().clicked() {
                        new_sel = Some(i);
                    }
                });
            });
        if let Some(c) = new_sort {
            self.sort = if self.sort.0 == c {
                (c, !self.sort.1)
            } else {
                (c, true)
            };
            self.rows_dirty = true;
        }
        if let Some(i) = new_sel {
            self.selected = Some(i);
        }
    }

    fn inspector(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("inspector").resizable(true).default_height(260.0).height_range(140.0..=600.0).show(ctx, |ui| {
            let Some(i) = self.selected.filter(|&i| i < self.crawl.pages.len()) else {
                ui.add_space(8.0);
                ui.label(RichText::new("Select a URL above to inspect it.").weak());
                return;
            };
            let p = &self.crawl.pages[i];
            let h = p.html.as_deref();
            let ins = self.audit.inlinks.get(i).map(|v| v.len()).unwrap_or(0);
            let tabs = [
                (Insp::Details, "Details".to_string()),
                (Insp::Issues, format!("Issues ({})", self.audit.page_issues.get(i).map(|v| v.len()).unwrap_or(0))),
                (Insp::Inlinks, format!("Inlinks ({ins})")),
                (Insp::Outlinks, format!("Outlinks ({})", h.map(|h| h.links.len()).unwrap_or(0))),
                (Insp::Images, format!("Images ({})", h.map(|h| h.images.len()).unwrap_or(0))),
                (Insp::Headers, "HTTP Headers".to_string()),
                (Insp::Serp, "SERP Snippet".to_string()),
                (Insp::Schema, format!("Structured Data ({})", h.map(|h| h.json_ld.len()).unwrap_or(0))),
            ];
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (t, name) in tabs {
                    if ui.selectable_label(self.insp == t, name).clicked() {
                        self.insp = t;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Open in browser").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&p.url));
                    }
                    if ui.small_button("Copy URL").clicked() {
                        ui.ctx().copy_text(p.url.clone());
                    }
                });
            });
            ui.separator();
            let cat = catalogue();
            let mut jump: Option<usize> = None;
            egui::ScrollArea::both().id_salt("insp").auto_shrink([false, false]).show(ui, |ui| match self.insp {
                Insp::Details => {
                    egui::Grid::new("details").num_columns(2).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                        kv(ui, "Address", &p.url);
                        kv(ui, "Status", if p.status == 0 { p.status_text.clone() } else { format!("{} {}", p.status, p.status_text) });
                        kv(ui, "Indexability", indexability_label(p));
                        kv(ui, "Content type", &p.content_type);
                        if let Some(r) = &p.redirect_to {
                            kv(ui, "Redirects to", r);
                        }
                        if let Some(h) = h {
                            kv(ui, "Title", format!("{}  ({} chars, {} px)", p.title(), p.title().chars().count(), pixel_width(p.title(), 20.0)));
                            kv(ui, "Meta description", format!("{}  ({} chars)", p.description(), p.description().chars().count()));
                            kv(ui, "H1", h.h1.join(" | "));
                            kv(ui, "H2", h.h2.iter().take(5).cloned().collect::<Vec<_>>().join(" | "));
                            kv(ui, "Canonical", h.canonicals.join(", "));
                            kv(ui, "Meta robots", h.meta_robots.join(", "));
                            kv(ui, "Language", h.lang.clone().unwrap_or_default());
                            kv(ui, "Word count", format!("{} words · Flesch {:.0}", h.word_count, h.flesch));
                            kv(ui, "Hreflang", h.hreflang.iter().map(|(l, u)| format!("{l} → {u}")).collect::<Vec<_>>().join("\n"));
                        }
                        kv(ui, "Crawl depth", p.depth.to_string());
                        kv(ui, "Found via", p.source.map(|s| format!("{s:?}")).unwrap_or_default());
                        kv(ui, "Response time", format!("{} ms", p.response_ms));
                        kv(ui, "Size", format!("{} ({} transferred, {})", fmt_bytes(p.size), fmt_bytes(p.transfer_size), if p.content_encoding.is_empty() { "uncompressed" } else { p.content_encoding.as_str() }));
                        kv(ui, "HTTP version", &p.http_version);
                        if p.custom_search_hits > 0 || !p.custom_extract.is_empty() {
                            kv(ui, "Custom search hits", p.custom_search_hits.to_string());
                            kv(ui, "Custom extraction", p.custom_extract.join(" | "));
                        }
                    });
                }
                Insp::Issues => {
                    let list = self.audit.page_issues.get(i).cloned().unwrap_or_default();
                    if list.is_empty() {
                        ui.label(RichText::new("No issues found on this URL.").color(GOOD));
                    }
                    let mut list = list;
                    list.sort_by_key(|&c| (cat[c].kind, cat[c].priority));
                    for c in list {
                        let k = &cat[c];
                        ui.horizontal(|ui| {
                            badge(ui, k.kind.label(), kind_color(k.kind));
                            if ui.link(RichText::new(format!("{}: {}", k.category, k.name)).strong()).clicked() {
                                jump = Some(c);
                            }
                        });
                        ui.label(RichText::new(&k.fix).weak());
                        ui.add_space(3.0);
                    }
                }
                Insp::Inlinks => {
                    egui::Grid::new("inl").striped(true).num_columns(3).spacing([16.0, 3.0]).show(ui, |ui| {
                        ui.label(RichText::new("From").strong());
                        ui.label(RichText::new("Anchor text").strong());
                        ui.label(RichText::new("Follow").strong());
                        ui.end_row();
                        for &(s, l) in self.audit.inlinks.get(i).map(|v| v.as_slice()).unwrap_or(&[]).iter().take(2000) {
                            let src = &self.crawl.pages[s];
                            if let Some(link) = src.html.as_ref().and_then(|h| h.links.get(l)) {
                                ui.label(&src.url);
                                ui.label(&link.anchor);
                                ui.label(if link.nofollow { "nofollow" } else { "follow" });
                                ui.end_row();
                            }
                        }
                    });
                }
                Insp::Outlinks => {
                    if let Some(h) = h {
                        egui::Grid::new("outl").striped(true).num_columns(4).spacing([16.0, 3.0]).show(ui, |ui| {
                            for t in ["To", "Status", "Anchor text", "Rel"] {
                                ui.label(RichText::new(t).strong());
                            }
                            ui.end_row();
                            for l in h.links.iter().take(3000) {
                                ui.label(&l.url);
                                let st = self.url_index.get(&l.url).map(|&x| self.crawl.pages[x].status);
                                match st {
                                    Some(s) => ui.label(RichText::new(s.to_string()).color(status_color(s))),
                                    None => ui.label("—"),
                                };
                                ui.label(&l.anchor);
                                ui.label(if l.nofollow { "nofollow" } else { "" });
                                ui.end_row();
                            }
                        });
                    }
                }
                Insp::Images => {
                    if let Some(h) = h {
                        egui::Grid::new("imgs").striped(true).num_columns(4).spacing([16.0, 3.0]).show(ui, |ui| {
                            for t in ["Image", "Alt text", "Size", "Dimensions set"] {
                                ui.label(RichText::new(t).strong());
                            }
                            ui.end_row();
                            for img in &h.images {
                                ui.label(&img.url);
                                match &img.alt {
                                    None => ui.label(RichText::new("(missing alt attribute)").color(ISSUE)),
                                    Some(a) if a.is_empty() => ui.label(RichText::new("(empty)").weak()),
                                    Some(a) => ui.label(a),
                                };
                                let t = self.url_index.get(&img.url).map(|&x| &self.crawl.pages[x]);
                                ui.label(t.map(|t| fmt_bytes(t.size)).unwrap_or_default());
                                ui.label(if img.has_width && img.has_height { "yes" } else { "no" });
                                ui.end_row();
                            }
                        });
                    }
                }
                Insp::Headers => {
                    egui::Grid::new("hdrs").striped(true).num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
                        for (k, v) in &p.headers {
                            ui.label(RichText::new(k).monospace().strong());
                            ui.add(egui::Label::new(RichText::new(v).monospace()).wrap());
                            ui.end_row();
                        }
                    });
                }
                Insp::Serp => {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(!self.mobile_serp, "Desktop").clicked() {
                            self.mobile_serp = false;
                        }
                        if ui.selectable_label(self.mobile_serp, "Mobile").clicked() {
                            self.mobile_serp = true;
                        }
                    });
                    let (title_px, desc_px, width) = if self.mobile_serp { (500, 680, 380.0) } else { (580, 990, 600.0) };
                    let title = if p.title().is_empty() { "(no title)" } else { p.title() };
                    let desc = if p.description().is_empty() { "No meta description, so search engines will choose text from the page." } else { p.description() };
                    egui::Frame::none().fill(Color32::WHITE).rounding(8.0).inner_margin(14.0).stroke(egui::Stroke::new(1.0_f32, Color32::from_gray(220))).show(ui, |ui| {
                        ui.set_max_width(width);
                        ui.label(RichText::new(&p.url).size(12.0).color(Color32::from_rgb(32, 33, 36)));
                        ui.label(RichText::new(truncate_px(title, title_px, 20.0)).size(19.0).color(Color32::from_rgb(26, 13, 171)));
                        ui.label(RichText::new(truncate_px(desc, desc_px, 14.0)).size(13.5).color(Color32::from_rgb(77, 81, 86)));
                    });
                    ui.label(format!(
                        "Title: {} chars, {} of {title_px} px · Description: {} chars, {} of {desc_px} px",
                        p.title().chars().count(), pixel_width(p.title(), 20.0), p.description().chars().count(), pixel_width(p.description(), 14.0)
                    ));
                }
                Insp::Schema => {
                    if let Some(h) = h {
                        for e in &h.json_ld_errors {
                            ui.label(RichText::new(format!("Parse error: {e}")).color(ISSUE));
                        }
                        for v in &h.json_ld {
                            let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
                            ui.add(egui::Label::new(RichText::new(pretty).monospace().small()));
                            ui.separator();
                        }
                        if h.json_ld.is_empty() && h.json_ld_errors.is_empty() {
                            ui.label(if h.microdata || h.rdfa { "Microdata or RDFa found (no JSON-LD)." } else { "No structured data on this page." });
                        }
                    }
                }
            });
            if let Some(c) = jump {
                self.select_check(Some(c));
            }
        });
    }
}

fn sort_triangle(ui: &mut egui::Ui, ascending: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let c = rect.center();
    let pts = if ascending {
        vec![
            egui::pos2(c.x - 4.0, c.y + 3.0),
            egui::pos2(c.x + 4.0, c.y + 3.0),
            egui::pos2(c.x, c.y - 3.0),
        ]
    } else {
        vec![
            egui::pos2(c.x - 4.0, c.y - 3.0),
            egui::pos2(c.x + 4.0, c.y - 3.0),
            egui::pos2(c.x, c.y + 3.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        pts,
        ui.visuals().text_color(),
        egui::Stroke::NONE,
    ));
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn row_num(i: usize) -> usize {
    i + 1
}

fn bar(ui: &mut egui::Ui, label: &str, n: usize, frac: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(150.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(150.0);
                ui.add(egui::Label::new(label).truncate());
            },
        );
        let w = (ui.available_width() - 60.0).max(60.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 12.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().faint_bg_color);
        let mut fill = rect;
        fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
        ui.painter().rect_filled(fill, 3.0, color);
        ui.label(RichText::new(n.to_string()).monospace());
    });
}

// ---------------------------------------------------------------- dialogs
impl App {
    fn dialogs(&mut self, ctx: &egui::Context) {
        self.config_dialog(ctx);
        self.list_dialog(ctx);
        self.ai_settings_dialog(ctx);
        self.ai_writer_dialog(ctx);
        self.links_dialog(ctx);
        self.report_dialog(ctx);
        self.ci_dialog(ctx);
        self.catalogue_dialog(ctx);
        self.about_dialog(ctx);
        self.compare_dialog(ctx);
        self.migrate_dialog(ctx);
        self.ai_search_dialog(ctx);
    }

    fn config_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_config;
        egui::Window::new("Spider settings").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            let c = &mut self.settings.cfg;
            egui::Grid::new("cfg").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Max URLs");
                ui.add(egui::DragValue::new(&mut c.max_urls).range(1..=5_000_000).speed(50));
                ui.end_row();
                ui.label("Max crawl depth");
                ui.add(egui::DragValue::new(&mut c.max_depth).range(0..=100));
                ui.end_row();
                ui.label("Threads");
                ui.add(egui::DragValue::new(&mut c.threads).range(1..=64));
                ui.end_row();
                ui.label("Delay between requests (ms)");
                ui.add(egui::DragValue::new(&mut c.delay_ms).range(0..=60_000).speed(10));
                ui.end_row();
                ui.label("Request timeout (s)");
                ui.add(egui::DragValue::new(&mut c.timeout_secs).range(1..=300));
                ui.end_row();
            });
            ui.separator();
            ui.checkbox(&mut c.respect_robots, "Respect robots.txt");
            ui.checkbox(&mut c.crawl_sitemaps, "Crawl URLs found in XML sitemaps (finds orphan pages)");
            ui.checkbox(&mut c.check_external, "Check external links");
            ui.checkbox(&mut c.check_resources, "Check images, CSS and JavaScript");
            ui.checkbox(&mut c.crawl_subdomains, "Treat subdomains as internal");
            ui.checkbox(&mut c.follow_nofollow, "Follow nofollow links");
            ui.checkbox(&mut c.ai_bots, "Test AI crawler access after the crawl (up to about 90 extra requests)");
            ui.separator();
            ui.label(RichText::new("User agent").strong());
            egui::ComboBox::from_id_salt("ua").selected_text("Presets").show_ui(ui, |ui| {
                let presets = [
                    ("ShoutingToad (default)", DEFAULT_UA.to_string()),
                    ("Googlebot Smartphone", "Mozilla/5.0 (Linux; Android 6.0.1; Nexus 5X Build/MMB29P) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)".to_string()),
                    ("Googlebot Desktop", "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)".to_string()),
                    ("Bingbot", "Mozilla/5.0 (compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm)".to_string()),
                    ("Chrome (desktop browser)", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36".to_string()),
                ];
                for (name, ua) in presets {
                    if ui.selectable_label(c.user_agent == ua, name).clicked() {
                        c.user_agent = ua;
                    }
                }
            });
            ui.add(egui::TextEdit::singleline(&mut c.user_agent).desired_width(520.0));
            ui.separator();
            ui.label(RichText::new("Include / exclude (regular expressions on internal URLs)").strong());
            ui.add(egui::TextEdit::singleline(&mut c.include).hint_text("Only crawl URLs matching, e.g. /blog/").desired_width(520.0));
            ui.add(egui::TextEdit::singleline(&mut c.exclude).hint_text("Skip URLs matching, e.g. \\?sort=|/tag/").desired_width(520.0));
            ui.separator();
            ui.label(RichText::new("Custom search & extraction (regular expressions on the HTML)").strong());
            ui.add(egui::TextEdit::singleline(&mut c.custom_search).hint_text("Count matches, e.g. GTM-[A-Z0-9]+").desired_width(520.0));
            ui.add(egui::TextEdit::singleline(&mut c.custom_extract).hint_text("Extract group 1, e.g. \"sku\":\\s*\"([^\"]+)\"").desired_width(520.0));
            ui.add_space(6.0);
            if ui.button("Reset to defaults").clicked() {
                *c = Config::default();
            }
        });
        self.show_config = open;
    }

    fn list_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_list;
        let mut go = false;
        egui::Window::new("List mode: audit specific URLs")
            .open(&mut open)
            .default_size([560.0, 360.0])
            .show(ctx, |ui| {
                ui.label(
                    "Paste one URL per line. Only these URLs are audited; links are not followed.",
                );
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.settings.list_text)
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(12)
                                .desired_width(f32::INFINITY)
                                .hint_text("https://example.com/\nhttps://example.com/about"),
                        );
                    });
                let n = self
                    .settings
                    .list_text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count();
                if ui
                    .add_enabled(n > 0, egui::Button::new(format!("Audit {n} URLs")))
                    .clicked()
                {
                    go = true;
                }
            });
        self.show_list = open;
        if go {
            let urls: Vec<String> = self
                .settings
                .list_text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            self.show_list = false;
            self.start(urls, true);
        }
    }

    fn ai_settings_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_ai_settings;
        let mut test = false;
        egui::Window::new("AI provider").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            let a = &mut self.settings.ai;
            ui.label("Used by the title & description writer. Bring your own API key; ShoutingToad sends page text only to the provider you choose.");
            ui.add_space(4.0);
            egui::ComboBox::from_id_salt("provider").selected_text(a.provider.label()).width(380.0).show_ui(ui, |ui| {
                for p in [Provider::Offline, Provider::Anthropic, Provider::OpenAiCompatible, Provider::Gemini] {
                    if ui.selectable_label(a.provider == p, p.label()).clicked() && a.provider != p {
                        a.provider = p;
                        a.model = p.default_model().to_string();
                        a.api_key.clear();
                        a.load_env_key();
                    }
                }
            });
            if a.provider != Provider::Offline {
                egui::Grid::new("ai").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Model");
                    ui.add(egui::TextEdit::singleline(&mut a.model).desired_width(300.0));
                    ui.end_row();
                    if a.provider == Provider::OpenAiCompatible {
                        ui.label("Base URL");
                        ui.add(egui::TextEdit::singleline(&mut a.base_url).desired_width(300.0).hint_text("http://localhost:11434/v1 for Ollama"));
                        ui.end_row();
                    }
                    ui.label("API key");
                    ui.add(egui::TextEdit::singleline(&mut a.api_key).password(true).desired_width(300.0));
                    ui.end_row();
                });
                ui.label(RichText::new("The key stays in memory and is never saved to disk. To avoid retyping it, set ANTHROPIC_API_KEY, OPENAI_API_KEY or GEMINI_API_KEY before starting the app.").small().weak());
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.ai_test_rx.is_none() && a.ready(), egui::Button::new("Test connection")).clicked() {
                        test = true;
                    }
                    if self.ai_test_rx.is_some() {
                        ui.spinner();
                    }
                    match &self.ai_test_result {
                        Some(Ok(m)) => {
                            ui.label(RichText::new(format!("Connected: {m}")).color(GOOD));
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(e).color(ISSUE));
                        }
                        None => {}
                    }
                });
            } else {
                ui.label(RichText::new("Offline rules rewrite titles and descriptions from each page's headings and opening text. No data leaves your computer.").weak());
            }
        });
        self.show_ai_settings = open;
        if test {
            let cfg = self.settings.ai.clone();
            let (tx, rx) = channel();
            self.ai_test_result = None;
            self.ai_test_rx = Some(rx);
            std::thread::spawn(move || {
                let r = ai::complete(&cfg, "Reply with the single word OK.", "Say OK.")
                    .map(|t| t.trim().chars().take(40).collect());
                let _ = tx.send(r);
            });
        }
    }

    fn ai_writer_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_ai_writer;
        let mut start = false;
        let mut export = false;
        egui::Window::new("Title & description writer")
            .open(&mut open)
            .default_size([980.0, 560.0])
            .show(ctx, |ui| {
                let n = ai::candidates(&self.crawl).len();
                let engine = if self.settings.ai.provider == Provider::Offline
                    || !self.settings.ai.ready()
                {
                    "Offline rules".to_string()
                } else {
                    format!(
                        "{} ({})",
                        self.settings.ai.provider.label(),
                        self.settings.ai.model
                    )
                };
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{n} indexable pages have a missing, short or long title or description."
                    ));
                    ui.label(RichText::new(format!("Engine: {engine}")).weak());
                    if ui.link("Change…").clicked() {
                        self.show_ai_settings = true;
                    }
                });
                ui.horizontal(|ui| {
                    let running = self.ai_rx.is_some();
                    if ui
                        .add_enabled(
                            !running && n > 0,
                            egui::Button::new(RichText::new("Write suggestions").strong()),
                        )
                        .clicked()
                    {
                        start = true;
                    }
                    if running {
                        if ui.button("Cancel").clicked() {
                            self.ai_cancel.store(true, Ordering::Relaxed);
                        }
                        let (a, b) = self.ai_progress;
                        ui.add(
                            egui::ProgressBar::new(a as f32 / b.max(1) as f32)
                                .text(format!("{a} / {b}"))
                                .desired_width(200.0),
                        );
                    }
                    if ui
                        .add_enabled(
                            !self.suggestions.is_empty(),
                            egui::Button::new("Export CSV…"),
                        )
                        .clicked()
                    {
                        export = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("sugg")
                            .striped(true)
                            .num_columns(4)
                            .spacing([14.0, 8.0])
                            .show(ui, |ui| {
                                for t in ["Page", "Suggested title", "Suggested description", ""] {
                                    ui.label(RichText::new(t).strong());
                                }
                                ui.end_row();
                                for s in &self.suggestions {
                                    ui.vertical(|ui| {
                                        ui.set_max_width(260.0);
                                        ui.add(
                                            egui::Label::new(RichText::new(&s.url).small()).wrap(),
                                        );
                                        ui.label(RichText::new(&s.reason).small().color(WARNING));
                                        if let Some(e) = &s.error {
                                            ui.label(
                                                RichText::new(format!("AI error: {e}"))
                                                    .small()
                                                    .color(ISSUE),
                                            );
                                        }
                                    });
                                    ui.vertical(|ui| {
                                        ui.set_max_width(280.0);
                                        ui.add(
                                            egui::Label::new(RichText::new(&s.title).strong())
                                                .wrap(),
                                        );
                                        let n = s.title.chars().count();
                                        ui.label(
                                            RichText::new(format!(
                                                "{n} chars · {} px",
                                                pixel_width(&s.title, 20.0)
                                            ))
                                            .small()
                                            .color(if n <= 60 { GOOD } else { WARNING }),
                                        );
                                    });
                                    ui.vertical(|ui| {
                                        ui.set_max_width(360.0);
                                        ui.add(egui::Label::new(&s.description).wrap());
                                        let n = s.description.chars().count();
                                        ui.label(
                                            RichText::new(format!("{n} chars · {}", s.source))
                                                .small()
                                                .color(if (70..=160).contains(&n) {
                                                    GOOD
                                                } else {
                                                    WARNING
                                                }),
                                        );
                                    });
                                    ui.vertical(|ui| {
                                        if ui.small_button("Copy title").clicked() {
                                            ui.ctx().copy_text(s.title.clone());
                                        }
                                        if ui.small_button("Copy description").clicked() {
                                            ui.ctx().copy_text(s.description.clone());
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    });
            });
        self.show_ai_writer = open;
        if start {
            self.start_ai();
        }
        if export {
            self.save_file("title_description_suggestions.csv", "csv", |a| {
                report::suggestions_csv(&a.suggestions)
            });
        }
    }

    fn links_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_links;
        let mut export = false;
        egui::Window::new("Internal link finder").open(&mut open).default_size([980.0, 520.0]).show(ctx, |ui| {
            ui.label("Pages that mention another page's topic (its H1 or title) in their text but do not link to it yet. Adding these contextual links helps visitors and spreads ranking signals.");
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} opportunities", self.opportunities.len())).strong().color(GOOD));
                if ui.button("Scan again").clicked() {
                    self.opportunities = ai::link_opportunities(&self.crawl);
                }
                if ui.add_enabled(!self.opportunities.is_empty(), egui::Button::new("Export CSV…")).clicked() {
                    export = true;
                }
            });
            ui.separator();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("opps").striped(true).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                    for t in ["On page", "Phrase", "Link to", "Context"] {
                        ui.label(RichText::new(t).strong());
                    }
                    ui.end_row();
                    for o in self.opportunities.iter().take(3000) {
                        ui.add(egui::Label::new(RichText::new(&o.source_url).small()).wrap());
                        ui.label(RichText::new(format!("{} ({}×)", o.phrase, o.mentions)).strong().color(WARNING));
                        ui.vertical(|ui| {
                            ui.set_max_width(280.0);
                            ui.add(egui::Label::new(&o.target_title).wrap());
                            ui.add(egui::Label::new(RichText::new(&o.target_url).small().weak()).wrap());
                        });
                        ui.vertical(|ui| {
                            ui.set_max_width(380.0);
                            ui.add(egui::Label::new(RichText::new(&o.context).small()).wrap());
                        });
                        ui.end_row();
                    }
                });
            });
        });
        self.show_links = open;
        if export {
            self.save_file("internal_link_opportunities.csv", "csv", |a| {
                report::link_opportunities_csv(&a.opportunities)
            });
        }
    }

    fn report_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_report;
        let mut save = false;
        egui::Window::new("HTML audit report").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.label("A self-contained report with the health score, every finding, how to fix it and the affected URLs. Open it in a browser and print to PDF to share.");
            egui::Grid::new("rep").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Client or site name");
                ui.add(egui::TextEdit::singleline(&mut self.settings.client_name).desired_width(280.0).hint_text("Optional"));
                ui.end_row();
                ui.label("Prepared by");
                ui.add(egui::TextEdit::singleline(&mut self.settings.prepared_by).desired_width(280.0).hint_text("Optional, e.g. your agency"));
                ui.end_row();
            });
            if ui.button(RichText::new("Save report…").strong()).clicked() {
                save = true;
            }
        });
        self.show_report = open;
        if save {
            let name = format!("seo-audit-{}.html", host_slug(&self.crawl.start_url));
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(name)
                .add_filter("HTML", &["html"])
                .save_file()
            {
                let html = report::html_report(
                    &self.crawl,
                    &self.audit,
                    &self.settings.client_name,
                    &self.settings.prepared_by,
                );
                match std::fs::write(&path, html) {
                    Ok(_) => {
                        self.toast(format!("Saved {}", path.display()));
                        if let Ok(u) = url::Url::from_file_path(&path) {
                            ctx.open_url(egui::OpenUrl::new_tab(u.to_string()));
                        }
                    }
                    Err(e) => self.error(format!("Could not save: {e}")),
                }
            }
        }
    }

    fn ci_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_ci;
        let mut save = false;
        egui::Window::new("CI workflow").open(&mut open).default_size([700.0, 520.0]).show(ctx, |ui| {
            ui.label("Run ShoutingToad headless in GitHub Actions on every push and weekly. The job fails when new issues at or above the chosen priority appear, and uploads the full report.");
            ui.horizontal(|ui| {
                ui.label("Site URL");
                ui.add(egui::TextEdit::singleline(&mut self.ci_url).desired_width(360.0));
                ui.label("Fail on");
                egui::ComboBox::from_id_salt("lvl").selected_text(self.ci_level.label()).show_ui(ui, |ui| {
                    for p in [Priority::High, Priority::Medium, Priority::Low] {
                        ui.selectable_value(&mut self.ci_level, p, format!("{} priority issues", p.label()));
                    }
                });
            });
            ui.checkbox(&mut self.compare_ui.ci_baseline, "Compare with the previous run: fail only on new issues, add a job summary and badge");
            let yaml = report::ci_workflow(self.ci_url.trim(), self.ci_level, self.compare_ui.ci_baseline);
            egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut yaml.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(yaml.clone());
                }
                if ui.button("Save as .github/workflows/seo-audit.yml…").clicked() {
                    save = true;
                }
            });
        });
        self.show_ci = open;
        if save {
            let yaml = report::ci_workflow(
                self.ci_url.trim(),
                self.ci_level,
                self.compare_ui.ci_baseline,
            );
            self.save_file("seo-audit.yml", "yml", move |_| yaml);
        }
    }

    fn catalogue_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_catalogue;
        let cat = catalogue();
        egui::Window::new(format!("All {} checks", cat.len()))
            .open(&mut open)
            .default_size([900.0, 560.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.catalogue_search)
                            .hint_text("Search checks")
                            .desired_width(300.0),
                    );
                    ui.label(RichText::new(format!("{} categories", CATEGORIES.len())).weak());
                });
                ui.separator();
                let q = self.catalogue_search.to_lowercase();
                let list: Vec<usize> = (0..cat.len())
                    .filter(|&i| {
                        q.is_empty()
                            || cat[i].name.to_lowercase().contains(&q)
                            || cat[i].category.to_lowercase().contains(&q)
                            || cat[i].id.contains(&q)
                    })
                    .collect();
                ui.columns(2, |cols| {
                    egui::ScrollArea::vertical()
                        .id_salt("catlist")
                        .auto_shrink([false, false])
                        .show(&mut cols[0], |ui| {
                            for i in list {
                                let c = &cat[i];
                                ui.horizontal(|ui| {
                                    dot(ui, kind_color(c.kind));
                                    if ui
                                        .selectable_label(
                                            self.catalogue_selected == Some(i),
                                            format!("{}: {}", c.category, c.name),
                                        )
                                        .clicked()
                                    {
                                        self.catalogue_selected = Some(i);
                                    }
                                });
                            }
                        });
                    let ui = &mut cols[1];
                    if let Some(i) = self.catalogue_selected {
                        let c = &cat[i];
                        ui.heading(&c.name);
                        ui.horizontal(|ui| {
                            badge(ui, c.kind.label(), kind_color(c.kind));
                            badge(
                                ui,
                                &format!("{} priority", c.priority.label()),
                                Color32::GRAY,
                            );
                            ui.label(RichText::new(c.category).weak());
                        });
                        ui.label(RichText::new(&c.id).monospace().small().weak());
                        ui.add_space(6.0);
                        ui.label(&c.description);
                        ui.add_space(6.0);
                        ui.label(RichText::new("How to fix").strong());
                        ui.label(&c.fix);
                    } else {
                        ui.label(
                            RichText::new(
                                "Select a check to see what it looks for and how to fix it.",
                            )
                            .weak(),
                        );
                    }
                });
            });
        self.show_catalogue = open;
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About ShoutingToad").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.icon(ui, 64.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("ShoutingToad SEO Spider").size(20.0).strong());
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label(RichText::new(format!("{} checks across {} categories", catalogue().len(), CATEGORIES.len())).weak());
                });
            });
            ui.add_space(8.0);
            ui.label("A website crawler that audits your site for over 300 SEO issues to help improve both SEO and UX, and more.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Made by");
                ui.hyperlink_to("N&D Co.", "https://ndcompany.in");
            });
            ui.horizontal(|ui| {
                ui.label("Source code:");
                ui.hyperlink_to("github.com/NDcompany/ShoutingToad", report::REPO);
            });
            ui.label(RichText::new("Free software under the GNU Affero General Public License v3.0 or later. It comes with no warranty.").small().weak());
        });
        self.show_about = open;
    }
}
