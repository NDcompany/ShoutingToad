//! Tools → Migration redirect mapper: propose a new URL for every old indexable URL, let the
//! user correct targets by hand, and export the map and server snippets.

use super::*;
use crate::migrate::{self, Mapping, Method};

#[derive(Default)]
pub(super) struct MigrateUi {
    pub(super) open: bool,
    pub(super) rx: Option<Receiver<Result<Crawl, String>>>,
    other: Option<Box<Crawl>>,
    path: String,
    /// The current crawl is the old site and the loaded one the new site.
    current_is_old: bool,
    maps: Vec<Mapping>,
    filter: String,
    review_only: bool,
}

impl App {
    pub(super) fn migrate_pick(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Crawl of the other site (by default: the old site)")
            .add_filter("ShoutingToad crawl", &["shoutingtoad", "json"])
            .pick_file()
        else {
            return;
        };
        self.migrate_load(path);
    }

    pub(super) fn migrate_load(&mut self, path: std::path::PathBuf) {
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Crawl::load(&p));
        });
        let ui = &mut self.migrate_ui;
        if let Some(old) = ui.other.take() {
            drop_in_background(old);
        }
        ui.maps.clear();
        ui.rx = Some(rx);
        ui.path = path.display().to_string();
        ui.open = true;
    }

    fn migrate_run(&mut self) {
        let Some(other) = &self.migrate_ui.other else {
            return;
        };
        let (old, new) = if self.migrate_ui.current_is_old {
            (&self.crawl, &**other)
        } else {
            (&**other, &self.crawl)
        };
        // Targets edited by hand survive a new proposal (Propose again, direction switch).
        let manual: std::collections::HashMap<String, Mapping> = self
            .migrate_ui
            .maps
            .drain(..)
            .filter(|m| m.method == Method::Manual)
            .map(|m| (m.old.clone(), m))
            .collect();
        let mut maps = migrate::propose(old, new);
        for m in &mut maps {
            if let Some(kept) = manual.get(&m.old) {
                *m = kept.clone();
            }
        }
        self.migrate_ui.maps = maps;
    }

    /// Tools menu: reopen the map from this session (with its edits) or pick a crawl to map.
    pub(super) fn migrate_open(&mut self) {
        if self.migrate_ui.other.is_some() && !self.migrate_ui.maps.is_empty() {
            self.migrate_ui.open = true;
        } else {
            self.migrate_pick();
        }
    }

    pub(super) fn migrate_dialog(&mut self, ctx: &egui::Context) {
        if !self.migrate_ui.open {
            return;
        }
        if let Some(rx) = &self.migrate_ui.rx {
            match rx.try_recv() {
                Ok(Ok(c)) => {
                    self.migrate_ui.other = Some(Box::new(c));
                    self.migrate_ui.rx = None;
                    self.migrate_run();
                }
                Ok(Err(e)) => {
                    self.migrate_ui.rx = None;
                    self.migrate_ui.open = false;
                    self.error(e);
                    return;
                }
                Err(_) => ctx.request_repaint_after(Duration::from_millis(150)),
            }
        }
        let mut open = true;
        let (mut rerun, mut save_csv, mut save_snippets) = (false, false, false);
        let mut pick_other = false;
        let current = self.crawl.start_url.clone();
        let u = &mut self.migrate_ui;
        egui::Window::new("Migration redirect mapper")
            .open(&mut open)
            .default_size([1100.0, 620.0])
            .show(ctx, |ui| {
                ui.label("Proposes where every indexable URL of the old site should redirect on the new one: same path first, then the same title or H1, similar content, similar URL words. Unmatched URLs get their closest parent as a suggestion only. Edit any target to override it.");
                let (old, new) = if u.current_is_old {
                    (format!("current crawl ({current})"), u.path.clone())
                } else {
                    (u.path.clone(), format!("current crawl ({current})"))
                };
                ui.label(format!("Old site: {old}\nNew site: {new}"));
                if u.rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading the crawl…");
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut u.current_is_old, "The current crawl is the old site").changed() {
                        rerun = true;
                    }
                    if ui.button("Propose again").on_hover_text("Targets you edited by hand are kept").clicked() {
                        rerun = true;
                    }
                    if ui.button("Load another crawl…").clicked() {
                        pick_other = true;
                    }
                    ui.separator();
                    ui.add(egui::TextEdit::singleline(&mut u.filter).hint_text("Filter").desired_width(200.0));
                    ui.checkbox(&mut u.review_only, "Only unmatched or below 60% confidence");
                    ui.separator();
                    let any = !u.maps.is_empty();
                    save_csv = ui.add_enabled(any, egui::Button::new("Export CSV…")).clicked();
                    save_snippets = ui.add_enabled(any, egui::Button::new("Export CSV + server snippets…")).clicked();
                });
                let matched = u.maps.iter().filter(|m| m.method.redirects()).count();
                ui.label(format!(
                    "{} old URLs: {matched} matched, {} need review",
                    u.maps.len(),
                    u.maps.len() - matched
                ));
                ui.separator();
                let f = u.filter.to_lowercase();
                let rows: Vec<usize> = (0..u.maps.len())
                    .filter(|&i| {
                        let m = &u.maps[i];
                        (!u.review_only || !m.method.redirects() || m.confidence < 60)
                            && (f.is_empty()
                                || m.old.to_lowercase().contains(&f)
                                || m.new.to_lowercase().contains(&f)
                                || m.reason.to_lowercase().contains(&f))
                    })
                    .collect();
                let maps = &mut u.maps;
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(330.0).at_least(120.0).clip(true))
                    .column(Column::initial(330.0).at_least(120.0).clip(true))
                    .column(Column::exact(44.0))
                    .column(Column::remainder().at_least(160.0).clip(true))
                    .header(22.0, |mut h| {
                        for t in ["Old URL", "New URL (editable)", "Conf.", "Reason"] {
                            h.col(|ui| {
                                ui.strong(t);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(22.0, rows.len(), |mut row| {
                            let m = &mut maps[rows[row.index()]];
                            row.col(|ui| {
                                ui.add(egui::Label::new(&m.old).truncate());
                            });
                            row.col(|ui| {
                                let r = ui.add(egui::TextEdit::singleline(&mut m.new).desired_width(f32::INFINITY));
                                if r.changed() {
                                    m.method = Method::Manual;
                                    m.confidence = 100;
                                    m.reason = "Manual: edited by hand".into();
                                }
                            });
                            row.col(|ui| {
                                let c = m.confidence;
                                let col = if !m.method.redirects() {
                                    ISSUE
                                } else if c >= 80 {
                                    GOOD
                                } else if c >= 60 {
                                    WARNING
                                } else {
                                    ISSUE
                                };
                                ui.label(RichText::new(format!("{c}%")).color(col));
                            });
                            row.col(|ui| {
                                ui.add(egui::Label::new(&m.reason).truncate());
                            });
                        });
                    });
            });
        self.migrate_ui.open = open;
        if rerun {
            self.migrate_run();
        }
        if pick_other {
            self.migrate_pick();
        }
        if save_csv {
            self.save_file("redirect_map.csv", "csv", |a| {
                migrate::map_csv(&a.migrate_ui.maps)
            });
        }
        if save_snippets {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                match migrate::write_all(&dir, &self.migrate_ui.maps) {
                    Ok(()) => self.toast(format!(
                        "Wrote redirect_map.csv, .htaccess, nginx, Netlify and Cloudflare files to {}",
                        dir.display()
                    )),
                    Err(e) => self.error(e),
                }
            }
        }
    }
}
