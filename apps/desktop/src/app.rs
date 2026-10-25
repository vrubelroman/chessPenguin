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
use crate::own_db::{
    self, Answer, ChooserFor, ChooserOutcome, EditorMode, EditorOutcome, NewBaseOutcome,
    QuestionKind,
};
use crate::settings::{EngineConfig, Settings};
use crate::settings_ui::{SettingsAction, SettingsWindow, Tab};
use crate::sound::SoundPlayer;

/// Results of work done off the UI thread (file dialogs, engine checks).
enum Async {
    DatabasesPicked(Vec<PathBuf>),
    SavePicked(PathBuf),
    SaveCancelled,
    EngineChecked(Result<(PathBuf, String), String>),
    /// The folder for new databases (`None`: the dialog was cancelled).
    FolderPicked(Option<PathBuf>),
}

/// A passing message at the bottom; `undo`: with the button that puts a deleted game back.
struct Toast {
    text: String,
    until: f64,
    undo: bool,
}

/// The dialogs and the pending steps of the user's own databases (own_db.rs).
#[derive(Default)]
struct Own {
    new_base: Option<own_db::NewBaseDialog>,
    chooser: Option<own_db::Chooser>,
    editor: Option<(own_db::DataEditor, EditorThen)>,
    question: Option<(own_db::Question, QuestionThen)>,
    /// The advice about a big database was given this session.
    big_advised: std::collections::HashSet<PathBuf>,
    /// Waits for its database to be read.
    pending: Option<Pending>,
    /// A deleted record that can be put back (the toast's Undo, Ctrl+Z).
    undo: Option<UndoDelete>,
    /// Leaving the game once it is saved or added (the question about unsaved edits led
    /// to the save or the chooser).
    then_close: Option<CloseIntent>,
}

enum EditorThen {
    Done,
    Add(PathBuf),
}

enum QuestionThen {
    Duplicate {
        target: PathBuf,
        index: usize,
    },
    /// A record of a table is already in the target (record `at`).
    DuplicateRecord {
        from: PathBuf,
        index: usize,
        target: PathBuf,
        at: usize,
    },
    Delete {
        path: PathBuf,
        index: usize,
    },
    /// Make the copy (asked from the card's menu); its card is then selected.
    Utf8Copy {
        path: PathBuf,
        dest: PathBuf,
    },
    /// The write failed: the same write again.
    Retry(Retry),
    /// The file changed on the disk: the game can only go in as a new record.
    Changed(Retry),
    BigBase,
}

/// A step that waits for a database to be read (it was just created, reloaded after a
/// write, or not opened yet).
enum Pending {
    AddCurrent(PathBuf),
    AppendRecord {
        from: PathBuf,
        index: usize,
        target: PathBuf,
    },
    Replace,
    Undo,
}

impl Pending {
    fn path(&self, game: Option<&GameView>, undo: Option<&UndoDelete>) -> Option<PathBuf> {
        match self {
            Pending::AddCurrent(p) => Some(p.clone()),
            Pending::AppendRecord { target, .. } => Some(target.clone()),
            Pending::Replace => game.and_then(|g| g.source.as_ref()).map(|s| s.path.clone()),
            Pending::Undo => undo.map(|u| u.path.clone()),
        }
    }
}

/// What to do again after a failed write.
#[derive(Clone)]
enum Retry {
    Replace,
    /// The duplicate's record in `target` is replaced.
    ReplaceIn {
        target: PathBuf,
        index: usize,
    },
    Append(PathBuf),
    AppendRecord {
        from: PathBuf,
        index: usize,
        target: PathBuf,
    },
    Delete {
        path: PathBuf,
        index: usize,
    },
}

