//! Library screen: shelf of databases (PGN, ChessBase, archives) + searchable,
//! sortable game table.

use crate::i18n::{tr, trf};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use chess_core::{GameEntry, ScanProgress};
use chess_import::{Database, Format, ImportError, Opened, Unsupported};
use egui::{Color32, RichText, Sense, Stroke, Ui, vec2};

use crate::board_search::{self, Background, BoardEvent, SearchBoard};
use crate::large_db::{self, LargeView, Pane};
use crate::settings::Settings;
use crate::viewed::{self, Viewed};
use chess_import::position::PositionOutcome;
use egui_extras::{Column, TableBuilder};
use shakmaty::uci::UciMove;

enum Slot {
    Loading {
        progress: Arc<Mutex<ScanProgress>>,
        cancel: Arc<AtomicBool>,
        rx: Receiver<Result<Opened, ImportError>>,
    },
    Ready(Arc<Database>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SortKey {
    Index,
    White,
    WhiteElo,
    Black,
    BlackElo,
    Result,
    Date,
    Event,
    Eco,
    Moves,
}

pub enum LibraryAction {
    /// Open a game; `at`: show the first position equal to this one.
    OpenGame {
        path: PathBuf,
        index: usize,
        at: Option<shakmaty::Chess>,
    },
    Sound(crate::assets::Sound),
    CopyText(String, &'static str),
    /// Open PGN text as a new game.
    OpenPgn(String),
    NewGame,
    PastePgn,
    AddDatabases,
    /// The window for downloading games from Lichess and Chess.com.
    OnlineGames,
    /// Fetch the new games of an online database.
    UpdateOnline(PathBuf),
    /// The window offering a newer version.
    OpenUpdate,
    RemoveDatabase(PathBuf),
    OpenSettings,
}

/// What a small database's position search answers: the search text and the position.
#[derive(Clone, PartialEq)]
struct SmallRequest {
    search: String,
    key: u64,
    line: Option<Vec<shakmaty::Move>>,
}

/// The search board of a small database (PGN, books): positions of all games,
/// variations included, are searched in memory.
struct SmallBoard {
    board: SearchBoard,
    open: bool,
    job: Option<(SmallRequest, shakmaty::Chess, Background<PositionOutcome>)>,
    /// The last result, the request and the position it answers.
    shown: Option<(SmallRequest, shakmaty::Chess, Arc<PositionOutcome>)>,
    table_hover: Option<UciMove>,
    /// Most games have no result: a book; continuations count chapters.
    books: bool,
    /// The position table cannot be built: memory it would need (bytes).
    blocked: Option<u64>,
}

impl SmallBoard {
    fn new(db: &chess_core::PgnDatabase) -> SmallBoard {
        let decided = db
            .entries()
            .iter()
            .filter(|e| matches!(e.result(), Some("1-0" | "0-1" | "1/2-1/2")))
            .count();
        let dev = SearchBoard::from_env();
        SmallBoard {
            open: dev.is_some(),
            board: dev.unwrap_or_else(SearchBoard::new),
            job: None,
            shown: None,
            table_hover: None,
            books: decided * 2 < db.len(),
            blocked: None,
        }
    }

    fn request(&self, search: &str) -> SmallRequest {
        let q = self.board.query();
        SmallRequest {
            search: search.trim().to_string(),
            key: self.board.key(),
            line: q.line,
        }
    }

    /// Collects a finished search and starts the one the board and search text ask for.
    fn update(&mut self, db: &Arc<Database>, search: &str, ctx: &egui::Context) {
        if let Some((req, pos, job)) = &self.job
            && let Some(result) = job.poll()
        {
            let (req, pos) = (req.clone(), pos.clone());
            self.job = None;
            if let Some(out) = result {
                self.shown = Some((req, pos, Arc::new(out)));
            }
        }
        if !self.open && !self.board.is_set() {
            return;
        }
        let wanted = self.request(search);
        let current = self
            .job
            .as_ref()
            .map(|(r, ..)| r)
            .or(self.shown.as_ref().map(|(r, ..)| r));
        if current != Some(&wanted) {
            // No memory for the table: say so instead of starting a search that
            // cannot answer.
            self.blocked = db.positions_blocked();
            if self.blocked.is_some() {
                return;
            }
            let (db, query, text) = (db.clone(), self.board.query(), wanted.search.clone());
            let pos = query.position.clone();
            let now = ctx.input(|i| i.time);
            self.job = Background::spawn(ctx, now, move |progress, cancel| {
                let pgn = db.as_pgn()?;
                let candidates: Option<Vec<u32>> = (!text.is_empty()).then(|| {
                    let mut v: Vec<u32> =
                        filtered_sorted(pgn.entries(), &text, (SortKey::Index, true), false)
                            .into_iter()
                            .map(|i| i as u32)
                            .collect();
                    v.sort_unstable();
                    v
                });
                db.search_position(candidates.as_deref(), &query, progress, cancel)
            })
            .map(|j| (wanted, pos, j));
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The outcome for the current board and search text, if ready.
    fn outcome(&self, search: &str) -> Option<&PositionOutcome> {
        let (req, _, out) = self.shown.as_ref()?;
        (*req == self.request(search)).then_some(out.as_ref())
    }
}

/// How a database was imported, and what is worth knowing about it.
struct ImportInfo {
    format: Format,
    notes: Vec<String>,
}

pub struct Library {
    slots: HashMap<PathBuf, Slot>,
    /// Games the user has opened (by content).
    viewed: Viewed,
    /// Filters and game list of each large ChessBase database (for the session).
    large: HashMap<PathBuf, LargeView>,
    /// Search board of each small database (for the session).
    small: HashMap<PathBuf, SmallBoard>,
    /// A position from a game to show once its database is open.
    pending_position: Option<(PathBuf, shakmaty::Chess, Vec<shakmaty::Move>, bool)>,
    info: HashMap<PathBuf, ImportInfo>,
    /// Database whose import notes are shown.
    notes_open: Option<PathBuf>,
    pub selected_db: Option<PathBuf>,
    search: String,
    selected_row: Option<usize>,
    sort: (SortKey, bool),
    /// Cached filtered+sorted entry indices for (db, search, sort).
    view: Vec<usize>,
    view_key: Option<(PathBuf, String, SortKey, bool)>,
    scroll_to_selected: bool,
    /// Lichess or Chess.com answered at start (the online games are offered).
    pub online: bool,
    /// Online databases being downloaded or updated.
    pub online_busy: Vec<PathBuf>,
    /// Games downloaded so far while downloads run (shown on the shelf card).
    pub online_running: Option<usize>,
    /// A newer version to offer (its number), next to Settings.
    pub update: Option<String>,
    /// File sizes of databases (`file_size`).
    sizes: HashMap<PathBuf, u64>,
}

impl Library {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            viewed: Viewed::load(),
            large: HashMap::new(),
            small: HashMap::new(),
            pending_position: None,
            info: HashMap::new(),
            notes_open: None,
            selected_db: None,
            search: String::new(),
            selected_row: None,
            sort: (SortKey::Index, true),
            view: Vec::new(),
            view_key: None,
            scroll_to_selected: false,
            online: false,
            online_busy: Vec::new(),
            online_running: None,
            sizes: HashMap::new(),
            update: None,
        }
    }

    /// Starts loading `path` in the background (no-op if already loaded/loading).
    pub fn load(&mut self, path: &Path, ctx: &egui::Context) {
        if matches!(
            self.slots.get(path),
            Some(Slot::Loading { .. } | Slot::Ready(_))
        ) {
            return;
        }
        let progress = Arc::new(Mutex::new(ScanProgress {
            bytes_done: 0,
            bytes_total: 0,
            games: 0,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (p, c, path_owned, ctx) = (
            progress.clone(),
            cancel.clone(),
            path.to_path_buf(),
            ctx.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("db-open".into())
            .spawn(move || {
                let res = chess_import::open(
                    &path_owned,
                    &mut |sp| {
                        if let Ok(mut g) = p.lock() {
                            *g = sp;
                        }
                        ctx.request_repaint();
                    },
                    &c,
                );
                let _ = tx.send(res);
                ctx.request_repaint();
            });
        let slot = match spawned {
            Ok(_) => Slot::Loading {
                progress,
                cancel,
                rx,
            },
            Err(e) => Slot::Failed(e.to_string()),
        };
        self.slots.insert(path.to_path_buf(), slot);
    }

    pub fn select_db(&mut self, path: &Path, ctx: &egui::Context) {
        self.load(path, ctx);
        if self.selected_db.as_deref() != Some(path) {
            self.selected_db = Some(path.to_path_buf());
            self.selected_row = None;
            self.search.clear();
            self.view_key = None;
        }
    }

    /// Selects (and scrolls to) a game row, e.g. when returning from a game.
    pub fn reveal(&mut self, path: &Path, index: usize) {
        if let Some(view) = self.large.get_mut(path) {
            view.reveal(index);
        }
        if self.selected_db.as_deref() == Some(path) {
            self.selected_row = Some(index);
            self.scroll_to_selected = true;
        }
    }

    /// Reads a database again (its file changed), keeping it selected if it was.
    pub fn reload(&mut self, path: &Path, ctx: &egui::Context) {
        let selected = self.selected_db.as_deref() == Some(path);
        self.forget(path);
        self.small.remove(path);
        if selected {
            self.select_db(path, ctx);
        } else {
            self.load(path, ctx);
        }
    }

    pub fn forget(&mut self, path: &Path) {
        self.info.remove(path);
        self.large.remove(path);
        if let Some(Slot::Loading { cancel, .. }) = self.slots.remove(path) {
            cancel.store(true, Ordering::Relaxed);
        }
        if self.selected_db.as_deref() == Some(path) {
            self.selected_db = None;
            self.view_key = None;
        }
    }

    /// Remembers that a game was opened (shown with a check mark in the table).
    pub fn mark_viewed(&mut self, path: &Path, index: usize) {
        if let Some(db) = self.database(path) {
            self.viewed.set(viewed::key(db.identity(index)), true);
        }
    }

    /// Size of a database's file (cached: asked every frame by the reference tab).
    pub fn file_size(&mut self, path: &Path) -> u64 {
        *self
            .sizes
            .entry(path.to_owned())
            .or_insert_with(|| std::fs::metadata(path).map_or(0, |m| m.len()))
    }

    /// Why the database could not be opened, if it could not.
    pub fn failure(&self, path: &Path) -> Option<&str> {
        match self.slots.get(path) {
            Some(Slot::Failed(e)) => Some(e.as_str()),
            _ => None,
        }
    }

    pub fn database(&self, path: &Path) -> Option<Arc<Database>> {
        match self.slots.get(path) {
            Some(Slot::Ready(db)) => Some(db.clone()),
            _ => None,
        }
    }

    /// Takes the databases that finished opening (also while a game is shown).
    pub fn poll(&mut self) {
        for (path, slot) in self.slots.iter_mut() {
            if let Slot::Loading { rx, .. } = slot
                && let Ok(res) = rx.try_recv()
            {
                *slot = match res {
                    Ok(o) => {
                        if matches!(o.db, Database::ChessBase(_)) {
                            self.large
                                .entry(path.clone())
                                .or_insert_with(LargeView::new);
                        }
                        self.info.insert(
                            path.clone(),
                            ImportInfo {
                                format: o.format,
                                notes: o.notes,
                            },
                        );
                        // Position searches without a usable `.cbb` get our own
                        // booster, kept in the cache directory.
                        if let Database::ChessBase(b) = &o.db
                            && let Some(dir) = Settings::cache_dir()
                        {
                            b.set_booster_dir(&dir.join("boosters"));
                        }
                        Slot::Ready(Arc::new(o.db))
                    }
                    Err(e) => Slot::Failed(import_error(&e)),
                };
                self.view_key = None;
            }
        }
        self.apply_pending_position();
    }

    /// Shows the board of the large database `path` with `line` played from `start`
    /// (a position taken from a game). False if the database is not a large one.
    pub fn show_position(
        &mut self,
        path: &Path,
        start: shakmaty::Chess,
        line: Vec<shakmaty::Move>,
        flipped: bool,
        ctx: &egui::Context,
    ) -> bool {
        self.select_db(path, ctx);
        self.pending_position = Some((path.to_path_buf(), start, line, flipped));
        self.apply_pending_position();
        true
    }

    /// Shows a requested position once its database is open.
    fn apply_pending_position(&mut self) {
        let Some((path, ..)) = &self.pending_position else {
            return;
        };
        let Some(Slot::Ready(db)) = self.slots.get(path) else {
            return;
        };
        let db = db.clone();
        let (path, start, line, flipped) = self.pending_position.take().expect("pending");
        match db.as_ref() {
            Database::ChessBase(_) => {
                let view = self.large.entry(path).or_insert_with(LargeView::new);
                view.show_position(start, line, flipped);
            }
            Database::Pgn(pgn) => {
                let small = self
                    .small
                    .entry(path)
                    .or_insert_with(|| SmallBoard::new(pgn));
                small.board.set(start, line, flipped);
                small.open = true;
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        databases: &[PathBuf],
        settings: &Settings,
    ) -> Vec<LibraryAction> {
        self.poll();
        let mut actions = Vec::new();
        // With the search board open in a low window the shelf folds into one line,
        // leaving the height to the board.
        let board_open = self.selected_db.as_ref().is_some_and(|p| {
            self.large.get(p).is_some_and(|v| v.pane() == Pane::Board)
                || self.small.get(p).is_some_and(|s| s.open)
        });
        let low = ui.ctx().content_rect().height() < 760.0;
        if board_open && low {
            egui::Panel::top("shelf_compact")
                .show(ui, |ui| self.compact_shelf(ui, databases, &mut actions));
        } else {
            egui::Panel::top("shelf").show(ui, |ui| self.shelf(ui, databases, &mut actions));
        }
        // A large database has its filter panel or its search board on the right.
        if let Some(path) = self.selected_db.clone()
            && let Some(Slot::Ready(db)) = self.slots.get(&path)
            && db.as_chessbase().is_some()
            && let Some(view) = self.large.get_mut(&path)
        {
            let db = db.clone();
            match view.pane() {
                Pane::Filters => {
                    egui::Panel::right("filters")
                        .resizable(true)
                        .default_size(280.0)
                        .size_range(260.0..=380.0)
                        .show(ui, |ui| view.panel(ui, &db));
                }
                Pane::Board => {
                    let width = ui.available_width();
                    egui::Panel::right("search_board")
                        .resizable(true)
                        .default_size((width * 0.5).max(340.0))
                        .size_range(340.0..=(width - 420.0).max(340.0))
                        .show(ui, |ui| view.board_panel(ui, &db, settings, &mut actions));
                }
                Pane::Hidden => {}
            }
        }
        // A small database has its search board on the right when it is open.
        if let Some(path) = self.selected_db.clone()
            && let Some(Slot::Ready(db)) = self.slots.get(&path)
            && db.as_pgn().is_some()
            && let Some(small) = self.small.get_mut(&path)
            && small.open
        {
            let db = db.clone();
            let search = self.search.clone();
            let width = ui.available_width();
            egui::Panel::right("search_board")
                .resizable(true)
                .default_size((width * 0.5).max(340.0))
                .size_range(340.0..=(width - 420.0).max(340.0))
                .show(ui, |ui| {
                    small.update(&db, &search, ui.ctx());
                    let outcome = small.outcome(&search).cloned();
                    let counting = small.job.as_ref().map(|_| 0.0);
                    let mut events = Vec::new();
                    small.board.keys(ui.ctx(), outcome.as_ref(), &mut events);
                    if let Some(need) = small.blocked {
                        let free = chess_import::memory::available().unwrap_or(0);
                        let missing = (need + (256 << 20)).saturating_sub(free) / 1_000_000;
                        ui.label(
                            egui::RichText::new(trf(
                                "Not enough memory for position search in this database: about {n} MB more is needed. Close other programs and try again.",
                                &[("n", &missing.max(1))],
                            ))
                            .color(ui.visuals().warn_fg_color),
                        );
                    }
                    events.extend(small.board.panel(
                        ui,
                        settings,
                        outcome.as_ref(),
                        counting,
                        small.table_hover,
                        small.books,
                    ));
                    for e in events {
                        match e {
                            BoardEvent::Changed => self.scroll_to_selected = false,
                            BoardEvent::Sound(s) => actions.push(LibraryAction::Sound(s)),
                            BoardEvent::CopyText(t, m) => {
                                actions.push(LibraryAction::CopyText(t, m))
                            }
                            BoardEvent::OpenAsGame(pgn) => {
                                actions.push(LibraryAction::OpenPgn(pgn))
                            }
                        }
                    }
                });
        }
        egui::CentralPanel::default().show(ui, |ui| self.table_area(ui, settings, &mut actions));
        self.notes_window(ui.ctx());
        actions
    }

    fn notes_window(&mut self, ctx: &egui::Context) {
        let Some(path) = self.notes_open.clone() else {
            return;
        };
        let Some(info) = self.info.get(&path) else {
            self.notes_open = None;
            return;
        };
        let mut open = true;
        egui::Window::new(tr("Import notes"))
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(tr(
                        "Everything else was read. Games listed here are shown as far as they could be decoded.",
                    ))
                    .weak(),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for n in &info.notes {
                        ui.label(format!("• {n}"));
                    }
                });
            });
        if !open {
            self.notes_open = None;
        }
    }

    /// The shelf as one line: a database picker, "+" and the usual buttons.
    fn compact_shelf(
        &mut self,
        ui: &mut Ui,
        databases: &[PathBuf],
        actions: &mut Vec<LibraryAction>,
    ) {
        let name = |p: &Path| {
            p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        ui.horizontal(|ui| {
            let current = self.selected_db.as_deref().map(name).unwrap_or_default();
            let mut pick = None;
            egui::ComboBox::from_id_salt("shelf_db")
                .selected_text(format!("🗄 {current}"))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for p in databases {
                        if ui
                            .selectable_label(self.selected_db.as_deref() == Some(p), name(p))
                            .clicked()
                        {
                            pick = Some(p.clone());
                        }
                    }
                });
            if let Some(p) = pick {
                self.select_db(&p, ui.ctx());
            }
            if ui
                .button("+")
                .on_hover_text(tr(
                    "Open PGN, ChessBase (.cbh, .cbv) or an archive (.zip, .7z) (Ctrl+O)",
                ))
                .clicked()
            {
                actions.push(LibraryAction::AddDatabases);
            }
            if self.online
                && ui
                    .button("🌐")
                    .on_hover_text(tr("Download any player's games by nickname into a database: yours, a pupil's, an opponent's"))
                    .clicked()
            {
                actions.push(LibraryAction::OnlineGames);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("⚙ Settings")).clicked() {
                    actions.push(LibraryAction::OpenSettings);
                }
                if let Some(v) = &self.update
                    && crate::widgets::update_button(ui, v).clicked()
                {
                    actions.push(LibraryAction::OpenUpdate);
                }
                if ui.button(tr("📋 Paste PGN")).clicked() {
                    actions.push(LibraryAction::PastePgn);
                }
                if ui.button(tr("♟ New game")).clicked() {
                    actions.push(LibraryAction::NewGame);
                }
            });
        });
    }

    fn shelf(&mut self, ui: &mut Ui, databases: &[PathBuf], actions: &mut Vec<LibraryAction>) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading(tr("Databases"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("⚙ Settings")).clicked() {
                    actions.push(LibraryAction::OpenSettings);
                }
                if let Some(v) = &self.update
                    && crate::widgets::update_button(ui, v).clicked()
                {
                    actions.push(LibraryAction::OpenUpdate);
                }
                if ui
                    .button(tr("📋 Paste PGN"))
                    .on_hover_text(tr("Open a game from the clipboard"))
                    .clicked()
                {
                    actions.push(LibraryAction::PastePgn);
                }
                if ui
                    .button(tr("♟ New game"))
                    .on_hover_text(tr("Empty board in the starting position (Ctrl+N)"))
                    .clicked()
                {
                    actions.push(LibraryAction::NewGame);
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for path in databases {
                self.db_card(ui, path, actions);
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(150.0, 64.0), Sense::click());
            let visuals = ui.style().interact(&resp);
            ui.painter()
                .rect_stroke(rect, 6.0, visuals.fg_stroke, egui::StrokeKind::Inside);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                tr("+ Add database"),
                egui::FontId::proportional(15.0),
                visuals.text_color(),
            );
            if resp
                .on_hover_text(tr(
                    "Open PGN, ChessBase (.cbh, .cbv) or an archive (.zip, .7z) (Ctrl+O)",
                ))
                .clicked()
            {
                actions.push(LibraryAction::AddDatabases);
            }
            if self.online {
                let (rect, resp) = ui.allocate_exact_size(vec2(150.0, 64.0), Sense::click());
                let visuals = ui.style().interact(&resp);
                ui.painter()
                    .rect_stroke(rect, 6.0, visuals.fg_stroke, egui::StrokeKind::Inside);
                let below = match self.online_running {
                    Some(n) => trf("downloading… {n}", &[("n", &crate::i18n::games(n))]),
                    None => "Lichess · Chess.com".to_owned(),
                };
                ui.painter().text(
                    rect.center() + vec2(0.0, -9.0),
                    egui::Align2::CENTER_CENTER,
                    format!("🌐 {}", tr("Online games")),
                    egui::FontId::proportional(15.0),
                    visuals.text_color(),
                );
                ui.painter().text(
                    rect.center() + vec2(0.0, 11.0),
                    egui::Align2::CENTER_CENTER,
                    below,
                    egui::FontId::proportional(13.0),
                    ui.visuals().weak_text_color(),
                );
                if resp
                    .on_hover_text(tr("Download any player's games by nickname into a database: yours, a pupil's, an opponent's"))
                    .clicked()
                {
                    actions.push(LibraryAction::OnlineGames);
                }
            }
        });
        ui.add_space(8.0);
    }

