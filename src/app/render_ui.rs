//! JavaScript rendering: settings, and the "Raw vs rendered" inspector tab.

use super::{App, GOOD, ISSUE, WARNING};
use crate::crawler::Config;
use crate::model::Page;
use eframe::egui::{self, RichText};

/// Rendering section of the Spider settings dialog.
pub(super) fn settings(ui: &mut egui::Ui, c: &mut Config) {
    ui.checkbox(
        &mut c.render,
        "Render JavaScript with Chrome or Edge (slower: compares raw and rendered HTML, adds contrast and keyboard checks)",
    );
    ui.add_enabled_ui(c.render, |ui| {
        ui.horizontal(|ui| {
            ui.label("Browser");
            ui.add(
                egui::TextEdit::singleline(&mut c.browser_path)
                    .hint_text("detect automatically")
                    .desired_width(330.0),
            );
            ui.label("Tabs");
            ui.add(egui::DragValue::new(&mut c.render_tabs).range(1..=16));
        });
        match crate::render::find_browser(&c.browser_path) {
            Ok(p) => ui.label(
                RichText::new(format!("Using {}", p.display()))
                    .small()
                    .color(GOOD),
            ),
            Err(e) => ui.label(RichText::new(e).small().color(ISSUE)),
        };
    });
}

impl App {
    /// Warn before a crawl when rendering is on but no browser can be found (the crawl still runs).
    pub(super) fn render_precheck(&mut self) {
        let c = &self.settings.cfg;
        if c.render {
            if let Err(e) = crate::render::find_browser(&c.browser_path) {
                self.error(e);
            }
        }
    }
}

fn row(ui: &mut egui::Ui, name: &str, raw: String, rendered: String) {
    ui.label(RichText::new(name).strong());
    let differs = raw != rendered;
    ui.add(egui::Label::new(raw).wrap());
    let r = RichText::new(rendered);
    ui.add(egui::Label::new(if differs { r.color(WARNING) } else { r }).wrap());
    ui.end_row();
}

fn list(ui: &mut egui::Ui, title: &str, items: &[String], color: egui::Color32) {
    if items.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(color));
    for i in items {
        ui.add(egui::Label::new(RichText::new(i).monospace().small()).wrap());
    }
}

/// Raw HTML vs rendered DOM for one page.
pub(super) fn rendered_tab(ui: &mut egui::Ui, p: &Page) {
    let Some(r) = p.rendered.as_deref() else {
        ui.label(
            RichText::new("Not rendered. Turn on JavaScript rendering in Configuration > Spider settings, then crawl again.")
                .weak(),
        );
        return;
    };
    let h = p.html.as_deref();
    if let Some(e) = &r.error {
        ui.label(RichText::new(e).color(ISSUE));
    }
    egui::Grid::new("raw_vs_rendered")
        .num_columns(3)
        .striped(true)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.label(
                RichText::new(
                    "Raw HTML: what crawlers that don't run JavaScript (most AI crawlers) see",
                )
                .strong(),
            );
            ui.label(RichText::new("Rendered: what browsers and Google see").strong());
            ui.end_row();
            row(ui, "Title", p.title().into(), r.title.clone());
            row(ui, "H1", p.h1().into(), r.h1.clone());
            row(
                ui,
                "H1 count",
                h.map(|h| h.h1.len()).unwrap_or(0).to_string(),
                r.h1_count.to_string(),
            );
            row(ui, "Canonical", p.canonical().into(), r.canonical.clone());
            row(
                ui,
                "Meta robots",
                h.map(|h| h.meta_robots.join(", ")).unwrap_or_default(),
                r.meta_robots.clone(),
            );
            row(
                ui,
                "Word count",
                p.word_count().to_string(),
                r.word_count.to_string(),
            );
            row(
                ui,
                "Links",
                h.map(|h| h.links.len()).unwrap_or(0).to_string(),
                if r.new_links_total == 0 {
                    r.links.to_string()
                } else {
                    format!("{} ({} only after rendering)", r.links, r.new_links_total)
                },
            );
            if r.final_url != p.url && !r.final_url.is_empty() {
                row(ui, "URL", p.url.clone(), r.final_url.clone());
            }
            ui.label(RichText::new("Render").strong());
            ui.label("");
            ui.label(format!(
                "{} ms{}",
                r.render_ms,
                if r.timed_out { " · timed out" } else { "" }
            ));
            ui.end_row();
        });
    list(ui, "Links only in the rendered page", &r.new_links, WARNING);
    list(ui, "JavaScript errors", &r.js_errors, ISSUE);
    if r.a11y_tested {
        ui.add_space(6.0);
        ui.label(RichText::new(format!(
            "Accessibility probes: {} text elements measured for contrast, {} below the minimum · {} elements reached with Tab",
            r.contrast_checked, r.contrast_fail_count, r.focus_tested
        )).weak());
        let contrast: Vec<String> = r
            .contrast
            .iter()
            .map(|c| {
                format!(
                    "{:.2}:1 (needs {:.1}:1)  {} on {}  ×{}  \"{}\"",
                    c.ratio, c.required, c.fg, c.bg, c.count, c.text
                )
            })
            .collect();
        list(ui, "Low contrast text (WCAG 1.4.3)", &contrast, ISSUE);
        list(
            ui,
            "No visible keyboard focus (WCAG 2.4.7)",
            &r.focus_not_visible,
            WARNING,
        );
        list(
            ui,
            "Click handlers not reachable by keyboard (WCAG 2.1.1)",
            &r.click_no_keyboard,
            WARNING,
        );
    }
    if !r.text_sample.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("Rendered text (start)").strong());
        ui.add(egui::Label::new(RichText::new(&r.text_sample).weak()).wrap());
    }
}
