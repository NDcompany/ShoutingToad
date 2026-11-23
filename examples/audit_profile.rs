//! Times the audit engine on a saved crawl: load, context build and the slowest checks.
//!
//! USAGE
//!   cargo run --release --example audit_profile -- path/to/crawl.shoutingtoad

use shouting_toad::audit::{self, Ctx};
use shouting_toad::checks::catalogue;
use shouting_toad::model::Crawl;
use std::time::Instant;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: audit_profile <crawl.shoutingtoad>");
    let t = Instant::now();
    let crawl = Crawl::load(std::path::Path::new(&path)).expect("load");
    println!(
        "load: {:.2}s ({} pages)",
        t.elapsed().as_secs_f32(),
        crawl.pages.len()
    );

    let t = Instant::now();
    let ctx = Ctx::new(&crawl);
    println!("Ctx::new: {:.2}s", t.elapsed().as_secs_f32());

    let mut times: Vec<(f32, &str)> = catalogue()
        .iter()
        .map(|c| {
            let t = Instant::now();
            let n = crawl.pages.iter().filter(|p| (c.test)(p, &ctx)).count();
            std::hint::black_box(n);
            (t.elapsed().as_secs_f32(), c.id.as_str())
        })
        .collect();
    let total: f32 = times.iter().map(|t| t.0).sum();
    times.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("all checks, single thread: {total:.2}s");
    for (s, id) in times.iter().take(25) {
        println!("  {s:>7.3}s  {id}");
    }

    let t = Instant::now();
    let a = audit::run(&crawl);
    println!(
        "audit::run (parallel): {:.2}s, {} checks triggered",
        t.elapsed().as_secs_f32(),
        a.triggered().count()
    );
}
