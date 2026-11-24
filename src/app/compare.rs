//! Tools -> Compare with a saved crawl: new, fixed and persisting findings and URL changes
//! between a saved crawl (before) and the current one (after).

use super::*;
use crate::compare::{self as cmp, Comparison, UrlChange};

#[derive(Default)]
pub(super) struct CompareUi {
    pub(super) open: bool,
    /// Also used by the CI workflow dialog: generate the --baseline variant.
    pub(super) ci_baseline: bool,
    pub(super) rx: Option<Receiver<Result<(Crawl, Audit), String>>>,
    before: Option<Box<(Crawl, Audit)>>,
    path: String,
    cmp: Option<Comparison>,
    /// (pages, start URL, health score) of the current crawl the comparison was made with.
    key: (usize, String, u32),
    v: View,
}

#[derive(Default)]
struct View {
    urls_tab: bool,
    filter: String,
    /// Issues, Warnings, Opportunities
    kinds: [bool; 3],
    changed_only: bool,
    selected: Option<usize>,
    /// 0 new, 1 fixed, 2 persisting
    list: usize,
    /// Added, Removed, Changed
    url_kinds: [bool; 3],
    rows: Vec<(u32, u32)>,
    rows_key: (String, [bool; 3], usize),
}

impl App {
    pub(super) fn compare_pick(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Saved crawl to compare with (before)")
            .add_filter("ShoutingToad crawl", &["shoutingtoad", "json"])
            .pick_file()
        else {
            return;
        };
        self.compare_load(path);
    }

