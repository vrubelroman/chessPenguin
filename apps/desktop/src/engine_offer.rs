//! Getting an engine when there is none (decision 14, `docs/design/engine-download.md`):
//! an offer to download the official Stockfish, its progress (hidden into a small
//! indicator if the user wants), the errors with what to do next, and the action the
//! user came for (live analysis or game analysis) started once the engine is there.

use std::path::PathBuf;

use egui::{RichText, Ui, vec2};

use crate::engine_download::{self, Download, Failure, Stage};
use crate::i18n::{tr, trf};
use crate::settings::Settings;

/// The id of the download indicator in the corner (it is never in the way of Escape).
pub const DOWNLOAD_INDICATOR: &str = "download_indicator";

/// What the user wanted to do when the engine turned out to be missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    LiveAnalysis,
    /// The analysis of the game on the screen whose `serial` is `game` (no other game's);
    /// `full`: anew, over evaluations read from the PGN.
    GameAnalysis {
        game: u64,
        full: bool,
    },
    /// The search for opening errors of the preparation whose `serial` is `prep`.
    OpeningErrors {
        prep: u64,
    },
}

/// A number of its own for a screen an action waits on (a game, a preparation), so
/// that an engine arriving later starts it there and nowhere else.
pub fn next_serial() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// What the owner should do.
pub enum OfferEvent {
    /// The downloaded Stockfish is ready: make it the active engine, then run `Pending`.
    Installed(Option<Pending>),
    /// Use the Stockfish found on this computer, then run `Pending`.
    UseSystem(PathBuf, Option<Pending>),
    /// Open the file dialog to choose an engine.
    #[cfg(not(web))]
    ChooseFile,
    Toast(String),
}

#[derive(Clone, PartialEq)]
enum Mode {
    Offer,
    /// A `stockfish` is installed on the computer.
    System(PathBuf),
    /// The downloaded engine was deleted from the disk.
    Missing,
    /// The game analysis or the opening errors, which need ChessPenguin's Stockfish 19
    /// itself (their search is the same on every computer): only its download helps.
    Own,
    /// The same, when the user's engine does not do (`review_engine`): it is named.
    OwnInstead(crate::review_engine::Unfit),
}

pub struct EngineOffer {
    open: bool,
    mode: Mode,
    pending: Option<Pending>,
    download: Option<Download>,
    failure: Option<Failure>,
    /// Bytes per second, smoothed, for "about 20 s left".
    speed: f64,
    last: Option<(f64, u64)>,
}

impl EngineOffer {
    pub fn new() -> EngineOffer {
        EngineOffer {
            open: false,
            mode: Mode::Offer,
            pending: None,
            download: None,
            failure: None,
            speed: 0.0,
            last: None,
        }
    }

    /// Whether the active engine can be started.
    pub fn engine_ready(settings: &Settings) -> bool {
        settings.active_engine().is_some_and(|e| e.present())
    }

    /// Opens the offer for an action that needs an engine.
    pub fn ask(&mut self, pending: Pending, settings: &Settings) {
        self.pending = Some(pending);
        self.failure = None;
        self.mode = if pending != Pending::LiveAnalysis {
            Mode::Own
        } else if settings.active_engine().is_some_and(|e| e.builtin) {
            Mode::Missing
        } else if settings.engines.is_empty()
            && let Some(p) = crate::engines::system_stockfish()
        {
            Mode::System(p)
        } else {
            Mode::Offer
        };
        self.open = true;
    }

    /// Opens the offer of ChessPenguin's Stockfish 19 for the game analysis or the
    /// opening errors, saying why the user's engine does not do.
    pub fn ask_instead_of(&mut self, pending: Pending, unfit: crate::review_engine::Unfit) {
        self.pending = Some(pending);
        self.failure = None;
        self.mode = Mode::OwnInstead(unfit);
        self.open = true;
    }

    /// Starts the download (from the offer or from the settings).
    pub fn start(&mut self, ctx: &egui::Context) {
        if self.download.as_ref().is_some_and(Download::running) {
            return;
        }
        self.failure = None;
        self.speed = 0.0;
        self.last = None;
        self.download = Some(Download::start(ctx));
    }