    fn db_card(&mut self, ui: &mut Ui, path: &Path, actions: &mut Vec<LibraryAction>) {
        let selected = self.selected_db.as_deref() == Some(path);
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let detail = match self.slots.get(path) {
            Some(Slot::Ready(db)) => match db.as_chessbase() {
                Some(b) => format!(
                    "{} · 🔒",
                    trf(
                        "{n} games",
                        &[("n", &large_db::short_count(b.games() + b.texts()))],
                    )
                ),
                None => trf("{n} games", &[("n", &db.len())]),
            },
            Some(Slot::Loading { progress, .. }) => {
                let p = progress.lock().map(|g| *g).unwrap_or(ScanProgress {
                    bytes_done: 0,
                    bytes_total: 0,
                    games: 0,
                });
                trf("loading… {n}", &[("n", &p.games)])
            }
            Some(Slot::Failed(_)) => tr("error").into(),
            None if !path.exists() => tr("file not found").into(),
            None => tr("not loaded").into(),
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(190.0, 64.0), Sense::click());
        let v = ui.visuals();
        let fill = if selected {
            v.selection.bg_fill
        } else if resp.hovered() {
            v.widgets.hovered.weak_bg_fill
        } else {
            v.faint_bg_color
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        let text_color = if selected {
            v.selection.stroke.color
        } else {
            v.text_color()
        };
        ui.painter().text(
            rect.left_top() + vec2(12.0, 10.0),
            egui::Align2::LEFT_TOP,
            if crate::online::is_online(path) {
                "🌐"
            } else {
                "🗄"
            },
            egui::FontId::proportional(22.0),
            text_color,
        );
        // Long names (book titles) are cut to two lines so the detail line stays readable.
        let mut job = egui::text::LayoutJob::simple(
            name,
            egui::FontId::proportional(14.0),
            text_color,
            rect.width() - 52.0,
        );
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = false;
        job.wrap.overflow_character = Some('…');
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter()
            .galley(rect.left_top() + vec2(44.0, 9.0), galley, text_color);
        ui.painter().text(
            rect.left_bottom() + vec2(44.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            detail,
            egui::FontId::proportional(12.0),
            v.weak_text_color(),
        );
        let resp = resp.on_hover_text(path.display().to_string());
        if resp.clicked() {
            self.select_db(path, ui.ctx());
        }
        resp.context_menu(|ui| {
            if ui.button(tr("Remove from list")).clicked() {
                actions.push(LibraryAction::RemoveDatabase(path.to_path_buf()));
                ui.close();
            }
            if ui.button(tr("Reload file")).clicked() {
                self.slots.remove(path);
                self.view_key = None;
                self.load(path, ui.ctx());
                ui.close();
            }
        });
    }

    fn table_area(&mut self, ui: &mut Ui, settings: &Settings, actions: &mut Vec<LibraryAction>) {
        let Some(path) = self.selected_db.clone() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(tr(
                        "Choose a database above, add a PGN file or start a new game",
                    ))
                    .weak()
                    .size(16.0),
                );
            });
            return;
        };
        let db = match self.slots.get(&path) {
            Some(Slot::Ready(db)) => db.clone(),
            Some(Slot::Loading {
                progress, cancel, ..
            }) => {
                let p = progress.lock().map(|g| *g).unwrap_or(ScanProgress {
                    bytes_done: 0,
                    bytes_total: 0,
                    games: 0,
                });
                let frac = if p.bytes_total > 0 {
                    p.bytes_done as f32 / p.bytes_total as f32
                } else {
                    0.0
                };
                let cancel = cancel.clone();
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(trf(
                        "Reading {path} — games found: {n}",
                        &[("path", &path.display()), ("n", &p.games)],
                    ));
                    ui.add(egui::ProgressBar::new(frac).desired_width(400.0));
                    if ui.button(tr("Cancel")).clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                });
                return;
            }
            Some(Slot::Failed(e)) => {
                let e = e.clone();
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(
                        RichText::new(trf(
                            "Could not open {path}: {e}",
                            &[("path", &path.display()), ("e", &e)],
                        ))
                        .color(ui.visuals().error_fg_color),
                    );
                    if ui.button(tr("Retry")).clicked() {
                        self.slots.remove(&path);
                        self.load(&path, ui.ctx());
                    }
                });
                return;
            }
            None => {
                self.load(&path, ui.ctx());
                return;
            }
        };

        if db.as_chessbase().is_some() {
            let view = self
                .large
                .entry(path.clone())
                .or_insert_with(LargeView::new);
            let notes = self.info.get(&path).map_or(0, |i| i.notes.len());
            if view.table(ui, &path, &db, &mut self.viewed, notes, settings, actions) {
                self.notes_open = Some(path.clone());
            }
            return;
        }
        let Some(pgn) = db.as_pgn() else {
            return;
        };

        let mut search_focused = false;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(
                    path.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
                .strong(),
            );
            match self.info.get(&path) {
                Some(info) if info.format != Format::Pgn => {
                    ui.label(
                        RichText::new(trf(
                            "· {n} games · {format}, read-only",
                            &[("n", &db.len()), ("format", &info.format.label())],
                        ))
                        .weak(),
                    )
                    .on_hover_text(tr(
                        "Converted to PGN when opened; the original files are not changed. Save a game to keep your edits.",
                    ));
                }
                _ => {
                    let enc = pgn.encoding();
                    ui.label(
                        RichText::new(trf(
                            "· {n} games · encoding {enc}",
                            &[("n", &db.len()), ("enc", &enc.label())],
                        ))
                        .weak(),
                    );
                }
            }
            let notes = self.info.get(&path).map_or(0, |i| i.notes.len());
            if notes > 0 && crate::widgets::notes_button(ui, notes).clicked() {
                self.notes_open = Some(path.clone());
            }
            if crate::online::is_online(&path) {
                if self.online_busy.contains(&path) {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(tr("updating…")).weak());
                } else {
                    let when = crate::online::State::load(&path)
                        .and_then(|s| s.updated)
                        .map(crate::online::ago);
                    let hint = if !self.online {
                        tr("No connection to Lichess and Chess.com").to_owned()
                    } else if let Some(w) = when {
                        trf(
                            "Fetch the games played since the last download ({when})",
                            &[("when", &w)],
                        )
                    } else {
                        tr("Fetch the games played since the last download").to_owned()
                    };
                    if ui
                        .add_enabled(self.online, egui::Button::new(tr("⟳ New games")).small())
                        .on_hover_text(&hint)
                        .on_disabled_hover_text(&hint)
                        .clicked()
                    {
                        actions.push(LibraryAction::UpdateOnline(path.clone()));
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let small = self
                    .small
                    .entry(path.clone())
                    .or_insert_with(|| SmallBoard::new(pgn));
                let label = if small.board.is_set() && !small.open {
                    format!("{} •", tr("Board"))
                } else {
                    tr("Board").to_string()
                };
                if ui
                    .selectable_label(small.open, label)
                    .on_hover_text(tr("Find games by position: play moves on the board (Ctrl+B)"))
                    .clicked()
                    || ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B))
                {
                    small.open = !small.open;
                }
                if !self.search.is_empty()
                    && crate::widgets::small_close_button(ui)
                        .on_hover_text(tr("Clear"))
                        .clicked()
                {
                    self.search.clear();
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(tr("🔍 Search: player, event, ECO, year"))
                        .desired_width(280.0),
                );
                if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                    resp.request_focus();
                }
                search_focused = resp.has_focus();
            });
        });
        ui.add_space(4.0);

        let key = (path.clone(), self.search.clone(), self.sort.0, self.sort.1);
        if self.view_key.as_ref() != Some(&key) {
            // Games from Lichess and Chess.com: by default the most recent first.
            let newest = crate::online::is_online(&path);
            self.view = filtered_sorted(pgn.entries(), &self.search, self.sort, newest);
            self.view_key = Some(key);
        }
        // With a position on the board: only the games that reach it.
        let small = self.small.get(&path);
        let position = small.filter(|s| s.board.is_set());
        // While a new search runs the last result stays (dimmed): its rows, arrows and
        // the position a game opens at all belong to the position it was found for.
        let last = position.and_then(|s| s.shown.as_ref());
        let outcome: Option<Arc<PositionOutcome>> = last.map(|(_, _, out)| out.clone());
        let current = position.is_some_and(|s| s.outcome(&self.search).is_some());
        let searching = position.is_some() && !current;
        let ordered = last.is_some_and(|(req, ..)| req.line.is_some());
        let at = last.map(|(_, pos, _)| pos.clone());
        let mut clear_position = false;
        if let Some(s) = position {
            ui.horizontal(|ui| {
                let text = trf("Position: {line}", &[("line", &s.board.chip_text())]);
                ui.label(RichText::new(text).size(13.0));
                clear_position = crate::widgets::small_close_button(ui)
                    .on_hover_text(tr("Remove the condition"))
                    .clicked();
                if searching {
                    ui.add(egui::Spinner::new().size(14.0));
                } else if let Some(out) = &outcome {
                    let n = large_db::grouped(out.matches.len());
                    ui.label(
                        RichText::new(trf(
                            "· found {n} of {total}",
                            &[("n", &n), ("total", &large_db::grouped(pgn.len()))],
                        ))
                        .weak(),
                    );
                }
            });
        }
        let filtered: Vec<usize>;
        let view: &Vec<usize> = match &outcome {
            Some(out) => {
                filtered = self
                    .view
                    .iter()
                    .copied()
                    .filter(|&i| {
                        out.matches
                            .binary_search_by_key(&(i as u32), |m| m.index)
                            .is_ok()
                    })
                    .collect();
                &filtered
            }
            None => &self.view,
        };
        let find = |index: usize| {
            let out = outcome.as_ref()?;
            let i = out
                .matches
                .binary_search_by_key(&(index as u32), |m| m.index)
                .ok()?;
            out.matches.get(i).copied()
        };

        // Keyboard: Up/Down move the selection, Enter opens.
        if !ui.ctx().egui_wants_keyboard_input() || search_focused {
            let pos = self
                .selected_row
                .and_then(|r| view.iter().position(|&i| i == r));
            let (up, down, enter) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::Enter),
                )
            });
            if down || up {
                let next = match (pos, down) {
                    (None, _) => 0,
                    (Some(p), true) => (p + 1).min(view.len().saturating_sub(1)),
                    (Some(p), false) => p.saturating_sub(1),
                };
                if let Some(&i) = view.get(next) {
                    self.selected_row = Some(i);
                    self.scroll_to_selected = true;
                }
            }
            if enter && let Some(index) = self.selected_row {
                actions.push(LibraryAction::OpenGame {
                    path: path.clone(),
                    index,
                    at: at.clone(),
                });
            }
        }

        let scroll_row = if std::mem::take(&mut self.scroll_to_selected) {
            self.selected_row
                .and_then(|r| view.iter().position(|&i| i == r))
        } else {
            None
        };
        let headers: [(&str, SortKey); 10] = [
            ("№", SortKey::Index),
            (tr("White"), SortKey::White),
            ("Elo", SortKey::WhiteElo),
            (tr("Black"), SortKey::Black),
            ("Elo", SortKey::BlackElo),
            (tr("Result"), SortKey::Result),
            (tr("Length"), SortKey::Moves),
            ("ECO", SortKey::Eco),
            (tr("Event"), SortKey::Event),
            (tr("Date"), SortKey::Date),
        ];
        if searching {
            ui.set_opacity(0.6);
        }
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if outcome.is_some() {
            table = table.column(Column::exact(52.0));
        }
        let mut table = table
            .column(Column::exact(52.0))
            .column(Column::initial(170.0).at_least(60.0).clip(true))
            .column(Column::exact(48.0))
            .column(Column::initial(170.0).at_least(60.0).clip(true))
            .column(Column::exact(48.0))
            .column(Column::exact(80.0))
            .column(Column::exact(52.0))
            .column(Column::exact(48.0))
            .column(Column::remainder().at_least(80.0).clip(true))
            .column(Column::exact(96.0));
        if let Some(r) = scroll_row {
            table = table.scroll_to_row(r, Some(egui::Align::Center));
        }
        let entries = pgn.entries();
        let mut hovered = None;
        let viewed = &self.viewed;
        let mut toggle_viewed = None;
        let mut clicked = None;
        let mut opened = None;
        let mut sort_click = None;
        table
            .header(26.0, |mut header| {
                if outcome.is_some() {
                    header.col(|ui| {
                        ui.label(RichText::new(tr("Move")).strong());
                    });
                }
                for (label, key) in headers {
                    header.col(|ui| {
                        let arrow = if self.sort.0 == key {
                            if self.sort.1 { " ⏶" } else { " ⏷" }
                        } else {
                            ""
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("{label}{arrow}")).strong(),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            sort_click = Some(key);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, view.len(), |mut row| {
                    let Some(e) = view.get(row.index()).and_then(|&i| entries.get(i)) else {
                        return;
                    };
                    row.set_selected(self.selected_row == Some(e.index));
                    let key = Some(viewed::key(pgn.raw_bytes(e.index).unwrap_or_default()));
                    let seen = key.is_some_and(|k| viewed.contains(k));
                    let cells: [String; 10] = [
                        (e.index + 1).to_string(),
                        e.white().unwrap_or("").to_string(),
                        e.white_elo().map(|x| x.to_string()).unwrap_or_default(),
                        e.black().unwrap_or("").to_string(),
                        e.black_elo().map(|x| x.to_string()).unwrap_or_default(),
                        e.result().unwrap_or("*").to_string(),
                        e.ply_count()
                            .map(|p| p.div_ceil(2).to_string())
                            .unwrap_or_default(),
                        e.eco().unwrap_or("").to_string(),
                        e.event().filter(|s| *s != "?").unwrap_or("").to_string(),
                        display_date(e.date().unwrap_or("")),
                    ];
                    if outcome.is_some() {
                        row.col(|ui| {
                            let Some(m) = find(e.index) else { return };
                            let mut text = board_search::ply_label(m.ply);
                            if ordered && !m.same_order {
                                text = format!("↪{text}");
                            }
                            if m.in_variation {
                                text.push_str(" 🔀");
                            }
                            ui.add(egui::Label::new(text).selectable(false))
                                .on_hover_text(tr("The move after which the position arose in the game. ↪ — reached by another move order, 🔀 — in a variation"));
                        });
                    }
                    for (i, text) in cells.into_iter().enumerate() {
                        row.col(|ui| {
                            if i == 0 && seen {
                                // A check mark before the number: the game was opened before.
                                let c = ui.max_rect().left_center() + vec2(7.0, 0.0);
                                let stroke = Stroke::new(2.0, Color32::from_rgb(0x62, 0x9c, 0x2e));
                                ui.painter().line_segment(
                                    [c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.5)],
                                    stroke,
                                );
                                ui.painter().line_segment(
                                    [c + vec2(-1.0, 3.5), c + vec2(5.0, -4.0)],
                                    stroke,
                                );
                            }
                            if i == 0 {
                                ui.add_space(14.0);
                            }
                            let mut t = RichText::new(text);
                            if i == 0 {
                                t = t.weak();
                            }
                            if i == 8 {
                                t = t.italics();
                            }
                            if i == 0 && (e.flags.has_structure_issues || e.flags.has_decode_errors)
                            {
                                t = t.color(Color32::from_rgb(0xe6, 0x8f, 0x00));
                            }
                            ui.add(egui::Label::new(t).selectable(false).truncate());
                        });
                    }
                    let resp = row.response();
                    if let Some(k) = key {
                        resp.context_menu(|ui| {
                            let label = if seen {
                                tr("Mark as not viewed")
                            } else {
                                tr("Mark as viewed")
                            };
                            if ui.button(label).clicked() {
                                toggle_viewed = Some((k, !seen));
                                ui.close();
                            }
                        });
                    }
                    if resp.hovered() {
                        hovered = Some(e.index);
                    }
                    if resp.clicked() {
                        clicked = Some(e.index);
                    }
                    if resp.double_clicked() {
                        opened = Some(e.index);
                    }
                });
            });
        let next = hovered
            .filter(|_| current)
            .zip(outcome.as_deref())
            .and_then(|(i, out)| board_search::next_move_of(out, i));
        if let Some(small) = self.small.get_mut(&path) {
            small.table_hover = next;
            if clear_position {
                small.board = SearchBoard::new();
            }
        }
        if let Some((k, v)) = toggle_viewed {
            self.viewed.set(k, v);
        }
        if let Some(k) = sort_click {
            self.sort = if self.sort.0 == k {
                (k, !self.sort.1)
            } else {
                (k, true)
            };
        }
        if let Some(i) = clicked {
            self.selected_row = Some(i);
        }
        if let Some(index) = opened {
            self.selected_row = Some(index);
            actions.push(LibraryAction::OpenGame { path, index, at });
        }
    }
}