    pub(super) fn compare_load(&mut self, path: std::path::PathBuf) {
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = Crawl::load(&p).map(|c| {
                let a = audit::run(&c);
                (c, a)
            });
            let _ = tx.send(r);
        });
        let ui = &mut self.compare_ui;
        if let Some(old) = ui.before.take() {
            drop_in_background(old);
        }
        ui.cmp = None;
        ui.rx = Some(rx);
        ui.path = path.display().to_string();
        ui.open = true;
        if ui.v.kinds == [false; 3] {
            ui.v.kinds = [true; 3];
            ui.v.url_kinds = [true; 3];
            ui.v.changed_only = true;
        }
    }

    pub(super) fn compare_dialog(&mut self, ctx: &egui::Context) {
        if !self.compare_ui.open {
            return;
        }
        if let Some(rx) = &self.compare_ui.rx {
            match rx.try_recv() {
                Ok(Ok(b)) => {
                    self.compare_ui.before = Some(Box::new(b));
                    self.compare_ui.rx = None;
                }
                Ok(Err(e)) => {
                    self.compare_ui.rx = None;
                    self.compare_ui.open = false;
                    self.error(e);
                    return;
                }
                Err(_) => ctx.request_repaint_after(Duration::from_millis(150)),
            }
        }
        let busy = matches!(self.state, State::Crawling | State::Paused);
        let key = (
            self.crawl.pages.len(),
            self.crawl.start_url.clone(),
            self.audit.health_score(&self.crawl),
        );
        if !busy && (self.compare_ui.cmp.is_none() || self.compare_ui.key != key) {
            if let Some(b) = &self.compare_ui.before {
                self.compare_ui.cmp = Some(cmp::compare(&b.0, &b.1, &self.crawl, &self.audit));
                self.compare_ui.key = key;
                self.compare_ui.v.rows_key.2 = usize::MAX;
            }
        }

        let mut open = true;
        let mut export = false;
        let crawl = &self.crawl;
        let u = &mut self.compare_ui;
        egui::Window::new("Compare with a saved crawl")
            .open(&mut open)
            .default_size([1000.0, 620.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "Before: {}    After: the current crawl ({})",
                    u.path, crawl.start_url
                ));
                if u.rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading and auditing the saved crawl…");
                    });
                    return;
                }
                if busy {
                    ui.label(
                        RichText::new("Wait for the crawl to finish to compare.").color(WARNING),
                    );
                    return;
                }
                let (Some(c), Some(before)) = (&u.cmp, &u.before) else {
                    return;
                };
                let (new_k, fixed_k) = c.kind_counts();
                ui.horizontal_wrapped(|ui| {
                    let d = c.score_after as i64 - c.score_before as i64;
                    ui.label(
                        RichText::new(format!(
                            "Health {} -> {} ({d:+})",
                            c.score_before, c.score_after
                        ))
                        .strong()
                        .color(if d < 0 { ISSUE } else { GOOD }),
                    );
                    ui.separator();
                    ui.label(format!("URLs {} -> {}", c.pages_before, c.pages_after));
                    ui.separator();
                    ui.label(format!(
                        "{} added · {} removed · {} changed",
                        c.count(UrlChange::Added),
                        c.count(UrlChange::Removed),
                        c.count(UrlChange::Changed)
                    ));
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "New: {} issues, {} warnings, {} opps",
                            new_k[0], new_k[1], new_k[2]
                        ))
                        .color(ISSUE),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Fixed: {} issues, {} warnings, {} opps",
                            fixed_k[0], fixed_k[1], fixed_k[2]
                        ))
                        .color(GOOD),
                    );
                    if ui.button("Export report and CSVs…").clicked() {
                        export = true;
                    }
                });
                let v = &mut u.v;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut v.urls_tab, false, RichText::new("Findings").strong());
                    ui.selectable_value(
                        &mut v.urls_tab,
                        true,
                        RichText::new("URL changes").strong(),
                    );
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut v.filter)
                            .hint_text("Filter")
                            .desired_width(220.0),
                    );
                    if v.urls_tab {
                        for (i, l) in ["Added", "Removed", "Changed"].iter().enumerate() {
                            ui.checkbox(&mut v.url_kinds[i], *l);
                        }
                    } else {
                        for (i, l) in ["Issues", "Warnings", "Opportunities"].iter().enumerate() {
                            ui.checkbox(&mut v.kinds[i], *l);
                        }
                        ui.checkbox(&mut v.changed_only, "Only with new or fixed URLs");
                    }
                });
                ui.separator();
                if v.urls_tab {
                    url_changes(ui, v, c);
                } else {
                    findings(ui, v, c, &before.0, crawl);
                }
            });
        self.compare_ui.open = open;
        if export {
            if let (Some(dir), Some(c), Some(b)) = (
                rfd::FileDialog::new().pick_folder(),
                &self.compare_ui.cmp,
                &self.compare_ui.before,
            ) {
                match cmp::write_reports(&dir, &b.0, &self.crawl, c) {
                    Ok(()) => {
                        self.toast(format!("Exported the comparison to {}", dir.display()));
                        if let Ok(u) = url::Url::from_file_path(dir.join("comparison.html")) {
                            ctx.open_url(egui::OpenUrl::new_tab(u.to_string()));
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }
}

fn findings(ui: &mut egui::Ui, u: &mut View, c: &Comparison, before: &Crawl, after: &Crawl) {
    let cat = catalogue();
    let f = u.filter.to_lowercase();
    let mut list: Vec<usize> = (0..c.checks.len())
        .filter(|&i| {
            let d = &c.checks[i];
            let k = &cat[d.check];
            u.kinds[k.kind as usize]
                && (!u.changed_only || !d.new.is_empty() || !d.fixed.is_empty())
                && (f.is_empty()
                    || k.name.to_lowercase().contains(&f)
                    || k.category.to_lowercase().contains(&f))
        })
        .collect();
    list.sort_by_key(|&i| {
        let k = &cat[c.checks[i].check];
        (k.kind, k.priority, std::cmp::Reverse(c.checks[i].new.len()))
    });
    let mut clicked = None;
    let height = (ui.available_height() * 0.55).max(120.0);
    ui.push_id("cmp-findings", |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .max_scroll_height(height)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(90.0))
            .column(Column::exact(60.0))
            .column(Column::remainder().at_least(200.0).clip(true))
            .columns(Column::exact(64.0), 5)
            .header(22.0, |mut h| {
                for t in [
                    "Type",
                    "Priority",
                    "Finding",
                    "Before",
                    "After",
                    "New",
                    "Fixed",
                    "Persisting",
                ] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, list.len(), |mut row| {
                    let i = list[row.index()];
                    let d = &c.checks[i];
                    let k = &cat[d.check];
                    row.set_selected(u.selected == Some(i));
                    row.col(|ui| {
                        ui.label(RichText::new(k.kind.label()).color(kind_color(k.kind)));
                    });
                    row.col(|ui| {
                        ui.label(k.priority.label());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(format!("{}: {}", k.category, k.name)).truncate());
                    });
                    row.col(|ui| {
                        ui.label((d.fixed.len() + d.persisting.len()).to_string());
                    });
                    row.col(|ui| {
                        ui.label((d.new.len() + d.persisting.len()).to_string());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(d.new.len().to_string()).color(
                            if d.new.is_empty() {
                                Color32::GRAY
                            } else {
                                ISSUE
                            },
                        ));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(d.fixed.len().to_string()).color(
                            if d.fixed.is_empty() {
                                Color32::GRAY
                            } else {
                                GOOD
                            },
                        ));
                    });
                    row.col(|ui| {
                        ui.label(d.persisting.len().to_string());
                    });
                    if row.response().clicked() {
                        clicked = Some(i);
                    }
                });
            });
    });
    if clicked.is_some() {
        u.selected = clicked;
    }
    let Some(d) = u.selected.and_then(|i| c.checks.get(i)) else {
        ui.label(RichText::new("Select a finding to list its URLs.").weak());
        return;
    };
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(&cat[d.check].name);
        ui.selectable_value(&mut u.list, 0, format!("New ({})", d.new.len()));
        ui.selectable_value(&mut u.list, 1, format!("Fixed ({})", d.fixed.len()));
        ui.selectable_value(
            &mut u.list,
            2,
            format!("Persisting ({})", d.persisting.len()),
        );
    });
    let (idx, pages) = match u.list {
        0 => (&d.new, &after.pages),
        1 => (&d.fixed, &before.pages),
        _ => (&d.persisting, &after.pages),
    };
    ui.push_id("cmp-urls", |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 18.0, idx.len(), |ui, range| {
                for &i in &idx[range] {
                    if let Some(p) = pages.get(i) {
                        ui.add(egui::Label::new(&p.url).truncate());
                    }
                }
            });
    });
}