    /// The download's stage, for the settings window.
    pub fn stage(&self) -> Option<Stage> {
        self.download.as_ref().map(Download::stage)
    }

    pub fn cancel(&mut self) {
        if let Some(d) = &self.download {
            d.cancel();
        }
    }

    /// The offer window, the indicator of a hidden download, and what follows a
    /// finished one.
    pub fn show(&mut self, ctx: &egui::Context) -> Vec<OfferEvent> {
        let mut events = Vec::new();
        let now = ctx.input(|i| i.time);
        if let Some(d) = &self.download {
            match d.stage() {
                Stage::Done(_) => {
                    let pending = self.pending.take();
                    events.push(OfferEvent::Installed(pending));
                    events.push(OfferEvent::Toast(
                        match pending {
                            Some(Pending::GameAnalysis { .. }) => {
                                tr("Stockfish installed — game analysis started")
                            }
                            Some(Pending::LiveAnalysis) => {
                                tr("Stockfish installed — analysis is on")
                            }
                            Some(Pending::OpeningErrors { .. }) => {
                                tr("Stockfish installed — the search for opening errors started")
                            }
                            None => tr("Stockfish installed"),
                        }
                        .to_string(),
                    ));
                    self.download = None;
                    self.open = false;
                }
                Stage::Failed(f) => {
                    if !self.open {
                        events.push(OfferEvent::Toast(
                            tr("⚠ Stockfish wasn't downloaded").into(),
                        ));
                    }
                    self.failure = Some(f);
                    self.download = None;
                }
                Stage::Cancelled => {
                    self.download = None;
                    self.pending = None;
                    self.open = false;
                }
                Stage::Downloading { done, .. } => {
                    if let Some((t, b)) = self.last
                        && now - t >= 0.5
                    {
                        let rate = (done.saturating_sub(b)) as f64 / (now - t);
                        self.speed = if self.speed == 0.0 {
                            rate
                        } else {
                            0.7 * self.speed + 0.3 * rate
                        };
                        self.last = Some((now, done));
                    } else if self.last.is_none() {
                        self.last = Some((now, done));
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Stage::Unpacking => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
            }
        }
        if self.open {
            self.window(ctx, &mut events);
        } else if self.download.is_some() || self.failure.is_some() {
            self.indicator(ctx);
        }
        events
    }

    fn window(&mut self, ctx: &egui::Context, events: &mut Vec<OfferEvent>) {
        let modal = egui::Modal::new(egui::Id::new("engine_offer")).show(ctx, |ui| {
            ui.set_width(440.0);
            if let Some(d) = &self.download {
                self.progress_body(ui, &d.stage());
            } else if let Some(f) = self.failure.clone() {
                self.failure_body(ui, &f, events);
            } else {
                self.offer_body(ui, events);
            }
        });
        if modal.should_close() {
            // Escape or a click outside: "Later", or "Hide" while downloading.
            self.open = false;
            if self.download.is_none() {
                self.pending = None;
                self.failure = None;
            }
        }
    }

    fn offer_body(&mut self, ui: &mut Ui, events: &mut Vec<OfferEvent>) {
        let size = engine_download::size_mb();
        let (title, text): (&str, String) = match &self.mode {
            Mode::Offer => (
                tr("Analysis needs a chess engine"),
                tr(
                    "Download Stockfish, the strongest free chess engine? Analysis will start as soon as the download finishes.",
                )
                .into(),
            ),
            Mode::System(_) => (
                tr("Analysis needs a chess engine"),
                tr("Stockfish is already installed on this computer.").into(),
            ),
            Mode::Missing => (
                tr("Stockfish not found"),
                tr("The Stockfish file is missing from the disk. Download it again?").into(),
            ),
            Mode::Own => (
                tr("Analysis needs Stockfish 19"),
                tr(
                    "Game analysis and the search for opening errors always use ChessPenguin's Stockfish 19, so that their marks are the same on every computer. Download it? The analysis will start as soon as the download finishes.",
                )
                .into(),
            ),
            Mode::OwnInstead(unfit) => (
                tr("Analysis needs Stockfish 19"),
                instead_text(unfit, self.pending),
            ),
        };
        ui.heading(title);
        ui.add_space(6.0);
        ui.label(text);
        ui.add_space(4.0);
        if matches!(self.mode, Mode::System(_)) {
            ui.label(
                RichText::new(trf(
                    "Or download the official Stockfish 19 build ({mb} MB).",
                    &[("mb", &size)],
                ))
                .weak(),
            );
        } else if cfg!(web) {
            // The browser's engine: from our site, into this browser (design §1.4).
            ui.label(tr(
                "It evaluates the position, shows the best moves and reviews the game. It is downloaded once and kept in this browser.",
            ));
            let free = crate::platform::storage_free().map(|f| {
                trf(
                    "free for the site: {free}",
                    &[("free", &crate::platform::size_text(f))],
                )
            });
            let size = trf("Size: {size}", &[("size", &format!("{size} MB"))]);
            ui.label(RichText::new(free.map_or(size.clone(), |f| format!("{size} · {f}"))).weak());
        } else {
            ui.label(
                RichText::new(trf(
                    "{mb} MB · from the official Stockfish page",
                    &[("mb", &size)],
                ))
                .weak(),
            )
            .on_hover_text("https://github.com/official-stockfish/Stockfish/releases/tag/sf_19");
            ui.label(RichText::new(tr("Installed for your account only")).weak());
        }
        ui.add_space(10.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.mode.clone() {
                    Mode::System(p) => {
                        let main = ui.add(egui::Button::new(RichText::new(tr("Use it")).strong()));
                        main.request_focus();
                        if main.clicked() || enter {
                            events.push(OfferEvent::UseSystem(p, self.pending.take()));
                            self.open = false;
                        }
                        if ui.button(tr("Later")).clicked() {
                            self.open = false;
                            self.pending = None;
                        }
                        if ui.button(tr("Download")).clicked() {
                            self.start(ui.ctx());
                        }
                    }
                    mode => {
                        let label = if mode == Mode::Missing {
                            tr("Download again")
                        } else {
                            tr("Download Stockfish")
                        };
                        let main = ui.add(egui::Button::new(RichText::new(label).strong()));
                        main.request_focus();
                        if main.clicked() || enter {
                            self.start(ui.ctx());
                        }
                        if ui.button(tr("Later")).clicked() {
                            self.open = false;
                            self.pending = None;
                        }
                    }
                }
                // Another engine does not do for the game analysis (and a browser has
                // no engine programs).
                #[cfg(not(web))]
                if !matches!(self.mode, Mode::Own | Mode::OwnInstead(_)) {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if ui.button(tr("Choose engine file…")).clicked() {
                            events.push(OfferEvent::ChooseFile);
                            self.open = false;
                            self.pending = None;
                        }
                    });
                }
            });
        });
    }

    fn progress_body(&mut self, ui: &mut Ui, stage: &Stage) {
        ui.heading(tr("Downloading Stockfish"));
        ui.add_space(8.0);
        match *stage {
            Stage::Downloading { done, total } => {
                let frac = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .show_percentage()
                        .desired_width(ui.available_width()),
                );
                let mb = |b: u64| b / 1_000_000;
                let mut line = trf(
                    "{done} of {total} MB",
                    &[("done", &mb(done)), ("total", &mb(total))],
                );
                if self.speed > 0.0 {
                    let left = (total.saturating_sub(done)) as f64 / self.speed;
                    line.push_str(" · ");
                    line.push_str(&trf("about {s} s left", &[("s", &(left.ceil() as u64))]));
                }
                ui.label(line);
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Checking and installing…"));
                });
            }
        }
        ui.add_space(4.0);
        let after = match self.pending {
            Some(Pending::GameAnalysis { .. }) => {
                tr("Game analysis will start by itself when the download finishes.")
            }
            Some(Pending::OpeningErrors { .. }) => {
                tr("The search for opening errors will start by itself when the download finishes.")
            }
            _ => tr("Analysis will start by itself when the download finishes."),
        };
        ui.label(RichText::new(after).weak());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let hide = ui.add(egui::Button::new(RichText::new(tr("Hide")).strong()));
                hide.request_focus();
                if hide.clicked() {
                    self.open = false;
                }
                if ui.button(tr("Cancel download")).clicked() {
                    self.cancel();
                }
            });
        });
    }

    // In a browser nothing is pushed: there are no engine files to choose.
    #[cfg_attr(web, allow(clippy::ptr_arg))]
    fn failure_body(&mut self, ui: &mut Ui, f: &Failure, events: &mut Vec<OfferEvent>) {
        ui.heading(tr("⚠ Stockfish wasn't downloaded"));
        ui.add_space(6.0);
        let (text, details) = failure_text(f);
        ui.label(text);
        if let Some(d) = details {
            egui::CollapsingHeader::new(tr("Details"))
                .id_salt("download_details")
                .show(ui, |ui| {
                    ui.label(RichText::new(d).small().weak());
                });
        }
        if matches!(f, Failure::Network(_))
            && cfg!(target_os = "linux")
            && !matches!(self.mode, Mode::Own | Mode::OwnInstead(_))
        {
            ui.label(
                RichText::new(tr(
                    "A stockfish package of your distribution is found by ChessPenguin too.",
                ))
                .weak(),
            );
        }
        ui.add_space(10.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let retry = ui.add(egui::Button::new(RichText::new(tr("Retry")).strong()));
                retry.request_focus();
                if retry.clicked() || enter {
                    self.start(ui.ctx());
                }
                if ui.button(tr("Close")).clicked() {
                    self.open = false;
                    self.failure = None;
                    self.pending = None;
                }
                #[cfg(not(web))]
                if !matches!(self.mode, Mode::Own | Mode::OwnInstead(_)) {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if ui.button(tr("Choose engine file…")).clicked() {
                            events.push(OfferEvent::ChooseFile);
                            self.open = false;
                            self.failure = None;
                            self.pending = None;
                        }
                    });
                }
            });
        });
    }

    /// "⬇ Stockfish 42 %" in the top right corner while a download runs hidden (or
    /// "⚠ Stockfish wasn't downloaded" after it failed); a click opens the window.
    fn indicator(&mut self, ctx: &egui::Context) {
        let text = match self.download.as_ref().map(Download::stage) {
            Some(Stage::Downloading { done, total }) if total > 0 => {
                format!("⬇ Stockfish {} %", done * 100 / total)
            }
            Some(_) => "⬇ Stockfish…".to_string(),
            None => tr("⚠ Stockfish wasn't downloaded").to_string(),
        };
        egui::Area::new(egui::Id::new(DOWNLOAD_INDICATOR))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-12.0, -12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Label::new(text).sense(egui::Sense::click()))
                            .on_hover_text(tr("Show the download"))
                            .clicked()
                        {
                            self.open = true;
                        }
                        let close = crate::widgets::small_close_button(ui);
                        let close = if self.download.is_some() {
                            close.on_hover_text(tr("Cancel download"))
                        } else {
                            close.on_hover_text(tr("Close"))
                        };
                        if close.clicked() {
                            if self.download.is_some() {
                                self.cancel();
                            } else {
                                self.failure = None;
                                self.pending = None;
                            }
                        }
                    });
                });
            });
    }
}