/// "2010.??.??" → "2010", "1997.05.??" → "05.1997", full dates as "dd.mm.yyyy".
pub fn display_date(raw: &str) -> String {
    let parts: Vec<&str> = raw.split('.').collect();
    let known = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        [y, m, d] if known(y) && known(m) && known(d) => format!("{d}.{m}.{y}"),
        [y, m, _] if known(y) && known(m) => format!("{m}.{y}"),
        [y, ..] if known(y) => (*y).to_string(),
        _ => String::new(),
    }
}

/// The games matching `search`, in the order of the column `key`. In a downloaded
/// database (`newest`) the "№" order is the time the games were played, the most recent
/// first: the sites' games are appended to the file oldest first, one site after another.
fn filtered_sorted(
    entries: &[GameEntry],
    search: &str,
    (key, asc): (SortKey, bool),
    newest: bool,
) -> Vec<usize> {
    let needle = search.trim().to_lowercase();
    let mut v: Vec<usize> = entries
        .iter()
        .filter(|e| {
            needle.is_empty()
                || [
                    "White",
                    "Black",
                    "Event",
                    "Site",
                    "ECO",
                    "Date",
                    "Annotator",
                ]
                .iter()
                .any(|t| e.tag(t).is_some_and(|s| s.to_lowercase().contains(&needle)))
        })
        .map(|e| e.index)
        .collect();
    let text = |e: &GameEntry| -> String {
        match key {
            SortKey::White => e.white().unwrap_or("").to_lowercase(),
            SortKey::Black => e.black().unwrap_or("").to_lowercase(),
            SortKey::Result => e.result().unwrap_or("").to_string(),
            SortKey::Date => e.date().unwrap_or("").to_string(),
            SortKey::Event => e.event().unwrap_or("").to_lowercase(),
            SortKey::Eco => e.eco().unwrap_or("").to_string(),
            _ => String::new(),
        }
    };
    let num = |e: &GameEntry| -> i64 {
        match key {
            SortKey::Index => e.index as i64,
            SortKey::WhiteElo => e.white_elo().map(i64::from).unwrap_or(-1),
            SortKey::BlackElo => e.black_elo().map(i64::from).unwrap_or(-1),
            SortKey::Moves => e.ply_count().map(i64::from).unwrap_or(-1),
            _ => 0,
        }
    };
    if newest && key == SortKey::Index {
        let mut keyed: Vec<(String, usize)> =
            v.into_iter().map(|i| (entries[i].played_at(), i)).collect();
        keyed.sort_by(|x, y| if asc { y.cmp(x) } else { x.cmp(y) });
        return keyed.into_iter().map(|(_, i)| i).collect();
    }
    v.sort_by(|&a, &b| {
        let (ea, eb) = (&entries[a], &entries[b]);
        let ord = text(ea)
            .cmp(&text(eb))
            .then(num(ea).cmp(&num(eb)))
            .then(a.cmp(&b));
        if asc { ord } else { ord.reverse() }
    });
    v
}

