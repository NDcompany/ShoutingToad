//! Docs helper: drive the app from a script and save screenshots, for the user guide.
//!
//!   SHOUTINGTOAD_SCRIPT="view:urls; select:/catalogue/; insp:serp; shot:out.png" \
//!     shouting-toad open crawl.shoutingtoad --screenshot last.png
//!
//! Steps run one after another, separated by `;`. The app exits after the last step; `--screenshot`
//! only switches scripted mode on (without a script it shoots one frame, as before).
//!
//!   view:overview|urls          quick:<filter label>        search:<text>
//!   issue:<check id>            select:<url substring>      insp:<inspector tab>
//!   open:<dialog>               compare:<file>              migrate:<file>
//!   url:<text>  start           list:<url>|<url>            pack:<rule pack path>
//!   click:x,y   rclick:x,y      move:x,y                    scroll:dy
//!   type:<text> key:<Enter|Tab|Escape|ArrowDown|...>        theme:dark|light
//!   wait:<frames>               until:crawl|compare|migrate|ai     shot:<png path>

use super::*;
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Scene {
    steps: VecDeque<String>,
    wait: usize,
    /// Input events for the next frame (see `raw_input_hook`).
    pub(super) events: Vec<egui::Event>,
    pointer: egui::Pos2,
    release: Option<egui::PointerButton>,
    shot_path: Option<String>,
    started: bool,
}