/// Why the user's engine does not do for the game analysis (or the opening errors),
/// what is offered instead, and that the analysis follows the download by itself.
fn instead_text(unfit: &crate::review_engine::Unfit, pending: Option<Pending>) -> String {
    use crate::review_engine::Why;
    let errors = matches!(pending, Some(Pending::OpeningErrors { .. }));
    let engine = crate::widgets::shorten(&unfit.name, 48);
    let mut parts = vec![match unfit.why {
        Why::Differs => tr(
            "Your Stockfish 19 calculates differently from ChessPenguin's build (another build or network).",
        )
        .to_owned(),
        Why::NotStarted => trf("{engine} could not be started.", &[("engine", &engine)]),
        Why::TimedOut => trf(
            "{engine} did not finish the check in time; it will be checked again next time.",
            &[("engine", &engine)],
        ),
        Why::Other if unfit.from_settings => {
            trf("The settings name {engine}.", &[("engine", &engine)])
        }
        Why::Other => trf("The system has {engine}.", &[("engine", &engine)]),
    }];
    let official = matches!(unfit.why, Why::Differs | Why::TimedOut);
    parts.push(
        match (official, errors) {
            (true, false) => tr(
                "So that the marks are the same on every computer, game analysis runs on the official build.",
            ),
            (true, true) => tr(
                "So that the marks are the same on every computer, the search for opening errors runs on the official build.",
            ),
            (false, false) => tr(
                "Game analysis needs Stockfish 19 itself, so that the marks are the same on every computer.",
            ),
            (false, true) => tr(
                "The search for opening errors needs Stockfish 19 itself, so that the marks are the same on every computer.",
            ),
        }
        .to_owned(),
    );
    parts.push(
        if official {
            tr("Download it?")
        } else {
            tr("Download Stockfish 19?")
        }
        .to_owned(),
    );
    if unfit.why != Why::NotStarted {
        parts.push(tr("Your engine stays for position analysis.").to_owned());
    }
    parts.push(
        if errors {
            tr("The search for opening errors will start by itself when the download finishes.")
        } else {
            tr("Game analysis will start by itself when the download finishes.")
        }
        .to_owned(),
    );
    parts.join(" ")
}