struct UndoDelete {
    path: PathBuf,
    removed: chess_core::pgn::store::Removed,
    /// The open game was this record: it gets its source back with the record.
    open: Option<GameSource>,
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

/// What starts once the played game is closed.
enum PlayNext {
    Rematch(crate::play::Start),
    /// The play dialog again (from the same position, for a game from a position).
    Dialog(Option<Box<crate::play::StartFrom>>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum CloseIntent {
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
    toast: Option<Toast>,
    own: Own,
    /// The play dialog («Play against the computer»).
    play_dialog: Option<crate::play::Dialog>,
    /// After the current game is closed: a new game against Maia (a rematch, or the
    /// dialog for a new one).
    play_next: Option<PlayNext>,
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
/// `CP_REVIEW=1` starts the whole-game analysis (`=dialog` only opens its dialog, `=ask`
/// presses «Analyse»: the dialog or, the options chosen before, the analysis),
/// `CP_KEYS="ArrowDown Shift+ArrowUp Ctrl+Z Escape"` presses these keys, one every half second
/// from the second second.
#[derive(Default)]
struct DebugStartup {
    open: Option<usize>,
    /// `CP_PLAY_FROM=white|black`: the game against Maia from the move reached.
    play_from: Option<crate::play::Start>,
    ply: usize,
    analyze: bool,
    /// `CP_SCREENSHOT=<file.png>`: go fullscreen, wait `CP_SCREENSHOT_DELAY` seconds,
    /// save the window contents and quit (README screenshots without desktop overlays).
    screenshot: Option<PathBuf>,
    screenshot_delay: f64,
    screenshot_requested: bool,
    open_settings: bool,
    /// `CP_OWN=new|chooser|data|save|delete`: that dialog of the own databases
    /// (own_db.rs) after a short delay; `delete` and `chooser` without a game: the
    /// record `CP_OPEN` (default 0) of the first database.
    own: Option<String>,
    /// `CP_PLAY`: a game against Maia at start (see `debug_screenshot`).
    play: Option<String>,
    keys: std::collections::VecDeque<(Modifiers, Key)>,
    next_key: f64,
    /// `CP_TYPE=<text>`: typed into the field that has the keyboard, 2.5 s after start.
    typed: Option<String>,
}

impl DebugStartup {
    fn from_env() -> Self {
        let num = |k| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok());
        Self {
            open: num("CP_OPEN"),
            play_from: None,
            ply: num("CP_PLY").unwrap_or(0),
            analyze: std::env::var("CP_ANALYZE").is_ok(),
            screenshot: std::env::var_os("CP_SCREENSHOT").map(PathBuf::from),
            screenshot_delay: std::env::var("CP_SCREENSHOT_DELAY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4.0),
            screenshot_requested: false,
            open_settings: false,
            own: std::env::var("CP_OWN").ok(),
            play: std::env::var("CP_PLAY").ok(),
            keys: std::env::var("CP_KEYS")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|k| {
                    // "Shift+ArrowUp", "Ctrl+Z", "Ctrl+Shift+S".
                    let mut mods = Modifiers::NONE;
                    let mut name = k;
                    loop {
                        if let Some(rest) = name.strip_prefix("Shift+") {
                            mods |= Modifiers::SHIFT;
                            name = rest;
                        } else if let Some(rest) = name.strip_prefix("Ctrl+") {
                            mods |= Modifiers::COMMAND;
                            name = rest;
                        } else {
                            break;
                        }
                    }
                    Key::from_name(name).map(|key| (mods, key))
                })
                .collect(),
            next_key: 2.0,
            typed: std::env::var("CP_TYPE").ok(),
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
            own: Own::default(),
            play_dialog: None,
            play_next: None,
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
                "interface" => self.settings_ui.tab = Tab::Interface,
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
        let now = ctx.input(|i| i.time);
        if now >= 2.5
            && let Some(text) = self.debug.typed.take()
        {
            ctx.input_mut(|i| i.events.push(egui::Event::Text(text)));
        }
        if self.debug.keys.is_empty() {
            return;
        }
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
        // `CP_PLAY=white|black[,5+3|none][,1500]`: a game against Maia starts at once;
        // `CP_PLAY=dialog`: the play dialog.
        if let Some(spec) = self.debug.play.take() {
            if spec == "dialog" {
                self.play_dialog = Some(crate::play::Dialog::new(&self.settings));
            } else {
                let mut parts = spec.split(',');
                let user = if parts.next() == Some("black") {
                    chess_core::shakmaty::Color::Black
                } else {
                    chess_core::shakmaty::Color::White
                };
                let clock = match parts.next() {
                    Some("none") => None,
                    Some(tc) => tc.split_once('+').and_then(|(m, i)| {
                        Some(crate::play::TimeControl {
                            base: m.parse::<u32>().ok()? * 60,
                            increment: i.parse().ok()?,
                        })
                    }),
                    None => Some(crate::play::TimeControl {
                        base: 300,
                        increment: 3,
                    }),
                };
                let strength = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1500);
                self.start_play(
                    ctx,
                    crate::play::Start {
                        user,
                        strength,
                        user_elo: self.settings.rating,
                        user_name: self.settings.your_name.clone(),
                        clock,
                        from: None,
                    },
                );
            }
        }
        if self.debug.own.is_some() && ctx.input(|i| i.time) > 1.5 {
            let what = self.debug.own.take().unwrap_or_default();
            // `CP_PASTE=<pgn text>`: this game is open first (a game from no database).
            if let Ok(text) = std::env::var("CP_PASTE") {
                self.open_pgn_text(ctx, text, false);
            }
            let first = self.settings.databases.first().cloned();
            let index = self.debug.open.unwrap_or(0);
            match (what.as_str(), first) {
                ("new", _) => self.new_base_dialog(own_db::AfterCreate::Shelf),
                ("chooser", _) if self.game.is_some() => {
                    self.open_chooser(ctx, ChooserFor::CurrentGame)
                }
                ("chooser", Some(path)) => {
                    self.open_chooser(ctx, ChooserFor::Record { path, index })
                }
                ("data", _) => self.edit_game_data(),
                // The table's «+ Game» for the first database.
                ("newin", Some(path)) => {
                    self.new_game();
                    if let Some(g) = self.game.as_mut() {
                        g.target = Some(path);
                    }
                }
                ("save", _) => self.save_current(ctx, None),
                ("delete", Some(path)) => self.delete_game(ctx, path, index),
                _ => {}
            }
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
            // `CP_COMMENT=text`: a comment on the move reached, so the game counts as
            // modified (to try saving it).
            if let Ok(text) = std::env::var("CP_COMMENT") {
                g.debug_comment(&text);
            }
            if let Ok(spec) = std::env::var("CP_SHAPE") {
                g.debug_shapes(&spec);
            }
            if std::env::var_os("CP_COMMENT_EDIT").is_some() {
                g.debug_comment_edit();
            }
            if let Ok(text) = std::env::var("CP_PASTE_LINE") {
                g.debug_paste(&text);
            }
            // `CP_PLAY_FROM=dialog`: the play dialog from the move reached;
            // `white|black[,strength]`: the game at once.
            if let Ok(spec) = std::env::var("CP_PLAY_FROM")
                && let Some(from) = g.debug_play_from()
            {
                let mut parts = spec.split(',');
                let side = parts.next().unwrap_or("dialog");
                let strength = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1500);
                let user = match side {
                    "white" => Some(chess_core::shakmaty::Color::White),
                    "black" => Some(chess_core::shakmaty::Color::Black),
                    _ => None,
                };
                match user {
                    Some(user) => {
                        self.debug.play_from = Some(crate::play::Start {
                            user,
                            strength,
                            user_elo: self.settings.rating,
                            user_name: self.settings.your_name.clone(),
                            clock: self.settings.play.clock,
                            from: Some(Box::new(from)),
                        })
                    }
                    None => {
                        self.play_dialog =
                            Some(crate::play::Dialog::new_from(&self.settings, from));
                    }
                }
            }
            if std::env::var("CP_ENGINE_PANEL").is_ok() {
                g.debug_open_engine_settings();
            }
            if std::env::var("CP_REPORT").is_ok() {
                g.debug_show_report();
            }
            if let Some(k) = std::env::var("CP_BEST").ok().and_then(|v| v.parse().ok()) {
                g.debug_best_line(k);
            }
            // CP_REVIEW=dialog opens the game-analysis dialog, =ask presses «Analyse», any
            // other value starts the analysis.
            if let Ok(v) = std::env::var("CP_REVIEW") {
                g.debug_review(&self.settings, ctx, &v);
            }
        }
        if let Some(start) = self.debug.play_from.take() {
            self.start_play(ctx, start);
        }
        // `CP_TRAIN=1`: learning the opening up to the move reached; `CP_TRAIN_MOVES`:
        // moves made in it (UCI).
        if std::env::var_os("CP_TRAIN").is_some()
            && let Some(start) = self.game.as_ref().and_then(|g| g.debug_train())
        {
            self.open_training(start);
            if let (Ok(moves), Some(g)) = (std::env::var("CP_TRAIN_MOVES"), self.game.as_mut()) {
                g.debug_train_moves(&moves);
            }
        }
        if self.debug.analyze {
            self.toggle_engine(ctx);
        }
    }

    fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.toast = Some(Toast {
            text: msg.into(),
            until: ctx.input(|i| i.time) + 3.5,
            undo: false,
        });
    }

    // ----- own databases (docs/design/own-databases.md) --------------------------------

    /// The game on the screen, when there is one.
    fn game_view(&mut self) -> Option<&mut GameView> {
        self.game.as_mut()
    }

