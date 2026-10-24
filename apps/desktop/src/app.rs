//! Top-level application state: library ⇄ game, settings, engine, background dialogs.

use crate::i18n::{tr, trf};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use std::sync::atomic::AtomicBool;

use chess_core::{Game, PgnDatabase, TextEncoding};
use egui::{Key, KeyboardShortcut, Modifiers, RichText};

use crate::analysis::Analysis;
use crate::assets::Sound;
use crate::game_view::{GameAction, GameSource, GameView};
use crate::library::{Library, LibraryAction};
use crate::settings::{EngineConfig, Settings, write_atomic};
use crate::settings_ui::{SettingsAction, SettingsWindow, Tab};
use crate::sound::SoundPlayer;

/// Results of work done off the UI thread (file dialogs, engine checks).
enum Async {
    DatabasesPicked(Vec<PathBuf>),
    SavePicked(PathBuf),
    SaveCancelled,
    EngineChecked(Result<(PathBuf, String), String>),
}

/// The id of the toast: a passing message, never in the way of the keyboard.
pub const TOAST: &str = "toast";

/// Something is open over the screen — a window, a menu, a dialog — in this frame or the
/// last one: Escape is meant for it (it closes itself), not for leaving the screen.
pub fn overlay_open(ctx: &egui::Context) -> bool {
    overlay_open_besides(ctx, &[])
}

/// `overlay_open`, not counting these layers of the screen itself (the filters over
/// the list).
pub fn overlay_open_besides(ctx: &egui::Context, own: &[egui::Id]) -> bool {
    let passive = [
        egui::Id::new(TOAST),
        egui::Id::new(crate::engine_offer::DOWNLOAD_INDICATOR),
    ];
    ctx.memory(|m| m.areas().visible_layer_ids())
        .iter()
        .any(|l| {
            matches!(l.order, egui::Order::Middle | egui::Order::Foreground)
                && !passive.contains(&l.id)
                && !own.contains(&l.id)
        })
}

#[derive(Clone, Copy, PartialEq)]
enum CloseIntent {
    BackToLibrary,
    QuitApp,
    /// Back to the game this one was opened from (reference tab).
    ReturnToPrevious,
}

pub struct App {
    settings: Settings,
    settings_dirty: bool,
    sound: SoundPlayer,
    library: Library,
    game: Option<GameView>,
    analysis: Analysis,
    /// The live engine was stopped for a whole-game analysis and resumes after it.
    live_paused: bool,
    settings_ui: SettingsWindow,
    toast: Option<(String, f64)>,
    tx: Sender<Async>,
    rx: Receiver<Async>,
    confirm_close: Option<CloseIntent>,
    /// Close the game once the pending "save as" succeeds.
    close_after_save: Option<CloseIntent>,
    /// The offer to download Stockfish when an action needs an engine.
    engine_offer: crate::engine_offer::EngineOffer,
    offer_hook_done: bool,
    /// Lichess / Chess.com: which answered at start, and the download window.
    online_probe: crate::online::Probe,
    online: crate::online_ui::OnlineDialog,
    /// Translating comments: the setup and consent windows, the ChatGPT sign-in.
    translation: crate::translate_ui::Hub,
    /// A position to find in a database once the game is closed.
    find_after_close: Option<(PathBuf, shakmaty::Chess, Vec<shakmaty::Move>, bool)>,
    /// Games left for one opened from the reference tab ("⬅ Back to …"), kept as they
    /// were (unsaved edits included).
    previous_games: Vec<GameView>,
    /// The last game showed the reference tab: the next one opens with it too.
    reference_tab: bool,
    /// Updates: the release check, how this copy is installed, the window, and the
    /// executable to start after quitting (a restart into the new version).
    update_check: crate::update::Check,
    update_kind: crate::update::Install,
    update_ui: crate::update_ui::UpdateDialog,
    restart_to: Option<PathBuf>,
    /// The reference tab of games (kept across games: its results stay cached).
    reference: crate::reference::Reference,
    allow_quit: bool,
    applied_dark: Option<bool>,
    debug: DebugStartup,
}

/// Dev-only startup automation for screenshots/manual QA (see docs/dev/desktop.md):
/// `CP_OPEN=<game index>` opens a game from the first database, `CP_PLY=<n>` steps
/// along the main line, `CP_ANALYZE=1` starts the engine, `CP_SETTINGS=1` opens settings (`=board`, `=about`: that tab),
/// `CP_REVIEW=1` starts the whole-game analysis (`=dialog` only opens its dialog),
/// `CP_KEYS="ArrowDown Shift+ArrowUp Escape"` presses these keys, one every half second
/// from the second second.
#[derive(Default)]
struct DebugStartup {
    open: Option<usize>,
    ply: usize,
    analyze: bool,
    /// `CP_SCREENSHOT=<file.png>`: go fullscreen, wait `CP_SCREENSHOT_DELAY` seconds,
    /// save the window contents and quit (README screenshots without desktop overlays).
    screenshot: Option<PathBuf>,
    screenshot_delay: f64,
    screenshot_requested: bool,
    open_settings: bool,
    keys: std::collections::VecDeque<(Modifiers, Key)>,
    next_key: f64,
}