/// What to tell the user about a failure, and the technical details (collapsed).
fn failure_text(f: &Failure) -> (String, Option<String>) {
    match f {
        Failure::Network(e) => (
            tr("Couldn't connect. Check your internet connection and try again.").into(),
            Some(e.clone()),
        ),
        Failure::Server(code) => (
            trf(
                "The server didn't provide the file (error {code}). Please try again later.",
                &[("code", code)],
            ),
            None,
        ),
        Failure::Checksum => (
            tr("The downloaded file failed the integrity check and was deleted. It was most likely damaged during the download.").into(),
            Some("SHA-256 mismatch".into()),
        ),
        Failure::Disk(e) => (
            tr("Couldn't save the engine on the disk (not enough space or no permission).").into(),
            Some(e.clone()),
        ),
        Failure::Broken => (
            tr("Stockfish was downloaded but doesn't run on this computer.").into(),
            None,
        ),
        Failure::Unsupported => (
            tr("There is no Stockfish download for this computer. Choose an engine file instead.")
                .into(),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{Pending, instead_text};
    use crate::review_engine::{Unfit, Why};

    fn text(why: Why, from_settings: bool, pending: Pending) -> String {
        let unfit = Unfit {
            name: "Rodent V 1.2".into(),
            from_settings,
            why,
        };
        instead_text(&unfit, Some(pending))
    }

    /// The offer says why the user's engine does not do and where it is from; the
    /// user's engine stays for the live analysis, the analysis follows the download.
    #[test]
    fn the_offer_says_why_the_users_engine_does_not_do() {
        let game = Pending::GameAnalysis {
            game: 1,
            full: false,
        };
        let errors = Pending::OpeningErrors { prep: 1 };
        let t = text(Why::Other, true, game);
        assert!(t.starts_with("The settings name Rodent V 1.2."), "{t}");
        assert!(t.contains("Your engine stays for position analysis."));
        assert!(t.ends_with("Game analysis will start by itself when the download finishes."));
        let t = text(Why::Other, false, errors);
        assert!(t.starts_with("The system has Rodent V 1.2."), "{t}");
        assert!(t.contains("The search for opening errors needs Stockfish 19 itself"));
        let t = text(Why::Differs, true, game);
        assert!(
            t.starts_with("Your Stockfish 19 calculates differently"),
            "{t}"
        );
        assert!(t.contains("Download it?"));
        let t = text(Why::TimedOut, true, game);
        assert!(
            t.starts_with("Rodent V 1.2 did not finish the check in time"),
            "{t}"
        );
        assert!(t.contains("runs on the official build") && t.contains("Download it?"));
        let t = text(Why::NotStarted, true, game);
        assert!(t.starts_with("Rodent V 1.2 could not be started."), "{t}");
        assert!(!t.contains("stays for position analysis"));
    }
}