    /// Ctrl+S: a game from a PGN that takes games is saved back into it; any other game
    /// is added to a database of the user's choice.
    fn save_current(&mut self, ctx: &egui::Context, then_close: Option<CloseIntent>) {
        // A comment being written goes into the game first.
        if let Some(g) = self.game.as_mut() {
            g.commit_editor();
        }
        let target = self.current_target();
        let Some(g) = &self.game else { return };
        self.own.then_close = then_close;
        match &g.source {
            Some(src) if src.block.is_none() => {
                if !g.game.is_modified() {
                    self.toast(ctx, tr("No changes to save"));
                    self.finish_then_close(ctx);
                    return;
                }
                let path = src.path.clone();
                let bytes = self.library.file_size(&path);
                if bytes > own_db::BIG_REWRITE_BYTES && !self.own.big_advised.contains(&path) {
                    self.own.big_advised.insert(path.clone());
                    self.ask(QuestionKind::BigBase { path, bytes }, QuestionThen::BigBase);
                    return;
                }
                self.replace_current(ctx);
            }
            // A new game meant for a database (the table's «+ Game»): straight there.
            None if let Some(t) = target => self.add_current_to(ctx, t),
            _ => self.open_chooser(ctx, ChooserFor::CurrentGame),
        }
    }

    /// The database the new game on the screen is meant for, while it still takes games
    /// and is on the shelf; else the target is dropped.
    fn current_target(&mut self) -> Option<PathBuf> {
        let g = self.game.as_mut()?;
        let t = g.target.clone()?;
        if g.source.is_some()
            || !self.settings.databases.contains(&t)
            || self.library.write_block(&t).is_some()
        {
            g.target = None;
            return None;
        }
        Some(t)
    }

