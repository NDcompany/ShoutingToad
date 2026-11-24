//! "AI search readiness" window: AI crawler × page access matrix and section quotability.

use super::*;
use crate::ai_search::{
    cell, flag_labels, llms_note, quotability_rows, AiBotReport, Outcome, Purpose, BOTS,
};

type QuoteRow = (String, u16, u16, String, u8);
/// (pages, crawl start time) -> rows, cached in egui's temp data.
type QuoteCache = ((usize, String), std::sync::Arc<Vec<QuoteRow>>);

impl App {
    pub(super) fn ai_search_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_ai_search;
        egui::Window::new("AI search readiness")
            .open(&mut open)
            .default_size([1040.0, 620.0])
            .show(ctx, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.ai_matrix(ui);
                        ui.separator();
                        self.ai_quotability(ui, ctx);
                    });
            });
        self.show_ai_search = open;
    }

    fn ai_matrix(&self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Can AI crawlers fetch your pages?")
                .strong()
                .size(16.0),
        );
        let Some(r) = &self.crawl.site.ai_bots else {
            ui.label("The AI crawler access test did not run for this crawl. It runs after a crawl finishes when \"Test AI crawler access\" is on in Spider settings.");
            return;
        };
        ui.label(RichText::new("Each crawler's robots.txt rules are evaluated, then the page is requested with its published user agent and compared with a normal request. Real AI crawlers also come from known IP ranges, which some firewalls check, so confirm blocks in your CDN logs.").weak());
        ui.add_space(4.0);
        for (i, p) in r.pages.iter().enumerate() {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Page {}: {}  ·  {}", i + 1, p.url, p.describe()))
                        .small(),
                )
                .wrap(),
            );
        }
        ui.add_space(6.0);
        egui::Grid::new("ai_matrix")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Crawler").strong());
                ui.label(RichText::new("Operator").strong());
                for i in 0..r.pages.len() {
                    ui.label(RichText::new(format!("Page {}", i + 1)).strong());
                }
                ui.end_row();
                for purpose in Purpose::ALL {
                    ui.label(RichText::new(purpose.label()).strong().color(OPPORTUNITY));
                    ui.end_row();
                    for b in BOTS.iter().filter(|b| b.purpose == purpose) {
                        ui.label(b.name).on_hover_text(format!(
                            "{}\nSource: {}",
                            b.ua.unwrap_or("robots.txt token only: no user agent of its own"),
                            b.source
                        ));
                        ui.label(RichText::new(b.operator).weak());
                        for pi in 0..r.pages.len() {
                            match r.rows_for(pi).find(|(x, _)| x.name == b.name) {
                                Some((_, row)) => {
                                    let color = if AiBotReport::blocked(b, row) {
                                        ISSUE
                                    } else if row.outcome == Outcome::Ok {
                                        GOOD
                                    } else if matches!(row.outcome, Outcome::NotFetched) {
                                        Color32::GRAY
                                    } else {
                                        WARNING
                                    };
                                    ui.label(RichText::new(cell(b, row)).small().color(color));
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                        }
                        ui.end_row();
                    }
                }
            });
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(crate::ai_search::LEGEND).small().weak()).wrap());
        ui.label(RichText::new(llms_note(r)).small().weak());
    }

    fn ai_quotability(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            RichText::new("Quotable sections per page")
                .strong()
                .size(16.0),
        );
        ui.label(RichText::new("Main content is split at headings. Sections of 40 words or more are scored. One stands alone when its first sentence is short and answers a question heading directly, it does not open with this/it/they, it is at most 300 words and it has numbers or names. Weakest pages first.").weak());
        // Sorting 50k pages every frame is too slow: cache per crawl size.
        let id = egui::Id::new("ai_quote_rows");
        let key = (self.crawl.pages.len(), self.crawl.started.clone());
        let cached: Option<QuoteCache> = ctx.data(|d| d.get_temp(id));
        let rows = match cached {
            Some((k, rows)) if k == key => rows,
            _ => {
                let rows: std::sync::Arc<Vec<QuoteRow>> = std::sync::Arc::new(
                    quotability_rows(&self.crawl, 1000)
                        .into_iter()
                        .map(|(u, a)| {
                            (
                                u.to_string(),
                                a.sections,
                                a.quotable,
                                a.weakest.clone(),
                                a.weakest_flags,
                            )
                        })
                        .collect(),
                );
                ctx.data_mut(|d| d.insert_temp(id, (key, rows.clone())));
                rows
            }
        };
        if rows.is_empty() {
            ui.label("No indexable page has headed sections of 40 words or more.");
            return;
        }
        egui::Grid::new("ai_quote")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for t in ["URL", "Quotable", "Weakest section", "Why"] {
                    ui.label(RichText::new(t).strong());
                }
                ui.end_row();
                for (url, sections, quotable, weakest, flags) in rows.iter() {
                    ui.label(RichText::new(url).small());
                    // Same rule as the ai.low_quotability check: 3+ sections, under half quotable.
                    let color = if *sections < 3 {
                        ui.visuals().weak_text_color()
                    } else if (*quotable as usize) * 2 < *sections as usize {
                        WARNING
                    } else {
                        GOOD
                    };
                    ui.label(RichText::new(format!("{quotable} of {sections}")).color(color));
                    ui.label(RichText::new(weakest).small());
                    ui.label(RichText::new(flag_labels(*flags).join(", ")).small().weak());
                    ui.end_row();
                }
            });
    }
}
