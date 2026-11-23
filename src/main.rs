#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use shouting_toad::{app::App, cli};

#[cfg(windows)]
fn attach_console() {
    // Release builds use the GUI subsystem; reattach to the launching terminal for CLI output.
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX);
    }
}

#[cfg(not(windows))]
fn attach_console() {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // GUI flags: `open <file>` loads a saved crawl, `--screenshot <png>` renders one frame and exits.
    let mut open = None;
    let mut screenshot = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "open" => open = args.get(i + 1).map(std::path::PathBuf::from),
            "--screenshot" => screenshot = args.get(i + 1).cloned(),
            a if a.ends_with(".shoutingtoad") => open = Some(a.into()),
            _ => {
                attach_console();
                std::process::exit(cli::run(args));
            }
        }
        i += 2;
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("ShoutingToad SEO Spider")
        .with_app_id("shouting-toad")
        .with_inner_size([1400.0, 880.0])
        .with_min_inner_size([960.0, 620.0]);
    if let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon_64.png")) {
        let rgba = img.to_rgba8();
        let (width, height) = (rgba.width(), rgba.height());
        viewport = viewport.with_icon(egui::IconData {
            rgba: rgba.into_raw(),
            width,
            height,
        });
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "ShoutingToad",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, screenshot, open)))),
    ) {
        eprintln!("Could not start the window: {e}");
        std::process::exit(1);
    }
}
