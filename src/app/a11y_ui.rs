//! Reports → Accessibility (WCAG/EAA) report.

use super::App;

impl App {
    /// Write accessibility.html and accessibility.csv to a folder the user picks.
    pub(super) fn export_a11y(&mut self) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let files = [
            (
                "accessibility.html",
                crate::wcag::html(&self.crawl, &self.audit),
            ),
            (
                "accessibility.csv",
                crate::wcag::csv(&self.crawl, &self.audit),
            ),
        ];
        for (name, data) in files {
            if let Err(e) = std::fs::write(dir.join(name), data) {
                self.error(format!("Could not write {name}: {e}"));
                return;
            }
        }
        self.toast(format!(
            "Saved accessibility.html and accessibility.csv to {}",
            dir.display()
        ));
    }
}