    /// Writes the game over its record in its own PGN.
    fn replace_current(&mut self, ctx: &egui::Context) {
        let Some(g) = &self.game else { return };
        let Some(src) = g.source.clone() else { return };
        let Some(db) = self.library.database(&src.path) else {
            self.wait_for(ctx, &src.path, Pending::Replace);
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        let text = g.game.to_pgn();
        match chess_core::pgn::store::replace(pgn, src.index, &text) {
            Ok(()) => {
                let name = own_db::base_name(&src.path);
                if let Some(g) = self.game.as_mut() {
                    g.game.mark_saved();
                }
                self.after_written(
                    ctx,
                    &src.path,
                    src.index,
                    trf("Saved to «{name}»", &[("name", &name)]),
                );
            }
            Err(e) => self.write_failed(ctx, &src.path, e, Retry::Replace, false),
        }
    }

    /// The shelf for the chooser: each database with its count and what blocks it.
    fn chooser_rows(&self) -> Vec<own_db::ChooserRow> {
        self.settings
            .databases
            .iter()
            .map(|p| own_db::ChooserRow {
                path: p.clone(),
                name: own_db::base_name(p),
                games: self.library.database(p).map(|d| d.len()),
                block: self.library.write_block(p),
            })
            .collect()
    }

    /// The list of databases to add a game to.
    fn open_chooser(&mut self, ctx: &egui::Context, purpose: ChooserFor) {
        let rows = self.chooser_rows();
        let title = match &purpose {
            ChooserFor::CurrentGame => self.game.as_ref().map(|g| g.title()).unwrap_or_default(),
            ChooserFor::Record { path, index } => self
                .library
                .database(path)
                .and_then(|db| db.load_game(*index))
                .map(|p| GameView::new(p.game, None, Vec::new()).title())
                .unwrap_or_default(),
        };
        let source = self
            .game
            .as_ref()
            .and_then(|g| g.source.as_ref())
            .map(|s| s.path.clone());
        let target = self.current_target();
        let preselect = target.or(self.settings.last_target.clone()).or(source);
        // Databases not read yet are read now, for their counts and encodings.
        for p in self.settings.databases.clone() {
            if self.library.database(&p).is_none() && p.exists() {
                self.library.load(&p, ctx);
            }
        }
        self.own.chooser = Some(own_db::Chooser::new(
            rows,
            preselect.as_deref(),
            purpose,
            title,
        ));
    }

    fn ask(&mut self, kind: QuestionKind, then: QuestionThen) {
        self.own.question = Some((own_db::Question { kind }, then));
    }

    /// The game on the screen goes to `target`: its data are asked once for a game that
    /// is in no database yet, then the duplicate check, then the write.
    fn add_current_to(&mut self, ctx: &egui::Context, target: PathBuf) {
        let Some(g) = &self.game else { return };
        if g.source.is_none() && !g.data_asked {
            let editor = own_db::DataEditor::from_game(
                &g.game,
                &self.settings.your_name,
                EditorMode::Add(target.clone()),
            );
            self.own.editor = Some((editor, EditorThen::Add(target)));
            return;
        }
        self.add_current_now(ctx, target);
    }

    fn add_current_now(&mut self, ctx: &egui::Context, target: PathBuf) {
        let Some(g) = &self.game else { return };
        let Some(db) = self.library.database(&target) else {
            self.wait_for(ctx, &target, Pending::AddCurrent(target.clone()));
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        if let Some(index) = chess_core::pgn::store::find_same_game(pgn, &g.game) {
            let title = g.title();
            self.ask(
                QuestionKind::Duplicate {
                    target: target.clone(),
                    index,
                    title,
                },
                QuestionThen::Duplicate { target, index },
            );
            return;
        }
        self.append_current(ctx, target);
    }

    /// The PGN text of the game on the screen: the original record when it is an
    /// unchanged game of a PGN file, else written anew. A source whose letters are
    /// restored on reading (ч/ю written as `~~`/`o^`) is written anew too: the copy
    /// gets the restored text.
    fn current_pgn(&self) -> Option<String> {
        let g = self.game.as_ref()?;
        if !g.game.is_modified()
            && let Some(src) = &g.source
            && let Some(db) = self.library.database(&src.path)
            && let Some(pgn) = db.as_pgn()
            && !pgn.repair_letters_enabled()
            && let Some(text) = db.raw_text(src.index)
        {
            return Some(text);
        }
        Some(g.game.to_pgn())
    }

    /// The PGN text of record `index` of a database, as `current_pgn` would give it.
    fn record_pgn(&self, from: &std::path::Path, index: usize) -> Option<String> {
        let db = self.library.database(from)?;
        match db.as_pgn() {
            Some(pgn) if !pgn.repair_letters_enabled() => db.raw_text(index),
            _ => db.load_game(index).map(|p| p.game.to_pgn()),
        }
    }

    fn append_current(&mut self, ctx: &egui::Context, target: PathBuf) {
        let Some(text) = self.current_pgn() else {
            return;
        };
        let Some(db) = self.library.database(&target) else {
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        match chess_core::pgn::store::append(pgn, &text) {
            Ok(_) => {
                let index = pgn.len();
                let name = own_db::base_name(&target);
                if let Some(g) = self.game.as_mut() {
                    g.source = Some(GameSource {
                        path: target.clone(),
                        index,
                        block: None,
                    });
                    g.target = None;
                    g.game.mark_saved();
                }
                self.settings.last_target = Some(target.clone());
                self.settings_dirty = true;
                self.after_written(
                    ctx,
                    &target,
                    index,
                    trf(
                        "Added to «{name}» (No. {n})",
                        &[("name", &name), ("n", &(index + 1))],
                    ),
                );
            }
            Err(e) => self.write_failed(ctx, &target, e, Retry::Append(target.clone()), false),
        }
    }

    /// The game on the screen replaces record `index` of `target` (a duplicate).
    fn replace_in(&mut self, ctx: &egui::Context, target: PathBuf, index: usize) {
        let Some(text) = self.current_pgn() else {
            return;
        };
        let Some(db) = self.library.database(&target) else {
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        match chess_core::pgn::store::replace(pgn, index, &text) {
            Ok(()) => {
                let name = own_db::base_name(&target);
                if let Some(g) = self.game.as_mut() {
                    g.source = Some(GameSource {
                        path: target.clone(),
                        index,
                        block: None,
                    });
                    g.game.mark_saved();
                }
                self.settings.last_target = Some(target.clone());
                self.settings_dirty = true;
                self.after_written(
                    ctx,
                    &target,
                    index,
                    trf(
                        "Saved to «{name}» (No. {n})",
                        &[("name", &name), ("n", &(index + 1))],
                    ),
                );
            }
            Err(e) => self.write_failed(
                ctx,
                &target,
                e,
                Retry::ReplaceIn {
                    target: target.clone(),
                    index,
                },
                false,
            ),
        }
    }

    /// A record of a table goes into `target`: the duplicate check first.
    fn append_record(&mut self, ctx: &egui::Context, from: PathBuf, index: usize, target: PathBuf) {
        let Some(db) = self.library.database(&target) else {
            self.wait_for(
                ctx,
                &target,
                Pending::AppendRecord {
                    from,
                    index,
                    target: target.clone(),
                },
            );
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        let Some(parsed) = self
            .library
            .database(&from)
            .and_then(|d| d.load_game(index))
        else {
            self.toast(ctx, tr("Could not read the game"));
            return;
        };
        if let Some(at) = chess_core::pgn::store::find_same_game(pgn, &parsed.game) {
            let title = GameView::new(parsed.game, None, Vec::new()).title();
            self.ask(
                QuestionKind::Duplicate {
                    target: target.clone(),
                    index: at,
                    title,
                },
                QuestionThen::DuplicateRecord {
                    from,
                    index,
                    target,
                    at,
                },
            );
            return;
        }
        self.append_record_now(ctx, from, index, target);
    }

    fn append_record_now(
        &mut self,
        ctx: &egui::Context,
        from: PathBuf,
        index: usize,
        target: PathBuf,
    ) {
        let Some(text) = self.record_pgn(&from, index) else {
            self.toast(ctx, tr("Could not read the game"));
            return;
        };
        let Some(db) = self.library.database(&target) else {
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        match chess_core::pgn::store::append(pgn, &text) {
            Ok(_) => {
                let n = pgn.len();
                let name = own_db::base_name(&target);
                self.settings.last_target = Some(target.clone());
                self.settings_dirty = true;
                self.library.reload(&target, ctx);
                self.toast(
                    ctx,
                    trf(
                        "Added to «{name}» (No. {n})",
                        &[("name", &name), ("n", &(n + 1))],
                    ),
                );
            }
            Err(e) => self.write_failed(
                ctx,
                &target,
                e,
                Retry::AppendRecord {
                    from,
                    index,
                    target: target.clone(),
                },
                false,
            ),
        }
    }

    /// A record of a table replaces record `at` of `target` (its duplicate).
    fn replace_record(
        &mut self,
        ctx: &egui::Context,
        from: PathBuf,
        index: usize,
        target: PathBuf,
        at: usize,
    ) {
        let Some(text) = self.record_pgn(&from, index) else {
            self.toast(ctx, tr("Could not read the game"));
            return;
        };
        let Some(db) = self.library.database(&target) else {
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        match chess_core::pgn::store::replace(pgn, at, &text) {
            Ok(()) => {
                let name = own_db::base_name(&target);
                self.settings.last_target = Some(target.clone());
                self.settings_dirty = true;
                self.library.reload(&target, ctx);
                self.toast(
                    ctx,
                    trf(
                        "Saved to «{name}» (No. {n})",
                        &[("name", &name), ("n", &(at + 1))],
                    ),
                );
            }
            Err(e) => self.write_failed(
                ctx,
                &target,
                e,
                Retry::ReplaceIn {
                    target: target.clone(),
                    index: at,
                },
                false,
            ),
        }
    }

    /// A step that needs the database read: it runs once the file is in (`run_pending`).
    fn wait_for(&mut self, ctx: &egui::Context, path: &std::path::Path, step: Pending) {
        if !self.library.loading(path) && self.library.database(path).is_none() {
            self.library.load(path, ctx);
        }
        self.own.pending = Some(step);
    }

    /// The database a step waits for is read: the step runs. Could not be read: said.
    fn run_pending(&mut self, ctx: &egui::Context) {
        let Some(step) = self.own.pending.as_ref() else {
            return;
        };
        let Some(path) = step.path(self.game.as_ref(), self.own.undo.as_ref()) else {
            self.own.pending = None;
            return;
        };
        if self.library.database(&path).is_some() {
            match self.own.pending.take().expect("pending") {
                Pending::AddCurrent(target) => self.add_current_to(ctx, target),
                Pending::AppendRecord {
                    from,
                    index,
                    target,
                } => self.append_record(ctx, from, index, target),
                Pending::Replace => self.replace_current(ctx),
                Pending::Undo => self.undo_delete(ctx),
            }
        } else if !self.library.loading(&path) {
            self.own.pending = None;
            let why = self.library.failure(&path).unwrap_or("?").to_owned();
            self.toast(ctx, trf("Could not read the database: {e}", &[("e", &why)]));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The file is written: the library reads it again and shows the record.
    fn after_written(
        &mut self,
        ctx: &egui::Context,
        path: &std::path::Path,
        index: usize,
        msg: String,
    ) {
        self.library.reload(path, ctx);
        self.library.reveal(path, index);
        self.toast(ctx, msg);
        self.finish_then_close(ctx);
    }

    fn finish_then_close(&mut self, ctx: &egui::Context) {
        if let Some(intent) = self.own.then_close.take() {
            self.finish_close(ctx, intent);
        }
    }

    fn write_failed(
        &mut self,
        ctx: &egui::Context,
        path: &std::path::Path,
        e: chess_core::pgn::store::WriteError,
        retry: Retry,
        deleting: bool,
    ) {
        use chess_core::pgn::store::WriteError;
        match e {
            WriteError::Changed => {
                self.library.reload(path, ctx);
                self.ask(
                    QuestionKind::ChangedOnDisk {
                        path: path.to_path_buf(),
                        deleting,
                    },
                    QuestionThen::Changed(retry),
                );
            }
            e => self.ask(
                QuestionKind::WriteFailed {
                    path: path.to_path_buf(),
                    error: e.to_string(),
                    deleting,
                },
                QuestionThen::Retry(retry),
            ),
        }
    }

    fn retry(&mut self, ctx: &egui::Context, retry: Retry) {
        match retry {
            Retry::Replace => self.replace_current(ctx),
            Retry::ReplaceIn { target, index } => self.replace_in(ctx, target, index),
            Retry::Append(target) => self.append_current(ctx, target),
            Retry::AppendRecord {
                from,
                index,
                target,
            } => self.append_record(ctx, from, index, target),
            Retry::Delete { path, index } => self.delete_now(ctx, path, index),
        }
    }

    fn delete_game(&mut self, ctx: &egui::Context, path: PathBuf, index: usize) {
        if let Some(block) = self.library.write_block(&path) {
            self.toast(ctx, block.long());
            return;
        }
        let title = self
            .library
            .database(&path)
            .and_then(|db| db.load_game(index))
            .map(|p| GameView::new(p.game, None, Vec::new()).title())
            .unwrap_or_default();
        self.ask(
            QuestionKind::Delete {
                path: path.clone(),
                index,
                title,
            },
            QuestionThen::Delete { path, index },
        );
    }

    fn delete_now(&mut self, ctx: &egui::Context, path: PathBuf, index: usize) {
        let Some(db) = self.library.database(&path) else {
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        match chess_core::pgn::store::remove(pgn, index) {
            Ok(removed) => {
                // Games open from this file: their records moved up by one.
                let mut open = None;
                for g in self.game.iter_mut().chain(self.previous_games.iter_mut()) {
                    if let Some(s) = &mut g.source
                        && s.path == path
                    {
                        if s.index == index {
                            open = g.source.take();
                        } else if s.index > index {
                            s.index -= 1;
                        }
                    }
                }
                self.own.undo = Some(UndoDelete {
                    path: path.clone(),
                    removed,
                    open,
                });
                self.library.reload(&path, ctx);
                self.library.reveal(&path, index);
                self.toast = Some(Toast {
                    text: trf(
                        "Deleted from «{name}»",
                        &[("name", &own_db::base_name(&path))],
                    ),
                    until: ctx.input(|i| i.time) + 6.0,
                    undo: true,
                });
            }
            Err(e) => self.write_failed(
                ctx,
                &path,
                e,
                Retry::Delete {
                    path: path.clone(),
                    index,
                },
                true,
            ),
        }
    }

    /// Puts the last deleted record back (the toast's Undo, Ctrl+Z on the library).
    fn undo_delete(&mut self, ctx: &egui::Context) {
        let Some(undo) = self.own.undo.take() else {
            return;
        };
        let Some(db) = self.library.database(&undo.path) else {
            // The file is still being read after the delete: as soon as it is in.
            let path = undo.path.clone();
            self.own.undo = Some(undo);
            self.wait_for(ctx, &path, Pending::Undo);
            return;
        };
        let Some(pgn) = db.as_pgn() else { return };
        match chess_core::pgn::store::restore(pgn, &undo.removed) {
            Ok(()) => {
                let index = undo.removed.index;
                for g in self.game.iter_mut().chain(self.previous_games.iter_mut()) {
                    if let Some(s) = &mut g.source
                        && s.path == undo.path
                        && s.index >= index
                    {
                        s.index += 1;
                    }
                }
                // The open game was the deleted record: it is that record again.
                if let Some(src) = undo.open
                    && let Some(g) = self.game.as_mut()
                    && g.source.is_none()
                {
                    g.source = Some(src);
                }
                self.library.reload(&undo.path, ctx);
                self.library.reveal(&undo.path, index);
                self.toast(ctx, tr("The game is back"));
            }
            Err(e) => self.toast(ctx, trf("Could not put the game back: {e}", &[("e", &e)])),
        }
    }

    /// A UTF-8 copy of a PGN in a legacy encoding (the card's menu): asked first.
    fn utf8_copy(&mut self, path: PathBuf) {
        let encoding = match self.library.write_block(&path) {
            Some(own_db::Block::Encoding(e)) => e,
            _ => return,
        };
        // "X (UTF-8).pgn", or "X (UTF-8 2).pgn" when that is taken.
        let stem = own_db::base_name(&path);
        let dest = (1..)
            .map(|k| {
                let n = if k == 1 {
                    String::new()
                } else {
                    format!(" {k}")
                };
                path.with_file_name(format!("{stem} (UTF-8{n}).pgn"))
            })
            .find(|p| !p.exists())
            .expect("a free name");
        self.ask(
            QuestionKind::Utf8Copy {
                path: path.clone(),
                dest: dest.clone(),
                encoding,
            },
            QuestionThen::Utf8Copy { path, dest },
        );
    }

    fn new_base_dialog(&mut self, after: own_db::AfterCreate) {
        let dir = own_db::default_base_dir(self.settings.new_database_dir.as_deref());
        self.own.new_base = Some(own_db::NewBaseDialog::new(dir, after));
    }

    /// Makes the empty file and puts it on the shelf, selected and as the database the
    /// next game goes to.
    fn create_base(&mut self, ctx: &egui::Context, path: PathBuf) -> Result<(), String> {
        path.parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
            })
            .and_then(|f| f.sync_all())
            .map_err(|e| trf("Could not create the file: {e}", &[("e", &e)]))?;
        self.settings.touch_database(&path);
        self.settings.last_target = Some(path.clone());
        self.settings_dirty = true;
        self.library.select_db(&path, ctx);
        Ok(())
    }

    /// A new game to enter, with today's date (it is probably being played or entered
    /// now).
    fn new_game(&mut self) {
        let mut g = Game::new();
        g.set_tag("Date", &chrono::Local::now().format("%Y.%m.%d").to_string());
        g.mark_saved();
        self.game = Some(GameView::new(g, None, Vec::new()));
    }

    /// What comes after a game against Maia was closed: a rematch, the dialog.
    fn play_next_now(&mut self, ctx: &egui::Context) {
        match self.play_next.take() {
            Some(PlayNext::Rematch(s)) => self.start_play(ctx, s),
            Some(PlayNext::Dialog(None)) => {
                self.play_dialog = Some(crate::play::Dialog::new(&self.settings))
            }
            Some(PlayNext::Dialog(Some(from))) => {
                self.play_dialog = Some(crate::play::Dialog::new_from(&self.settings, *from))
            }
            None => {}
        }
    }

    /// A game from a position of another game: its moves to that position, the game
    /// it comes from waiting under it (as a game opened from the reference does).
    fn game_from(&mut self, from: &crate::play::StartFrom) {
        let mut g = match from
            .start_fen
            .as_deref()
            .and_then(|f| chess_core::StartPosition::from_fen(f).ok())
        {
            Some(start) => Game::from_start(start),
            None => Game::new(),
        };
        g.set_tag("Date", &chrono::Local::now().format("%Y.%m.%d").to_string());
        if !from.moves.is_empty() {
            let _ = g.add_line(chess_core::NodeId::ROOT, &from.moves);
        }
        g.forget_history();
        g.mark_saved();
        self.open_over(g);
    }

    /// `g` opens over the current game, which waits under it as it is (as a game
    /// opened from the reference does); the engine stops.
    fn open_over(&mut self, g: Game) {
        self.analysis.stop();
        let title = self.game.as_ref().map(|g| g.title());
        if let Some(mut prev) = self.game.take() {
            prev.suspend();
            self.previous_games.push(prev);
        }
        let mut view = GameView::new(g, None, Vec::new());
        view.previous = title;
        self.game = Some(view);
    }

    /// Learning the opening of the current game: the training opens over it.
    fn open_training(&mut self, start: crate::train::TrainStart) {
        let g = match start
            .start_fen
            .as_deref()
            .and_then(|f| chess_core::StartPosition::from_fen(f).ok())
        {
            Some(s) => Game::from_start(s),
            None => Game::new(),
        };
        self.open_over(g);
        if let Some(view) = self.game.as_mut() {
            view.start_training(start);
        }
    }

    /// A new game against Maia. Her model missing (a rematch, a game from a position):
    /// the dialog with the same choices, to download it.
    fn start_play(&mut self, ctx: &egui::Context, start: crate::play::Start) {
        let Ok(maia) = crate::play::model_for(start.strength) else {
            self.play_dialog = Some(crate::play::Dialog::for_start(&self.settings, &start));
            return;
        };
        // The engine judging Maia's resignation: ChessPenguin's Stockfish (its node count
        // and the twelve pawns are Stockfish's measures), else the active engine when its
        // file is there; none — she plays every game to its end.
        let judge = crate::engines::own_stockfish().or_else(|| {
            self.settings
                .active_engine()
                .and_then(|e| e.executable())
                .filter(|p| p.is_file())
        });
        match &start.from {
            Some(from) => self.game_from(from),
            None => self.new_game(),
        }
        if let Some(g) = self.game.as_mut() {
            g.start_play(start, maia, judge, ctx);
        }
    }

    /// The game's data for editing (F2, the title, the menu).
    fn edit_game_data(&mut self) {
        let Some(g) = &self.game else { return };
        let editor =
            own_db::DataEditor::from_game(&g.game, &self.settings.your_name, EditorMode::Done);
        self.own.editor = Some((editor, EditorThen::Done));
    }

    fn pick_folder(&self, ctx: &egui::Context, start: PathBuf) {
        self.spawn_dialog(ctx, move || {
            let picked = rfd::FileDialog::new()
                .set_title(tr("Folder for new databases"))
                .set_directory(start)
                .pick_folder();
            Some(Async::FolderPicked(picked))
        });
    }

    /// Draws the dialog that is open and acts on what was chosen.
    fn own_dialogs(&mut self, ctx: &egui::Context) {
        self.run_pending(ctx);
        if let Some((q, _)) = self.own.question.as_mut()
            && let Some(answer) = q.show(ctx)
        {
            let (_, then) = self.own.question.take().expect("question");
            match (then, answer) {
                (QuestionThen::Duplicate { target, index }, Answer::Primary) => {
                    self.replace_in(ctx, target, index)
                }
                (QuestionThen::Duplicate { target, .. }, Answer::Secondary) => {
                    self.append_current(ctx, target)
                }
                (
                    QuestionThen::DuplicateRecord {
                        from,
                        index,
                        target,
                        at,
                    },
                    Answer::Primary,
                ) => self.replace_record(ctx, from, index, target, at),
                (
                    QuestionThen::DuplicateRecord {
                        from,
                        index,
                        target,
                        ..
                    },
                    Answer::Secondary,
                ) => self.append_record_now(ctx, from, index, target),
                (QuestionThen::Delete { path, index }, Answer::Primary) => {
                    self.delete_now(ctx, path, index)
                }
                (QuestionThen::Utf8Copy { path, dest }, Answer::Primary) => {
                    let Some(db) = self.library.database(&path) else {
                        return;
                    };
                    let Some(pgn) = db.as_pgn() else { return };
                    match chess_core::pgn::store::write_utf8_copy(pgn, &dest) {
                        Ok(()) => {
                            // The copy goes on the shelf right after the original.
                            let at = self
                                .settings
                                .databases
                                .iter()
                                .position(|p| *p == path)
                                .map_or(0, |i| i + 1);
                            self.settings.databases.retain(|p| *p != dest);
                            self.settings
                                .databases
                                .insert(at.min(self.settings.databases.len()), dest.clone());
                            // The copy is the one to add to from now on.
                            self.settings.last_target = Some(dest.clone());
                            self.settings_dirty = true;
                            self.library.select_db(&dest, ctx);
                            self.toast(ctx, trf("Copy made: {path}", &[("path", &dest.display())]));
                        }
                        Err(e) => {
                            self.toast(ctx, trf("Could not make the copy: {e}", &[("e", &e)]))
                        }
                    }
                }
                (QuestionThen::Retry(retry), Answer::Primary) => self.retry(ctx, retry),
                (QuestionThen::Retry(_), Answer::Secondary) => {
                    self.open_chooser(ctx, ChooserFor::CurrentGame)
                }
                // After a change on the disk a record cannot be replaced: the game goes
                // in as a new one ("Add as new"); a delete is only reread ("Reread").
                (QuestionThen::Changed(retry), Answer::Primary) => match retry {
                    Retry::Replace => {
                        if let Some(src) = self.game.as_ref().and_then(|g| g.source.clone()) {
                            self.append_current(ctx, src.path);
                        }
                    }
                    Retry::ReplaceIn { target, .. } | Retry::Append(target) => {
                        self.append_current(ctx, target)
                    }
                    Retry::AppendRecord {
                        from,
                        index,
                        target,
                    } => self.append_record(ctx, from, index, target),
                    Retry::Delete { .. } => {}
                },
                (QuestionThen::Changed(_), Answer::Secondary) => {}
                (QuestionThen::BigBase, Answer::Primary) => self.replace_current(ctx),
                (QuestionThen::BigBase, Answer::Secondary) => {
                    self.open_chooser(ctx, ChooserFor::CurrentGame)
                }
                (_, Answer::Cancel)
                | (QuestionThen::Delete { .. }, _)
                | (QuestionThen::Utf8Copy { .. }, _) => {
                    if let Some(intent) = self.own.then_close.take() {
                        self.confirm_close = Some(intent);
                    }
                }
            }
            return;
        }
        if let Some((editor, _)) = self.own.editor.as_mut()
            && let Some(outcome) = editor.show(ctx)
        {
            let (editor, then) = self.own.editor.take().expect("editor");
            match (outcome, then) {
                (EditorOutcome::Done, EditorThen::Done) => {
                    if let Some(g) = self.game_view() {
                        editor.apply(&mut g.game);
                    }
                }
                (EditorOutcome::Done, EditorThen::Add(target)) => {
                    if let Some(g) = self.game_view() {
                        editor.apply(&mut g.game);
                        g.data_asked = true;
                    }
                    self.add_current_now(ctx, target);
                }
                (EditorOutcome::Cancel, EditorThen::Add(_)) => {
                    if let Some(intent) = self.own.then_close.take() {
                        self.confirm_close = Some(intent);
                    }
                }
                (EditorOutcome::Cancel, EditorThen::Done) => {}
            }
            return;
        }
        if self.own.chooser.is_some() {
            let rows = self.chooser_rows();
            if let Some(c) = self.own.chooser.as_mut() {
                c.refresh(rows);
            }
        }
        if let Some(chooser) = self.own.chooser.as_mut()
            && let Some(outcome) = chooser.show(ctx)
        {
            let chooser = self.own.chooser.take().expect("chooser");
            match outcome {
                ChooserOutcome::Pick(target) => match chooser.purpose {
                    ChooserFor::CurrentGame => self.add_current_to(ctx, target),
                    ChooserFor::Record { path, index } => {
                        self.append_record(ctx, path, index, target)
                    }
                },
                ChooserOutcome::NewBase => {
                    let after = match chooser.purpose {
                        ChooserFor::CurrentGame => own_db::AfterCreate::AddGame,
                        ChooserFor::Record { path, index } => {
                            own_db::AfterCreate::AddRecord { path, index }
                        }
                    };
                    self.new_base_dialog(after);
                }
                ChooserOutcome::SaveAsFile => {
                    self.close_after_save = self.own.then_close.take();
                    self.pick_save(ctx);
                }
                ChooserOutcome::Cancel => {
                    if let Some(intent) = self.own.then_close.take() {
                        self.confirm_close = Some(intent);
                    }
                }
            }
            return;
        }
        if let Some(dialog) = self.own.new_base.as_mut()
            && let Some(outcome) = dialog.show(ctx)
        {
            match outcome {
                NewBaseOutcome::PickFolder => {
                    let start = dialog.dir.clone();
                    self.pick_folder(ctx, start);
                }
                NewBaseOutcome::Cancel => {
                    self.own.new_base = None;
                    if let Some(intent) = self.own.then_close.take() {
                        self.confirm_close = Some(intent);
                    }
                }
                NewBaseOutcome::Create(_) | NewBaseOutcome::AddExisting(_) => {
                    let (create, path) = match outcome {
                        NewBaseOutcome::Create(p) => (true, p),
                        NewBaseOutcome::AddExisting(p) => (false, p),
                        _ => unreachable!(),
                    };
                    let made = if create {
                        self.create_base(ctx, path.clone())
                    } else {
                        self.settings.touch_database(&path);
                        self.settings_dirty = true;
                        self.library.select_db(&path, ctx);
                        Ok(())
                    };
                    match made {
                        // The dialog stays, with the reason, and the name typed.
                        Err(e) => {
                            if let Some(d) = self.own.new_base.as_mut() {
                                d.error = Some(e);
                            }
                        }
                        Ok(()) => {
                            let dialog = self.own.new_base.take().expect("dialog");
                            self.settings.new_database_dir = Some(dialog.dir.clone());
                            self.settings_dirty = true;
                            match dialog.after {
                                own_db::AfterCreate::Shelf => {}
                                own_db::AfterCreate::AddGame => {
                                    self.wait_for(ctx, &path, Pending::AddCurrent(path.clone()))
                                }
                                own_db::AfterCreate::AddRecord { path: from, index } => {
                                    let step = Pending::AppendRecord {
                                        from,
                                        index,
                                        target: path.clone(),
                                    };
                                    self.wait_for(ctx, &path, step)
                                }
                            }
                        }
                    }
                }
            }
        }
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
                Async::FolderPicked(Some(dir)) => {
                    self.settings.new_database_dir = Some(dir.clone());
                    self.settings_dirty = true;
                    if let Some(d) = self.own.new_base.as_mut() {
                        d.dir = dir;
                    }
                }
                Async::FolderPicked(None) => {}
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
        // The user's game file: synced to the disk before the rename, like a database.
        match chess_core::pgn::store::write_atomic(&path, pgn.as_bytes()) {
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
                        block: self.library.write_block(&path),
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
        // A comment being written goes into the game first. A game against Maia in
        // progress asks «The game is on» (its answer leaves, or not); before its first
        // moves it is just dropped.
        if let Some(g) = self.game.as_mut() {
            g.commit_editor();
            if g.playing_started() {
                g.ask_leave(intent);
                return;
            }
            g.resign_if_playing();
        }
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
            // A rematch from a position goes on over the same game.
            self.play_next_now(ctx);
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
        self.play_next_now(ctx);
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
        // Where the game can go (own-databases.md §3.4): back into its own PGN, or into a
        // database of the user's choice.
        let source = self.game.as_ref().and_then(|g| g.source.clone());
        let own = source
            .as_ref()
            .filter(|s| s.block.is_none())
            .map(|s| own_db::base_name(&s.path));
        // A new game meant for a database goes there.
        let target = self.current_target().map(|t| own_db::base_name(&t));
        egui::Modal::new(egui::Id::new("confirm_close")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr("Game modified"));
            if let Some(g) = &self.game {
                ui.label(RichText::new(g.title()).strong());
            }
            match (&own, &source) {
                (Some(name), _) => {
                    ui.label(trf("Save the changes to «{name}»?", &[("name", name)]));
                }
                (None, None) if let Some(name) = &target => {
                    ui.label(trf("Add the game to «{name}»?", &[("name", name)]));
                }
                (None, Some(s)) => {
                    if let Some(b) = &s.block {
                        ui.label(b.long());
                    }
                    ui.label(tr(
                        "Add the game with the changes to a database of your own?",
                    ));
                }
                (None, None) => {
                    ui.label(tr("The game is in no database yet. Add it to one?"));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let primary = match (&own, &target) {
                    (Some(name), _) => trf("Save to «{name}»", &[("name", name)]),
                    (None, Some(name)) => trf("Add to «{name}»", &[("name", name)]),
                    (None, None) => tr("Add to database…").to_owned(),
                };
                if crate::widgets::primary(ui, &primary).clicked() {
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
                if own.is_some() || target.is_some() {
                    self.save_current(ctx, Some(intent));
                } else {
                    self.own.then_close = Some(intent);
                    self.open_chooser(ctx, ChooserFor::CurrentGame);
                }
            }
            Some(1) => {
                self.confirm_close = None;
                self.finish_close(ctx, intent);
            }
            Some(2) => {
                self.confirm_close = None;
                self.find_after_close = None;
                self.play_next = None;
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
        // Quitting: a comment being written goes into the game first, so the question
        // about unsaved changes counts it.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_quit
            && let Some(g) = self.game.as_mut()
        {
            g.commit_editor();
            if g.playing_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                g.ask_leave(CloseIntent::QuitApp);
            } else {
                g.resign_if_playing();
            }
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
        // (A game against Maia in progress has asked «The game is on» instead.)
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_quit
            && self
                .game
                .as_ref()
                .is_some_and(|g| g.game.is_modified() && !g.playing_started())
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
        if ctx.input_mut(|i| {
            i.consume_shortcut(&KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::N,
            ))
        }) && self.game.is_none()
        {
            self.own.then_close = None;
            self.new_base_dialog(own_db::AfterCreate::Shelf);
        }
        // In the library a game from the start; on a game's screen the game plays on
        // from its position (the game view takes Ctrl+G there).
        if self.game.is_none() && ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::G))) {
            self.play_dialog = Some(crate::play::Dialog::new(&self.settings));
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::N))) && self.game.is_none() {
            self.new_game();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::Comma))) {
            self.settings_ui.open = !self.settings_ui.open;
        }

        // A new game's target base that left the shelf or stopped taking games is
        // dropped before the game is drawn (its header names the target).
        let _ = self.current_target();
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
            let review = self.settings.review.clone();
            let actions = game.show(
                ui,
                &mut self.settings,
                &mut self.analysis,
                &mut self.reference,
                &mut self.translation,
                input_enabled,
            );
            // The analysis options (its dialog) are kept.
            self.settings_dirty |= self.settings.review != review;
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
                    GameAction::Save => self.save_current(&ctx, None),
                    GameAction::Leave(intent) => self.request_close(&ctx, intent),
                    GameAction::SaveAndLeave(intent) => self.save_current(&ctx, Some(intent)),
                    GameAction::Rematch => {
                        let next = self
                            .game
                            .as_ref()
                            .and_then(|g| g.play_end.clone())
                            .map(|e| {
                                let mut s = e.start;
                                // From a position the colour is part of the task: the same.
                                s.user = match self.settings.play.color {
                                    _ if s.from.is_some() => s.user,
                                    crate::play::ColorChoice::Random => {
                                        if crate::play::random_unit() < 0.5 {
                                            chess_core::shakmaty::Color::White
                                        } else {
                                            chess_core::shakmaty::Color::Black
                                        }
                                    }
                                    _ => !s.user,
                                };
                                s
                            });
                        if let Some(s) = next {
                            // From a position: back over the game it came from.
                            let intent = if s.from.is_some() && !self.previous_games.is_empty() {
                                CloseIntent::ReturnToPrevious
                            } else {
                                CloseIntent::BackToLibrary
                            };
                            self.play_next = Some(PlayNext::Rematch(s));
                            self.request_close(&ctx, intent);
                        }
                    }
                    GameAction::PlayAgain => {
                        let from = self
                            .game
                            .as_ref()
                            .and_then(|g| g.play_end.as_ref())
                            .and_then(|e| e.start.from.clone());
                        let intent = if from.is_some() && !self.previous_games.is_empty() {
                            CloseIntent::ReturnToPrevious
                        } else {
                            CloseIntent::BackToLibrary
                        };
                        self.play_next = Some(PlayNext::Dialog(from));
                        self.request_close(&ctx, intent);
                    }
                    GameAction::Train(start) => self.open_training(*start),
                    GameAction::PlayFrom(from) => {
                        self.play_dialog =
                            Some(crate::play::Dialog::new_from(&self.settings, *from));
                    }
                    GameAction::AddToDatabase => {
                        if let Some(g) = self.game.as_mut() {
                            g.commit_editor();
                        }
                        self.own.then_close = None;
                        self.open_chooser(&ctx, ChooserFor::CurrentGame);
                    }
                    GameAction::EditGameData => self.edit_game_data(),
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
                    LibraryAction::NewGame => self.new_game(),
                    LibraryAction::Play => {
                        self.play_dialog = Some(crate::play::Dialog::new(&self.settings))
                    }
                    LibraryAction::NewGameIn(path) => {
                        self.new_game();
                        if let Some(g) = self.game.as_mut() {
                            g.target = Some(path);
                        }
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
                        // The file stays where it is; said so, with the place.
                        self.toast = Some(Toast {
                            text: trf(
                                "Removed from the shelf; the file stays: {path}",
                                &[("path", &p.display())],
                            ),
                            until: ctx.input(|i| i.time) + 6.0,
                            undo: false,
                        });
                    }
                    LibraryAction::OpenSettings => self.settings_ui.open = true,
                    LibraryAction::NewDatabase => {
                        self.own.then_close = None;
                        self.new_base_dialog(own_db::AfterCreate::Shelf);
                    }
                    LibraryAction::Utf8Copy(path) => self.utf8_copy(path),
                    LibraryAction::DeleteGame { path, index } => {
                        self.delete_game(&ctx, path, index)
                    }
                    LibraryAction::AddGameToDatabase { path, index } => {
                        self.own.then_close = None;
                        self.open_chooser(&ctx, ChooserFor::Record { path, index });
                    }
                    LibraryAction::EditGameData { path, index } => {
                        self.open_game(&ctx, path, index, None);
                        self.edit_game_data();
                    }
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
                SettingsAction::PickBaseFolder => {
                    let start = own_db::default_base_dir(self.settings.new_database_dir.as_deref());
                    self.pick_folder(&ctx, start);
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
        self.own_dialogs(&ctx);
        if let Some(d) = self.play_dialog.as_mut()
            && let Some(out) = d.show(&ctx, &mut self.settings)
        {
            self.play_dialog = None;
            if let crate::play::DialogOutcome::Play(start) = out {
                self.settings_dirty = true;
                self.start_play(&ctx, start);
            }
        }
        // Ctrl+Z on the library while the deleted game can still come back.
        if self.game.is_none()
            && self.own.undo.is_some()
            && self.toast.as_ref().is_some_and(|t| t.undo)
            && ctx.input_mut(|i| {
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z))
            })
        {
            self.undo_delete(&ctx);
        }

        let mut undo_clicked = false;
        if let Some(t) = &self.toast {
            let now = ctx.input(|i| i.time);
            if now > t.until {
                self.toast = None;
                self.own.undo = None;
            } else {
                let (msg, undo) = (t.text.clone(), t.undo && self.own.undo.is_some());
                let area = egui::Area::new(egui::Id::new(TOAST))
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            // Long messages wrap: at most 560 wide, inside a narrow window.
                            ui.set_max_width((ctx.content_rect().width() - 32.0).min(560.0));
                            ui.horizontal_wrapped(|ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(msg.as_str()).size(14.0)).wrap(),
                                );
                                if undo && ui.button(tr("Undo")).on_hover_text("Ctrl+Z").clicked() {
                                    undo_clicked = true;
                                }
                            });
                        });
                    });
                // Under the mouse the toast waits (the button is being reached for).
                if undo
                    && area.response.contains_pointer()
                    && let Some(t) = self.toast.as_mut()
                {
                    t.until = t.until.max(now + 1.5);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if undo_clicked {
            self.toast = None;
            self.undo_delete(&ctx);
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