/// User-facing text of an import failure.
fn import_error(e: &ImportError) -> String {
    match e {
        ImportError::Cancelled => tr("Loading cancelled").into(),
        ImportError::Empty => tr("No PGN or ChessBase database in the archive").into(),
        ImportError::Unsupported(Unsupported::Encrypted) => {
            tr("Password-protected archives (.cbz, encrypted zip/7z) are not supported").into()
        }
        ImportError::Unsupported(Unsupported::ChessBase2) => tr(
            "The ChessBase 2CBH format is not supported: export the database from ChessBase as CBH or PGN",
        )
        .into(),
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::pgn::scan_games;
    use std::sync::atomic::AtomicBool;

    fn entries(pgn: &str) -> Vec<GameEntry> {
        scan_games(
            pgn.as_bytes(),
            chess_core::TextEncoding::Utf8,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn downloaded_games_come_newest_first() {
        // As a download writes them: one site's games oldest first, then the other's.
        let e = entries(concat!(
            "[Site \"lichess\"][UTCDate \"2026.01.01\"][UTCTime \"10:00:00\"] 1. e4 *\n",
            "[Site \"lichess\"][UTCDate \"2026.03.01\"][UTCTime \"10:00:00\"] 1. d4 *\n",
            "[Site \"chess.com\"][UTCDate \"2026.02.01\"][UTCTime \"10:00:00\"] 1. c4 *\n",
        ));
        assert_eq!(filtered_sorted(&e, "", (SortKey::Index, true), true), vec![1, 2, 0]);
        assert_eq!(filtered_sorted(&e, "", (SortKey::Index, false), true), vec![0, 2, 1]);
        // Any other database keeps the order of its file.
        assert_eq!(filtered_sorted(&e, "", (SortKey::Index, true), false), vec![0, 1, 2]);
    }
}