impl App {
    /// One scripted step per call; returns false when there is no script.
    pub(super) fn scene_tick(&mut self, ctx: &egui::Context) -> bool {
        if !self.scene.started {
            self.scene.started = true;
            let script = std::env::var("SHOUTINGTOAD_SCRIPT").unwrap_or_default();
            self.scene.steps = script
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if self.scene.steps.is_empty() {
                return false;
            }
            self.scene.wait = 20; // let the first frames lay out
        }
        if self.scene.steps.is_empty() && self.scene.shot_path.is_none() {
            return !std::env::var("SHOUTINGTOAD_SCRIPT")
                .unwrap_or_default()
                .trim()
                .is_empty();
        }
        if let Some(b) = self.scene.release.take() {
            self.scene.events.push(egui::Event::PointerButton {
                pos: self.scene.pointer,
                button: b,
                pressed: false,
                modifiers: Default::default(),
            });
            self.scene.wait = self.scene.wait.max(8);
        }
        if self.scene.shot_path.is_some() || self.scene.wait > 0 {
            self.scene.wait = self.scene.wait.saturating_sub(1);
            return true;
        }
        let Some(step) = self.scene.steps.pop_front() else {
            return true;
        };
        let (cmd, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        let arg = arg.trim().to_string();
        let mut wait = 6;
        match cmd.trim() {
            "view" => {
                self.view = if arg == "urls" {
                    View::Urls
                } else {
                    View::Overview
                };
            }
            "quick" => {
                if let Some((q, _)) = QUICKS.iter().find(|(_, n)| n.eq_ignore_ascii_case(&arg)) {
                    self.quick = *q;
                    self.view = View::Urls;
                    self.rows_dirty = true;
                }
            }
            "search" => {
                self.search = arg;
                self.rows_dirty = true;
            }
            "issue" => {
                self.selected_check = catalogue().iter().position(|c| c.id == arg);
                self.view = View::Urls;
                self.rows_dirty = true;
            }
            "select" => {
                let pages = &self.crawl.pages;
                self.selected = pages
                    .iter()
                    .position(|p| p.url == arg)
                    .or_else(|| pages.iter().position(|p| p.url.contains(&arg)));
                self.view = View::Urls;
            }
            "insp" => {
                self.insp = match arg.as_str() {
                    "issues" => Insp::Issues,
                    "inlinks" => Insp::Inlinks,
                    "outlinks" => Insp::Outlinks,
                    "images" => Insp::Images,
                    "headers" => Insp::Headers,
                    "serp" => Insp::Serp,
                    "schema" => Insp::Schema,
                    "rendered" => Insp::Rendered,
                    "vitals" => Insp::Vitals,
                    "mobile" => {
                        self.mobile_serp = true;
                        Insp::Serp
                    }
                    _ => Insp::Details,
                };
            }
            "open" => match arg.as_str() {
                "config" => self.show_config = true,
                "list" => self.show_list = true,
                "ai_settings" => self.show_ai_settings = true,
                "ai_writer" => self.show_ai_writer = true,
                "links" => {
                    self.opportunities = ai::link_opportunities(&self.crawl);
                    self.show_links = true;
                }
                "report" => self.show_report = true,
                "ci" => {
                    self.ci_url = self.crawl.start_url.clone();
                    self.show_ci = true;
                }
                "catalogue" => self.show_catalogue = true,
                "about" => self.show_about = true,
                "ai_search" => self.show_ai_search = true,
                "rules" => self.rule_packs.open = true,
                "cwv" => self.cwv.show = true,
                _ => {}
            },
            "compare" => self.compare_load(arg.into()),
            "migrate" => self.migrate_load(arg.into()),
            "pack" => {
                self.settings.rule_packs.push(rules::PackEntry {
                    path: arg,
                    enabled: true,
                });
                self.apply_rule_packs();
            }
            "url" => self.url_input = arg,
            "start" => {
                let u = self.url_input.clone();
                self.start(vec![u], false);
            }
            "list" => self.settings.list_text = arg.replace('|', "\n"),
            "click" | "rclick" => {
                // Move first, press on a later frame: a press that arrives with the move is
                // treated as a drag by windows under the pointer.
                if let Some(p) = pos(&arg) {
                    self.scene.pointer = p;
                    self.scene.events.push(egui::Event::PointerMoved(p));
                    self.scene.steps.push_front(format!("press{cmd}:{arg}"));
                    wait = 2;
                }
            }
            "pressclick" | "pressrclick" | "move" => {
                if let Some(p) = pos(&arg) {
                    self.scene.pointer = p;
                    self.scene.events.push(egui::Event::PointerMoved(p));
                    if cmd != "move" {
                        let b = if cmd == "pressrclick" {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        };
                        self.scene.events.push(egui::Event::PointerButton {
                            pos: p,
                            button: b,
                            pressed: true,
                            modifiers: Default::default(),
                        });
                        self.scene.release = Some(b);
                        wait = 0;
                    }
                }
            }
            "scroll" => {
                let dy: f32 = arg.parse().unwrap_or(-200.0);
                self.scene.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, dy),
                    modifiers: Default::default(),
                });
            }
            "type" => self.scene.events.push(egui::Event::Text(arg)),
            "key" => {
                if let Some(key) = egui::Key::from_name(&arg) {
                    for pressed in [true, false] {
                        self.scene.events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: Default::default(),
                        });
                    }
                }
            }
            "theme" => {
                self.settings.theme = if arg == "light" {
                    ThemeChoice::Light
                } else {
                    ThemeChoice::Dark
                };
                apply_theme(ctx, self.settings.theme);
            }
            "wait" => wait = arg.parse().unwrap_or(30),
            "until" => {
                let done = match arg.as_str() {
                    "crawl" => !matches!(self.state, State::Crawling | State::Paused),
                    "compare" => self.compare_ui.rx.is_none(),
                    "migrate" => self.migrate_ui.rx.is_none(),
                    "ai" => self.ai_rx.is_none(),
                    _ => true,
                };
                if !done {
                    self.scene.steps.push_front(step.clone());
                }
                wait = if done { 20 } else { 10 };
            }
            "shot" => {
                self.scene.shot_path = Some(arg);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                wait = 0;
            }
            _ => eprintln!("scene: unknown step {step:?}"),
        }
        self.rows_dirty = true;
        self.scene.wait = wait;
        true
    }

    /// Path for a screenshot that just arrived; exits after the last step.
    pub(super) fn scene_shot_taken(&mut self) -> Option<(String, bool)> {
        let path = self.scene.shot_path.take()?;
        Some((path, self.scene.steps.is_empty()))
    }
}

fn pos(s: &str) -> Option<egui::Pos2> {
    let (x, y) = s.split_once(',')?;
    Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
}
