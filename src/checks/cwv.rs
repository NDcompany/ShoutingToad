//! Core Web Vitals field data (Chrome UX Report), at URL and origin level.

use super::*;
use crate::cwv::{Metric, Rating};

pub fn add(v: &mut Vec<Check>) {
    const CAT: &str = "Core Web Vitals";
    let metrics = [
        (
            Metric::Lcp,
            "lcp",
            "Largest Contentful Paint (LCP)",
            "how long the main content takes to appear (good: 2.5 s or less, poor: over 4 s)",
            "Speed up the server response, preload and compress the LCP image, and remove render-blocking CSS and JavaScript.",
        ),
        (
            Metric::Inp,
            "inp",
            "Interaction to Next Paint (INP)",
            "how quickly the page responds to taps, clicks and key presses (good: 200 ms or less, poor: over 500 ms)",
            "Break up long JavaScript tasks, defer non-essential scripts and reduce third-party code.",
        ),
        (
            Metric::Cls,
            "cls",
            "Cumulative Layout Shift (CLS)",
            "how much the layout jumps while loading (good: 0.1 or less, poor: over 0.25)",
            "Give images, embeds and ads fixed dimensions and avoid inserting content above existing content.",
        ),
    ];
    for (m, key, name, what, fix) in metrics {
        for (rating, label, slug, kind, prio) in [
            (Rating::Poor, "Poor", "poor", Kind::Issue, Priority::High),
            (
                Rating::NeedsImprovement,
                "Needs Improvement",
                "needs_improvement",
                Kind::Warning,
                Priority::Medium,
            ),
        ] {
            let rated = label.to_lowercase();
            v.push(Check {
                id: format!("cwv.{key}_{slug}"),
                category: CAT,
                name: format!("{label} {} (URL)", m.label()),
                kind,
                priority: prio,
                description: format!("Real Chrome users' {name} on this URL, at the 75th percentile (Chrome UX Report, phone or desktop), is rated {rated}. {name} measures {what}."),
                fix: fix.into(),
                test: t(move |p, _| p.cwv.as_ref().is_some_and(|c| c.worst(m) == Some(rating))),
            });
            v.push(Check {
                id: format!("cwv.origin_{key}_{slug}"),
                category: CAT,
                name: format!("{label} {} (Whole Site)", m.label()),
                kind,
                priority: prio,
                description: format!("Across the whole origin, real Chrome users' {name} at the 75th percentile (Chrome UX Report, phone or desktop) is rated {rated}. Reported on the start URL. {name} measures {what}."),
                fix: fix.into(),
                test: t(move |p, x| {
                    x.is_start(p)
                        && x.crawl
                            .site
                            .cwv_origin
                            .as_ref()
                            .is_some_and(|c| c.worst(m) == Some(rating))
                }),
            });
        }
    }
}
