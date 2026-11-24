//! Configuration → Rule packs: pick community rule pack files or folders, see their
//! validation errors, switch them on and off. Only the paths are saved in the settings.

use super::{App, State};
use crate::rules::{self as engine, Pack, Problem};
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const DOCS: &str = "https://github.com/NDcompany/ShoutingToad/blob/main/docs/rule-packs.md";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PackEntry {
    pub(super) path: String,
    pub(super) enabled: bool,
}

#[derive(Default)]
pub(super) struct RulesUi {
    pub open: bool,
    packs: Vec<Pack>,
    problems: Vec<Problem>,
    /// Files and modification times of what is installed; a reload with the same stamp is a no-op.
    stamp: Option<Vec<(PathBuf, Option<SystemTime>)>>,
    /// Packs changed during a crawl: applied when it ends or the next one starts.
    pending: bool,
}

fn stamp(paths: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>)> {
    paths
        .iter()
        .flat_map(|p| engine::pack_files(p).unwrap_or_else(|_| vec![p.clone()]))
        .map(|f| {
            let t = std::fs::metadata(&f).and_then(|m| m.modified()).ok();
            (f, t)
        })
        .collect()
}

impl App {
    fn crawling(&self) -> bool {
        matches!(self.state, State::Crawling | State::Paused)
    }

    /// Load the enabled packs and install them when they changed. A crawl in progress keeps
    /// the rules it started with; the current results are re-audited when they can be.
    pub(super) fn apply_rule_packs(&mut self) {
        if self.crawling() {
            self.rule_packs.pending = true;
            return;
        }
        self.rule_packs.pending = false;
        let paths: Vec<PathBuf> = self
            .settings
            .rule_packs
            .iter()
            .filter(|e| e.enabled)
            .map(|e| PathBuf::from(&e.path))
            .collect();
        let st = stamp(&paths);
        if self.rule_packs.stamp.as_ref() == Some(&st) {
            return;
        }
        let (packs, problems) = engine::load_paths(&paths);
        engine::install(&packs);
        self.rule_packs.packs = packs;
        self.rule_packs.problems = problems;
        self.rule_packs.stamp = Some(st);
        // The audit indexes the catalogue it ran with: refresh it.
        if self.has_data() {
            self.run_audit();
        } else {
            self.audit = Default::default();
        }
    }

    pub(super) fn rule_packs_dialog(&mut self, ctx: &egui::Context) {
        if self.rule_packs.pending && !self.crawling() {
            self.apply_rule_packs();
        }
        let mut open = self.rule_packs.open;
        let mut changed = false;
        let mut remove = None;
        egui::Window::new("Rule packs")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Rule packs add extra checks, written as simple TOML files. Their findings appear with the built-in checks in every view, report and export.");
                ui.horizontal(|ui| {
                    ui.hyperlink_to("How to write a rule pack", DOCS);
                    ui.label(RichText::new("· starter packs are in the rules/ folder").weak());
                });
                ui.add_space(6.0);
                if self.settings.rule_packs.is_empty() {
                    ui.label(RichText::new("No rule packs yet. Add a pack file or a folder of packs.").weak());
                }
                let installed = &self.rule_packs;
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for (i, e) in self.settings.rule_packs.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            changed |= ui.checkbox(&mut e.enabled, "").changed();
                            ui.label(RichText::new(&e.path).monospace());
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        if !e.enabled {
                            ui.label(RichText::new("    Disabled").weak());
                            continue;
                        }
                        let root = Path::new(&e.path);
                        for p in installed.packs.iter().filter(|p| p.path.starts_with(root)) {
                            let by = if p.author.is_empty() { String::new() } else { format!(" by {}", p.author) };
                            ui.horizontal(|ui| {
                                ui.label(format!("    ✔ {} {}{by}: {} rules", p.name, p.version, p.rules.len()));
                                if p.homepage.starts_with("http") {
                                    ui.hyperlink_to("homepage", &p.homepage);
                                }
                            });
                        }
                        for pr in installed.problems.iter().filter(|pr| pr.file.starts_with(root)) {
                            ui.label(RichText::new(format!("    ✖ {pr}")).color(super::ISSUE));
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Add pack file…").clicked() {
                        if let Some(files) = rfd::FileDialog::new().add_filter("Rule pack", &["toml"]).pick_files() {
                            for f in files {
                                self.settings.rule_packs.push(PackEntry { path: f.display().to_string(), enabled: true });
                            }
                            changed = true;
                        }
                    }
                    if ui.button("Add folder…").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.settings.rule_packs.push(PackEntry { path: d.display().to_string(), enabled: true });
                            changed = true;
                        }
                    }
                    if ui.button("Reload").on_hover_text("Read the files again after editing them").clicked() {
                        self.rule_packs.stamp = None;
                        changed = true;
                    }
                });
                let n: usize = self.rule_packs.packs.iter().map(|p| p.rules.len()).sum();
                let note = if self.rule_packs.pending {
                    "Changes apply when the current crawl ends.".to_string()
                } else {
                    format!("{n} rules active. Selector and HTML tests need a new crawl; other tests apply to the current results now.")
                };
                ui.label(RichText::new(note).weak());
            });
        if let Some(i) = remove {
            self.settings.rule_packs.remove(i);
            changed = true;
        }
        if changed {
            let mut seen = std::collections::HashSet::new();
            self.settings
                .rule_packs
                .retain(|p| seen.insert(p.path.clone()));
            self.apply_rule_packs();
        }
        self.rule_packs.open = open;
    }
}