fn url_changes(ui: &mut egui::Ui, u: &mut View, c: &Comparison) {
    let key = (u.filter.to_lowercase(), u.url_kinds, 0);
    if u.rows_key != key {
        u.rows = c
            .urls
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                u.url_kinds[match d.change {
                    UrlChange::Added => 0,
                    UrlChange::Removed => 1,
                    UrlChange::Changed => 2,
                }]
            })
            .flat_map(|(i, d)| (0..d.fields.len()).map(move |j| (i, j)))
            .filter(|&(i, j)| {
                let (d, f) = (&c.urls[i], &c.urls[i].fields[j]);
                key.0.is_empty()
                    || d.url.to_lowercase().contains(&key.0)
                    || f.field.to_lowercase().contains(&key.0)
                    || f.before.to_lowercase().contains(&key.0)
                    || f.after.to_lowercase().contains(&key.0)
            })
            .map(|(i, j)| (i as u32, j as u32))
            .collect();
        u.rows_key = key;
    }
    let rows = &u.rows;
    ui.push_id("cmp-changes", |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(340.0).at_least(120.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::exact(120.0))
            .column(Column::initial(220.0).clip(true))
            .column(Column::remainder().at_least(120.0).clip(true))
            .header(22.0, |mut h| {
                for t in ["Address", "Change", "Field", "Before", "After"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, rows.len(), |mut row| {
                    let (i, j) = rows[row.index()];
                    let d = &c.urls[i as usize];
                    let f = &d.fields[j as usize];
                    row.col(|ui| {
                        ui.add(egui::Label::new(&d.url).truncate());
                    });
                    row.col(|ui| {
                        let col = match d.change {
                            UrlChange::Added => GOOD,
                            UrlChange::Removed => ISSUE,
                            UrlChange::Changed => WARNING,
                        };
                        ui.label(RichText::new(d.change.label()).color(col));
                    });
                    row.col(|ui| {
                        ui.label(f.field);
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&f.before).truncate());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&f.after).truncate());
                    });
                });
            });
    });
}