impl DebugStartup {
    fn from_env() -> Self {
        let num = |k| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok());
        Self {
            open: num("CP_OPEN"),
            ply: num("CP_PLY").unwrap_or(0),
            analyze: std::env::var("CP_ANALYZE").is_ok(),
            screenshot: std::env::var_os("CP_SCREENSHOT").map(PathBuf::from),
            screenshot_delay: std::env::var("CP_SCREENSHOT_DELAY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4.0),
            screenshot_requested: false,
            open_settings: false,
            keys: std::env::var("CP_KEYS")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|k| {
                    let (shift, name) = match k.strip_prefix("Shift+") {
                        Some(name) => (Modifiers::SHIFT, name),
                        None => (Modifiers::NONE, k),
                    };
                    Key::from_name(name).map(|key| (shift, key))
                })
                .collect(),
            next_key: 2.0,
        }
    }
}

impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut settings = Settings::load();
        // Before the samples, which are named in this language.
        crate::i18n::set_lang(settings.language);
        crate::engine_download::clean_leftovers();
        crate::update::clean_leftovers();
        let settings_check_updates = settings.check_updates;
        // "Updated to …" once, when this version runs for the first time after another.
        let current = env!("CARGO_PKG_VERSION");
        let updated_from = settings
            .last_version
            .clone()
            .filter(|v| crate::update::parse_version(v) < crate::update::parse_version(current));
        if settings.install_id.is_empty() {
            settings.install_id = crate::update::new_install_id();
        }
        crate::update::set_install_id(&settings.install_id);
        if settings.last_version.as_deref() != Some(current) {
            settings.last_version = Some(current.to_owned());
            let _ = settings.save();
        }
        let engines_found = crate::engines::first_run_setup(&mut settings);
        if crate::examples::first_run_setup(&mut settings) || engines_found {
            let _ = settings.save();
        }
        // PGN files passed on the command line (e.g. "Open with…") join the library, first one selected.
        let args: Vec<PathBuf> = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .collect();
        for p in args.iter().rev() {
            let p = p.canonicalize().unwrap_or_else(|_| p.clone());
            settings.touch_database(&p);
        }
        if !args.is_empty() {
            let _ = settings.save();
        }
        let (tx, rx) = channel();
        let mut library = Library::new();
        if let Some(first) = settings.databases.first().filter(|p| p.exists()) {
            library.select_db(first, ctx);
        }
        let mut app = Self {
            settings,
            settings_dirty: false,
            sound: SoundPlayer::new(),
            library,
            game: None,
            analysis: Analysis::new(),
            live_paused: false,
            settings_ui: SettingsWindow::new(),
            toast: None,
            tx,
            rx,
            confirm_close: None,
            close_after_save: None,
            find_after_close: None,
            previous_games: Vec::new(),
            reference_tab: false,
            update_check: if settings_check_updates {
                crate::update::Check::start(ctx)
            } else {
                crate::update::Check::idle()
            },
            update_kind: crate::update::detect(),
            update_ui: crate::update_ui::UpdateDialog::from_env(),
            restart_to: None,
            reference: Default::default(),
            engine_offer: crate::engine_offer::EngineOffer::new(),
            offer_hook_done: false,
            online_probe: crate::online::Probe::start(ctx),
            online: crate::online_ui::OnlineDialog::from_env(),
            translation: Default::default(),
            allow_quit: false,
            applied_dark: None,
            debug: DebugStartup::from_env(),
        }
        .with_debug_settings();
        if let Some(from) = updated_from {
            let msg = trf(
                "ChessPenguin is updated: {from} → {to}",
                &[("from", &from), ("to", &current)],
            );
            app.toast(ctx, msg);
        }
        app
    }

    fn with_debug_settings(mut self) -> Self {
        if let Ok(tab) = std::env::var("CP_SETTINGS") {
            // Opened after a short delay (see debug_screenshot) so a fullscreen switch is done first.
            self.debug.open_settings = true;
            match tab.as_str() {
                "board" => self.settings_ui.tab = Tab::Board,
                "about" => self.settings_ui.tab = Tab::About,
                "translation" => self.settings_ui.tab = Tab::Translation,
                _ => {}
            }
        }
        // `CP_TRANSLATE_SETUP=1`: the "How to translate?" window (`=key`: with an API key chosen).
        if let Ok(v) = std::env::var("CP_TRANSLATE_SETUP") {
            self.translation
                .debug_open_setup(&self.settings.translation, v == "key");
        }
        self
    }

    fn debug_keys(&mut self, ctx: &egui::Context) {
        if self.debug.keys.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        if now >= self.debug.next_key
            && let Some((modifiers, key)) = self.debug.keys.pop_front()
        {
            self.debug.next_key = now + 0.5;
            ctx.input_mut(|i| {
                i.modifiers = modifiers;
                i.events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
            });
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        if self.debug.open_settings && ctx.input(|i| i.time) > 1.5 {
            self.debug.open_settings = false;
            self.settings_ui.open = true;
        }
        let Some(path) = self.debug.screenshot.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if !self.debug.screenshot_requested {
            if ctx.cumulative_frame_nr() == 1 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            }
            if now >= self.debug.screenshot_delay {
                self.debug.screenshot_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Err(e) =
                image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgba8)
            {
                eprintln!("screenshot: {e}");
            }
            self.allow_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn debug_step(&mut self, ctx: &egui::Context) {
        let Some(index) = self.debug.open else { return };
        let Some(path) = self.settings.databases.first().cloned() else {
            return;
        };
        if self.library.database(&path).is_none() {
            return;
        }
        self.debug.open = None;
        self.open_game(ctx, path, index, None);
        if let Some(g) = &mut self.game {
            g.debug_advance(self.debug.ply);
            if std::env::var("CP_ENGINE_PANEL").is_ok() {
                g.debug_open_engine_settings();
            }
            if std::env::var("CP_REPORT").is_ok() {
                g.debug_show_report();
            }
            if let Some(k) = std::env::var("CP_BEST").ok().and_then(|v| v.parse().ok()) {
                g.debug_best_line(k);
            }
            // CP_REVIEW=dialog opens the game-analysis dialog, any other value starts the analysis.
            if let Ok(v) = std::env::var("CP_REVIEW") {
                g.debug_review(&self.settings, ctx, v != "dialog");
            }
        }
        if self.debug.analyze {
            self.toggle_engine(ctx);
        }
    }

    fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.toast = Some((msg.into(), ctx.input(|i| i.time) + 3.5));
    }

    fn play(&self, s: Sound) {
        if self.settings.sound_enabled {
            self.sound
                .play(self.settings.sound_set, s, self.settings.volume);
        }
    }

    fn spawn_dialog(
        &self,
        ctx: &egui::Context,
        f: impl FnOnce() -> Option<Async> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("dialog".into())
            .spawn(move || {
                if let Some(r) = f() {
                    let _ = tx.send(r);
                }
                ctx.request_repaint();
            });
    }

    fn pick_databases(&self, ctx: &egui::Context) {
        self.spawn_dialog(ctx, || {
            rfd::FileDialog::new()
                .set_title(tr("Open database"))
                .add_filter(
                    tr("Chess databases"),
                    &[
                        "pgn", "PGN", "cbh", "CBH", "cbv", "CBV", "zip", "ZIP", "7z", "7Z",
                    ],
                )
                .add_filter("PGN", &["pgn", "PGN"])
                .add_filter(tr("All files"), &["*"])
                .pick_files()
                .map(Async::DatabasesPicked)
        });
    }

    fn pick_save(&self, ctx: &egui::Context) {
        let Some(g) = &self.game else { return };
        let name = sanitize(&g.title()) + ".pgn";
        self.spawn_dialog(ctx, move || {
            rfd::FileDialog::new()
                .set_title(tr("Save game"))
                .set_file_name(name)
                .add_filter("PGN", &["pgn"])
                .save_file()
                .map_or(Some(Async::SaveCancelled), |p| Some(Async::SavePicked(p)))
        });
    }

    fn pick_engine(&mut self, ctx: &egui::Context) {
        self.settings_ui.checking_engine = true;
        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("engine-check".into())
            .spawn(move || {
                let Some(path) = rfd::FileDialog::new()
                    .set_title(tr("Choose the UCI engine executable"))
                    .pick_file()
                else {
                    let _ = tx.send(Async::EngineChecked(Err(String::new())));
                    ctx2.request_repaint();
                    return;
                };
                let res = uci::Engine::spawn(&path, std::iter::empty::<&str>(), Box::new(|| {}))
                    .map(|h| {
                        let name = h.info().name.clone().unwrap_or_else(|| {
                            path.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        });
                        h.quit();
                        (path.clone(), name)
                    })
                    .map_err(|e| format!("{}: {e}", path.display()));
                let _ = tx.send(Async::EngineChecked(res));
                ctx2.request_repaint();
            });
    }

    fn handle_async(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Async::DatabasesPicked(paths) => {
                    for p in paths.iter().rev() {
                        self.settings.touch_database(p);
                    }
                    if let Some(first) = paths.first() {
                        self.library.select_db(first, ctx);
                    }
                    self.settings_dirty = true;
                }
                Async::SavePicked(path) => self.save_game(ctx, path),
                Async::SaveCancelled => {
                    self.close_after_save = None;
                    self.restart_to = None;
                }
                Async::EngineChecked(res) => {
                    self.settings_ui.checking_engine = false;
                    match res {
                        Ok((path, name)) => {
                            self.settings
                                .engines
                                .push(EngineConfig::new(name.clone(), path));
                            if self.settings.active_engine.is_none() {
                                self.settings.active_engine = Some(self.settings.engines.len() - 1);
                            }
                            self.settings_dirty = true;
                            self.toast(ctx, trf("Engine “{name}” added", &[("name", &name)]));
                        }
                        Err(e) if e.is_empty() => {}
                        Err(e) => self.toast(
                            ctx,
                            trf("Not a UCI engine or it failed to start: {e}", &[("e", &e)]),
                        ),
                    }
                }
            }
        }
    }

    fn save_game(&mut self, ctx: &egui::Context, path: PathBuf) {
        // Never replace a source database with a single game (explicit invariant).
        let same = |a: &std::path::Path| match (a.canonicalize(), path.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == path,
        };
        let is_source = self
            .game
            .as_ref()
            .and_then(|g| g.source.as_ref())
            .is_some_and(|s| same(&s.path));
        if is_source || self.settings.databases.iter().any(|d| same(d)) {
            self.close_after_save = None;
            self.toast(
                ctx,
                tr("An open database cannot be overwritten with a single game — choose another file"),
            );
            return;
        }
        let Some(g) = &mut self.game else { return };
        let pgn = g.game.to_pgn();
        match write_atomic(&path, pgn.as_bytes()) {
            Ok(()) => {
                g.game.mark_saved();
                self.toast(ctx, trf("Saved: {path}", &[("path", &path.display())]));
                if let Some(intent) = self.close_after_save.take() {
                    self.finish_close(ctx, intent);
                }
            }
            Err(e) => {
                self.close_after_save = None;
                self.toast(ctx, trf("Save failed: {e}", &[("e", &e)]));
            }
        }
    }

    fn open_game(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        index: usize,
        at: Option<shakmaty::Chess>,
    ) {
        let Some(db) = self.library.database(&path) else {
            return;
        };
        match db.load_game(index) {
            Some(parsed) => {
                self.game = Some(GameView::new(
                    parsed.game,
                    Some(GameSource {
                        path: path.clone(),
                        index,
                    }),
                    parsed.diagnostics,
                ));
                if self.reference_tab
                    && let Some(g) = self.game.as_mut()
                {
                    g.show_reference_tab();
                }
                if let (Some(target), Some(g)) = (at, self.game.as_mut()) {
                    g.goto_position(&target);
                }
                self.library.mark_viewed(&path, index);
                self.settings.touch_database(&path);
                self.settings_dirty = true;
            }
            None => self.toast(ctx, tr("Could not read the game")),
        }
    }

    fn request_close(&mut self, ctx: &egui::Context, intent: CloseIntent) {
        if self.game.as_ref().is_some_and(|g| g.game.is_modified()) {
            self.confirm_close = Some(intent);
        } else {
            self.finish_close(ctx, intent);
        }
    }

    fn finish_close(&mut self, ctx: &egui::Context, intent: CloseIntent) {
        self.analysis.stop();
        self.live_paused = false;
        let closed = self.game.take();
        if intent == CloseIntent::ReturnToPrevious {
            self.game = self.previous_games.pop();
            return;
        }
        // Games left for others (reference tab) with unsaved edits are asked about one
        // by one (each shown with the question) before the library, the search board or
        // quitting.
        if let Some(k) = self
            .previous_games
            .iter()
            .rposition(|g| g.game.is_modified())
        {
            self.previous_games.truncate(k + 1);
            self.game = self.previous_games.pop();
            // Ask about it right away; its answer continues to where the user was going
            // (the library, the search board, quitting).
            self.confirm_close = Some(intent);
            return;
        }
        self.previous_games.clear();
        if let Some(g) = closed
            && let Some(src) = g.source
        {
            self.library.reveal(&src.path, src.index);
        }
        if let Some((path, start, line, flipped)) = self.find_after_close.take() {
            self.library.show_position(&path, start, line, flipped, ctx);
        }

        if intent == CloseIntent::QuitApp {
            // A restart into the new version: start it as this one goes.
            if let Some(exe) = self.restart_to.take() {
                // The settings first (the new process reads them), and none of the dev
                // hooks of this one (they would start the update again).
                let _ = self.settings.save();
                let mut cmd = std::process::Command::new(exe);
                for (k, _) in std::env::vars_os() {
                    if k.to_string_lossy().starts_with("CP_") {
                        cmd.env_remove(k);
                    }
                }
                let _ = cmd.spawn();
            }
            self.allow_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Leaves the game for the library's search board with the game's current position.
    fn find_position(&mut self, ctx: &egui::Context, db: Option<PathBuf>) {
        let Some((start, line, flipped)) = self.game.as_ref().and_then(|g| g.search_line()) else {
            return;
        };
        let path = db
            .or_else(|| self.library.selected_db.clone())
            .or_else(|| self.settings.databases.first().cloned());
        let Some(path) = path else {
            self.toast(ctx, tr("Add a database first"));
            return;
        };
        self.find_after_close = Some((path, start, line, flipped));
        self.request_close(ctx, CloseIntent::BackToLibrary);
    }

    fn toggle_engine(&mut self, ctx: &egui::Context) {
        self.live_paused = false;
        if self.analysis.is_on() {
            self.analysis.stop();
            return;
        }
        match self.settings.active_engine().cloned() {
            Some(cfg) if crate::engine_offer::EngineOffer::engine_ready(&self.settings) => {
                self.analysis.start(&cfg, ctx)
            }
            _ => self
                .engine_offer
                .ask(crate::engine_offer::Pending::LiveAnalysis, &self.settings),
        }
    }

    /// Makes ChessPenguin's Stockfish (just downloaded) or a found engine the active
    /// one, then runs what the user was doing.
    fn engine_arrived(
        &mut self,
        ctx: &egui::Context,
        cfg: crate::settings::EngineConfig,
        pending: Option<crate::engine_offer::Pending>,
    ) {
        let known = self.settings.engines.iter().position(|e| {
            (cfg.builtin && e.builtin) || (!cfg.builtin && !e.builtin && e.path == cfg.path)
        });
        let index = known.unwrap_or_else(|| {
            self.settings.engines.push(cfg);
            self.settings.engines.len() - 1
        });
        self.settings.active_engine = Some(index);
        self.settings_dirty = true;
        match pending {
            Some(crate::engine_offer::Pending::LiveAnalysis) => {
                if !self.analysis.is_on() {
                    self.toggle_engine(ctx);
                }
            }
            Some(crate::engine_offer::Pending::GameAnalysis) => {
                let actions = match self.game.as_mut() {
                    Some(g) => g.start_review_now(&self.settings, ctx),
                    None => Vec::new(),
                };
                for a in actions {
                    match a {
                        GameAction::PauseLive if self.analysis.is_on() => {
                            self.analysis.stop();
                            self.live_paused = true;
                        }
                        GameAction::Toast(m) => self.toast(ctx, m),
                        _ => {}
                    }
                }
            }
            None => {}
        }
    }

    fn offer_events(&mut self, ctx: &egui::Context) {
        for e in self.engine_offer.show(ctx) {
            match e {
                crate::engine_offer::OfferEvent::Installed(pending) => {
                    self.engine_arrived(ctx, crate::engines::builtin_config(), pending)
                }
                crate::engine_offer::OfferEvent::UseSystem(path, pending) => {
                    let cfg = crate::settings::EngineConfig::new("Stockfish".into(), path);
                    self.engine_arrived(ctx, cfg, pending)
                }
                crate::engine_offer::OfferEvent::ChooseFile => self.pick_engine(ctx),
                crate::engine_offer::OfferEvent::Toast(m) => self.toast(ctx, m),
            }
        }
    }

    fn paste_pgn(&mut self, ctx: &egui::Context) {
        let text = arboard::Clipboard::new().and_then(|mut c| c.get_text());
        match text {
            Ok(t) if !t.trim().is_empty() => self.open_pgn_text(ctx, t, false),
            _ => self.toast(ctx, tr("The clipboard is empty or unavailable")),
        }
    }

    /// Opens PGN text (the first game of it) as a new game.
    /// `at_end`: show the position after the last main-line move (a line from the
    /// search board continues where the user stopped).
    fn open_pgn_text(&mut self, ctx: &egui::Context, t: String, at_end: bool) {
        // Split into records first: several games must not be glued into one.
        let never = AtomicBool::new(false);
        let db = PgnDatabase::from_bytes(
            t.into_bytes(),
            Some(TextEncoding::Utf8),
            &mut |_| {},
            &never,
        );
        let (parsed, count) = match db.as_ref().ok().filter(|db| !db.is_empty()) {
            Some(db) => (db.load_game(0), db.len()),
            None => (None, 0),
        };
        let Some(parsed) = parsed else {
            self.toast(ctx, tr("The clipboard contains no recognizable PGN game"));
            return;
        };
        if parsed.game.main_child(parsed.game.root()).is_none()
            && parsed.game.tags().len() <= 7
            && !parsed.diagnostics.is_empty()
        {
            self.toast(ctx, tr("The clipboard contains no recognizable PGN game"));
            return;
        }
        if count > 1 {
            self.toast(ctx, trf("The clipboard has {count} games — the first one is open. To work with all of them, save them to a .pgn file and add it as a database", &[("count", &count)]));
        }
        let mut view = GameView::new(parsed.game, None, parsed.diagnostics);
        if at_end {
            view.goto_main_line_end();
        }
        self.game = Some(view);
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(intent) = self.confirm_close else {
            return;
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("confirm_close")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr("Game modified"));
            if let Some(g) = &self.game {
                ui.label(RichText::new(g.title()).strong());
            }
            ui.label(tr(
                "The source PGN is not changed. Save the changes to a separate file?",
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Save as…")).clicked() {
                    choice = Some(0);
                }
                // Escape goes on where the user was going back to (the owner's choice):
                // the second Escape of "Escape, Escape" leaves the game without saving.
                // Quitting the program is not undone that lightly: there it is Cancel.
                let leaving = intent != CloseIntent::QuitApp;
                let esc = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
                let dont = ui.button(tr("Don't save"));
                let dont = if leaving {
                    dont.on_hover_text("Esc")
                } else {
                    dont
                };
                if dont.clicked() || (esc && leaving) {
                    choice = Some(1);
                }
                let cancel = ui.button(tr("Cancel"));
                let cancel = if leaving {
                    cancel
                } else {
                    cancel.on_hover_text("Esc")
                };
                if cancel.clicked() || (esc && !leaving) {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                self.confirm_close = None;
                self.close_after_save = Some(intent);
                self.pick_save(ctx);
            }
            Some(1) => {
                self.confirm_close = None;
                self.finish_close(ctx, intent);
            }
            Some(2) => {
                self.confirm_close = None;
                self.find_after_close = None;
                // A restart asked for waits for no later quit.
                self.restart_to = None;
            }
            _ => {}
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::san::set(self.settings.move_notation);
        // The window has shown its first frame: this graphics mode works.
        if ui.ctx().cumulative_frame_nr() >= 1 {
            crate::graphics::started();
        }
        let ctx = ui.ctx().clone();
        if self.applied_dark != Some(self.settings.dark_mode) {
            ctx.set_visuals(if self.settings.dark_mode {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            self.applied_dark = Some(self.settings.dark_mode);
        }
        // Quitting with a modified game left for another (reference tab): show that one,
        // so the question "save?" is about the game that has the changes.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_quit
            && !self.game.as_ref().is_some_and(|g| g.game.is_modified())
            && let Some(k) = self
                .previous_games
                .iter()
                .rposition(|g| g.game.is_modified())
        {
            self.previous_games.truncate(k + 1);
            self.game = self.previous_games.pop();
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_quit
            && self.game.as_ref().is_some_and(|g| g.game.is_modified())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = Some(CloseIntent::QuitApp);
        }
        self.handle_async(&ctx);
        // Dev hook (screenshots): the engine offer as on pressing L without an engine.
        if std::env::var("CP_ENGINE_OFFER").is_ok() && !self.offer_hook_done {
            self.offer_hook_done = true;
            self.engine_offer
                .ask(crate::engine_offer::Pending::LiveAnalysis, &self.settings);
            if std::env::var("CP_ENGINE_OFFER").as_deref() == Ok("download") {
                self.engine_offer.start(&ctx);
            }
        }
        self.offer_events(&ctx);
        self.debug_step(&ctx);
        self.debug_keys(&ctx);
        self.debug_screenshot(&ctx);
        self.analysis.poll();

        let ctrl = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::O))) {
            self.pick_databases(&ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::N))) && self.game.is_none() {
            self.game = Some(GameView::new(Game::new(), None, Vec::new()));
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::Comma))) {
            self.settings_ui.open = !self.settings_ui.open;
        }

        if let Some(game) = &mut self.game {
            let input_enabled = self.confirm_close.is_none();
            // A database opened for the reference tab arrives here (the library is
            // not drawn while a game is shown).
            self.library.poll();
            self.reference.dbs = self
                .settings
                .databases
                .iter()
                .filter(|p| p.exists())
                .map(|p| crate::reference::RefDb {
                    path: p.clone(),
                    name: p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    db: self.library.database(p),
                    failed: self.library.failure(p).map(str::to_owned),
                    bytes: self.library.file_size(p),
                })
                .collect();
            let actions = game.show(
                ui,
                &mut self.settings,
                &mut self.analysis,
                &mut self.reference,
                &mut self.translation,
                input_enabled,
            );
            self.reference_tab = game.reference_tab();
            for a in actions {
                match a {
                    GameAction::Back => self.request_close(&ctx, CloseIntent::BackToLibrary),
                    GameAction::FindPosition(db) => self.find_position(&ctx, db),
                    GameAction::ReferenceChoose(p) => {
                        self.settings.reference_db = Some(p);
                        self.settings_dirty = true;
                    }
                    GameAction::ReferenceLoad(p) => self.library.load(&p, &ctx),
                    GameAction::OpenGame(path, index, at) => {
                        if let Some(mut prev) = self.game.take() {
                            let title = prev.title();
                            prev.suspend();
                            self.previous_games.push(prev);
                            self.open_game(&ctx, path, index, Some(at));
                            match &mut self.game {
                                Some(g) => {
                                    g.previous = Some(title);
                                    g.show_reference_tab();
                                }
                                // Not opened: back to where we were.
                                None => self.game = self.previous_games.pop(),
                            }
                        }
                    }
                    GameAction::OpenUpdate => self.update_ui.open = true,
                    GameAction::ReturnToPrevious => {
                        if !self.previous_games.is_empty() {
                            self.request_close(&ctx, CloseIntent::ReturnToPrevious);
                        }
                    }
                    GameAction::NeedEngine => self
                        .engine_offer
                        .ask(crate::engine_offer::Pending::GameAnalysis, &self.settings),
                    GameAction::Sound(s) => self.play(s),
                    GameAction::CopyText(text, msg) => {
                        ctx.copy_text(text);
                        self.toast(&ctx, msg);
                    }
                    GameAction::SaveAs => self.pick_save(&ctx),
                    GameAction::ToggleEngine => self.toggle_engine(&ctx),
                    GameAction::EngineReconfigure => {
                        self.settings_dirty = true;
                        if self.analysis.is_on()
                            && let Some(cfg) = self.settings.active_engine().cloned()
                        {
                            self.analysis.reconfigure(&cfg, &ctx);
                        }
                    }
                    GameAction::EngineSwitch => {
                        self.settings_dirty = true;
                        if self.analysis.is_on()
                            && let Some(cfg) = self.settings.active_engine().cloned()
                        {
                            self.analysis.start(&cfg, &ctx);
                        }
                    }
                    GameAction::OpenEngineSettings => {
                        self.settings_ui.open = true;
                        self.settings_ui.tab = Tab::Engines;
                    }
                    GameAction::OpenSettings => self.settings_ui.open = !self.settings_ui.open,
                    GameAction::OpenTranslationSettings => {
                        self.settings_ui.open = true;
                        self.settings_ui.tab = Tab::Translation;
                    }
                    GameAction::Toast(m) => self.toast(&ctx, m),
                    GameAction::PauseLive => {
                        if self.analysis.is_on() {
                            self.analysis.stop();
                            self.live_paused = true;
                        }
                    }
                    GameAction::ResumeLive => {
                        if std::mem::take(&mut self.live_paused)
                            && let Some(cfg) = self.settings.active_engine().cloned()
                        {
                            self.analysis.start(&cfg, &ctx);
                        }
                    }
                }
            }
        } else {
            let dbs = self.settings.databases.clone();
            self.online_probe.poll();
            self.library.online = self.online_probe.any();
            self.library.online_busy = self.online.busy();
            self.library.online_running = self.online.running();
            for a in self.library.show(ui, &dbs, &self.settings) {
                match a {
                    LibraryAction::OpenGame { path, index, at } => {
                        self.open_game(&ctx, path, index, at)
                    }
                    LibraryAction::Sound(s) => self.play(s),
                    LibraryAction::CopyText(text, msg) => {
                        ctx.copy_text(text);
                        self.toast(&ctx, msg);
                    }
                    LibraryAction::OpenPgn(text) => self.open_pgn_text(&ctx, text, true),
                    LibraryAction::NewGame => {
                        self.game = Some(GameView::new(Game::new(), None, Vec::new()))
                    }
                    LibraryAction::PastePgn => self.paste_pgn(&ctx),
                    LibraryAction::AddDatabases => self.pick_databases(&ctx),
                    LibraryAction::OnlineGames => self.online.open(),
                    LibraryAction::UpdateOnline(p) => self.online.update(&p, &ctx),
                    LibraryAction::OpenUpdate => self.update_ui.open = true,
                    LibraryAction::RemoveDatabase(p) => {
                        self.settings.databases.retain(|d| *d != p);
                        self.library.forget(&p);
                        self.settings_dirty = true;
                    }
                    LibraryAction::OpenSettings => self.settings_ui.open = true,
                }
            }
        }

        let events = self.online.show(&ctx, &self.online_probe);
        for e in events {
            match e {
                crate::online_ui::OnlineEvent::Added(p) => {
                    let known = self.settings.databases.contains(&p);
                    self.settings.touch_database(&p);
                    if known {
                        self.library.reload(&p, &ctx);
                    }
                    self.library.select_db(&p, &ctx);
                    self.settings_dirty = true;
                }
                crate::online_ui::OnlineEvent::Updated(p) => self.library.reload(&p, &ctx),
                crate::online_ui::OnlineEvent::Open(p) => {
                    self.settings.touch_database(&p);
                    self.library.select_db(&p, &ctx);
                    self.settings_dirty = true;
                }
                crate::online_ui::OnlineEvent::Message(t) => self.toast(&ctx, t),
            }
        }
        // Updates: the badge next to Settings, the window, the status in Settings → About.
        self.update_check.poll();
        let skipped = self.settings.skipped_version.clone();
        let badge = self
            .update_check
            .newer()
            .filter(|r| self.update_ui.active() || skipped.as_deref() != Some(r.version.as_str()))
            .map(|r| self.update_ui.badge(r));
        self.library.update = badge.clone();
        if let Some(g) = &mut self.game {
            g.update = badge;
        }
        for e in self
            .update_ui
            .show(&ctx, self.update_check.newer(), &self.update_kind)
        {
            match e {
                crate::update_ui::UpdateEvent::Restart(exe) => {
                    self.restart_to = Some(exe);
                    self.request_close(&ctx, CloseIntent::QuitApp);
                }
                crate::update_ui::UpdateEvent::CopyText(t) => {
                    ctx.copy_text(t);
                    self.toast(&ctx, tr("Copied"));
                }
                crate::update_ui::UpdateEvent::Quit => {
                    self.request_close(&ctx, CloseIntent::QuitApp);
                }
                crate::update_ui::UpdateEvent::Skip(v) => {
                    self.settings.skipped_version = Some(v);
                    self.settings_dirty = true;
                }
            }
        }
        self.settings_ui.update_available = self.update_check.newer().map(|r| r.version.clone());
        self.settings_ui.update_status = if self.update_check.running() {
            Some(tr("Checking…").to_owned())
        } else if let Some(r) = self.update_check.newer() {
            Some(trf("Version {v} is out", &[("v", &r.version)]))
        } else if self.update_check.release.is_some() {
            Some(tr("This is the latest version").to_owned())
        } else if self.update_check.error.is_some() {
            Some(tr("Could not check for updates").to_owned())
        } else {
            None
        };

        self.settings_ui.download = self.engine_offer.stage();
        self.settings_dirty |= self.translation.poll(&ctx, &mut self.settings);
        self.settings_dirty |= self.translation.windows(&ctx, &mut self.settings);
        if std::mem::take(&mut self.translation.open_settings) {
            self.settings_ui.open = true;
            self.settings_ui.tab = Tab::Translation;
        }
        let (actions, changed) =
            self.settings_ui
                .show(&ctx, &mut self.settings, &mut self.translation);
        self.settings_dirty |= changed;
        for a in actions {
            match a {
                SettingsAction::AddEngine => self.pick_engine(&ctx),
                SettingsAction::CheckUpdates => {
                    self.update_check = self.update_check.again(&ctx);
                }
                SettingsAction::OpenUpdate => self.update_ui.open = true,
                SettingsAction::DownloadEngine => self.engine_offer.start(&ctx),
                SettingsAction::CancelDownload => self.engine_offer.cancel(),
                SettingsAction::RemoveDownloaded => {
                    if let Err(e) = crate::engine_download::remove() {
                        self.toast(&ctx, e.to_string());
                    }
                    if self.settings.active_engine().is_some_and(|e| e.builtin)
                        && !crate::engine_offer::EngineOffer::engine_ready(&self.settings)
                    {
                        self.analysis.stop();
                    }
                }
                SettingsAction::TestSound => {
                    if self.settings.sound_enabled {
                        self.sound
                            .play(self.settings.sound_set, Sound::Move, self.settings.volume);
                    }
                }
                SettingsAction::EngineChanged => {
                    if self.analysis.is_on() {
                        self.analysis.stop();
                        if let Some(cfg) = self.settings.active_engine().cloned() {
                            self.analysis.start(&cfg, &ctx);
                        }
                    }
                }
            }
        }

        self.confirm_dialog(&ctx);

        if let Some((msg, until)) = &self.toast {
            let now = ctx.input(|i| i.time);
            if now > *until {
                self.toast = None;
            } else {
                egui::Area::new(egui::Id::new(TOAST))
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(RichText::new(msg.as_str()).size(14.0));
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }

        if self.settings_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.settings_dirty = false;
            if let Err(e) = self.settings.save() {
                eprintln!("settings: save failed: {e}");
            }
        }
    }

    fn on_exit(&mut self) {
        self.analysis.stop();
        // Downloads of online games stop and save what they got; a new database goes on
        // the shelf with it (the next download continues it).
        for p in self.online.stop_all() {
            if crate::online::State::load(&p).is_some_and(|s| s.games > 0) {
                self.settings.touch_database(&p);
            }
        }
        let _ = self.settings.save();
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '—' | '–' => '-',
            c if c.is_alphanumeric() || " -.,".contains(c) => c,
            _ => '_',
        })
        .collect::<String>()
}
