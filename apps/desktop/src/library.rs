//! Library screen: shelf of databases (PGN, ChessBase, archives) and, for the chosen
//! one, the game table with its filters and position search (`large_db.rs`).

use crate::i18n::{tr, trf};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use chess_core::ScanProgress;
use chess_import::{Database, Format, ImportError, Opened, Unsupported};
use egui::{RichText, Sense, Ui, vec2};

use crate::large_db::{self, Header, LargeView, OnlineButton, Pane};
use crate::settings::Settings;
use crate::viewed::{self, Viewed};

enum Slot {
    Loading {
        progress: Arc<Mutex<ScanProgress>>,
        cancel: Arc<AtomicBool>,
        rx: Receiver<Result<Opened, ImportError>>,
    },
    Ready(Arc<Database>),
    Failed(String),
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

/// How a database was imported, and what is worth knowing about it.
struct ImportInfo {
    format: Format,
    notes: Vec<String>,
}

pub struct Library {
    slots: HashMap<PathBuf, Slot>,
    /// Games the user has opened (by content).
    viewed: Viewed,
    /// Filters, quick search, position and game list of each database (for the session).
    large: HashMap<PathBuf, LargeView>,
    /// A position from a game to show once its database is open.
    pending_position: Option<(PathBuf, shakmaty::Chess, Vec<shakmaty::Move>, bool)>,
    info: HashMap<PathBuf, ImportInfo>,
    /// Database whose import notes are shown.
    notes_open: Option<PathBuf>,
    pub selected_db: Option<PathBuf>,
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
            pending_position: None,
            info: HashMap::new(),
            notes_open: None,
            selected_db: None,
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
                // The filters' index of a PGN is built here, not on the screen's thread
                // (a large download takes a second, §9.7).
                if let Ok(opened) = &res
                    && let Database::Pgn(pgn) = &opened.db
                {
                    pgn.catalog();
                }
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
        }
    }

    /// Selects (and scrolls to) a game row, e.g. when returning from a game.
    pub fn reveal(&mut self, path: &Path, index: usize) {
        if let Some(view) = self.large.get_mut(path) {
            view.reveal(index);
        }
    }

    /// Reads a database again (its file changed), keeping it selected if it was.
    pub fn reload(&mut self, path: &Path, ctx: &egui::Context) {
        let selected = self.selected_db.as_deref() == Some(path);
        // The conditions, the position, the sort and the selection stay (§9.7): the view
        // finds its list anew once the file is read.
        let view = self.large.remove(path);
        self.forget(path);
        if let Some(v) = view {
            self.large.insert(path.to_path_buf(), v);
        }
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
                        // Every database has the filters and the position search.
                        match self.large.get_mut(path) {
                            Some(view) => view.reread(&o.db),
                            None => {
                                let online = crate::online::is_online(path);
                                self.large
                                    .insert(path.clone(), LargeView::for_database(&o.db, online));
                            }
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
        let online = crate::online::is_online(&path);
        let view = self
            .large
            .entry(path)
            .or_insert_with(|| LargeView::for_database(&db, online));
        view.show_position(start, line, flipped);
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
        let board_open = self
            .selected_db
            .as_ref()
            .is_some_and(|p| self.large.get(p).is_some_and(|v| v.pane() == Pane::Board));
        let low = ui.ctx().content_rect().height() < 760.0;
        if board_open && low {
            egui::Panel::top("shelf_compact")
                .show(ui, |ui| self.compact_shelf(ui, databases, &mut actions));
        } else {
            egui::Panel::top("shelf").show(ui, |ui| self.shelf(ui, databases, &mut actions));
        }
        // A database has its filter panel or its search board on the right.
        if let Some(path) = self.selected_db.clone()
            && let Some(Slot::Ready(db)) = self.slots.get(&path)
            && let Some(view) = self.large.get_mut(&path)
        {
            let db = db.clone();
            match view.pane() {
                Pane::Filters => {
                    let r = egui::Panel::right("filters")
                        .resizable(true)
                        .default_size(280.0)
                        .size_range(260.0..=380.0)
                        .show(ui, |ui| view.panel(ui, &db));
                    crate::widgets::check_panel("filters", &r.response);
                }
                Pane::Board => {
                    let width = ui.available_width();
                    let r = egui::Panel::right("search_board")
                        .resizable(true)
                        .default_size((width * 0.5).max(340.0))
                        .size_range(340.0..=(width - 420.0).max(340.0))
                        .show(ui, |ui| view.board_panel(ui, &db, settings, &mut actions));
                    crate::widgets::check_panel("search_board", &r.response);
                }
                Pane::Hidden => {}
            }
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

        let online = crate::online::is_online(&path);
        let header = self.header(&path, &db, online);
        let view = self
            .large
            .entry(path.clone())
            .or_insert_with(|| LargeView::for_database(&db, online));
        let notes = self.info.get(&path).map_or(0, |i| i.notes.len());
        if view.table(
            ui,
            &path,
            &db,
            &header,
            &mut self.viewed,
            notes,
            settings,
            actions,
        ) {
            self.notes_open = Some(path.clone());
        }
    }

    /// What the header says about a database: its format, and for one downloaded from
    /// Lichess and Chess.com, its "⟳ New games".
    fn header(&self, path: &Path, db: &Database, online: bool) -> Header {
        let (format, format_hint) = match (db, self.info.get(path)) {
            (Database::ChessBase(_), _) => (
                tr("ChessBase, read-only").to_string(),
                tr("Read in place; the original files are not changed. Save a game to keep your edits.").to_string(),
            ),
            (Database::Pgn(_), Some(info)) if info.format != Format::Pgn => (
                trf("{format}, read-only", &[("format", &info.format.label())]),
                tr("Converted to PGN when opened; the original files are not changed. Save a game to keep your edits.").to_string(),
            ),
            (Database::Pgn(pgn), _) => (
                trf("PGN, encoding {enc}", &[("enc", &pgn.encoding().label())]),
                String::new(),
            ),
        };
        let online = online.then(|| {
            let when = crate::online::State::load(path)
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
            OnlineButton {
                busy: self.online_busy.contains(&path.to_path_buf()),
                enabled: self.online,
                hint,
            }
        });
        Header {
            format,
            format_hint,
            online,
        }
    }
}

/// "2010.??.??" → "2010", "1997.05.??" → "05.1997", full dates as "dd.mm.yyyy".
pub fn display_date(raw: &str) -> String {
    let parts: Vec<&str> = raw.split(['.', '-', '/']).collect();
    let known = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        [y, m, d] if known(y) && known(m) && known(d) => format!("{d}.{m}.{y}"),
        [y, m, _] if known(y) && known(m) => format!("{m}.{y}"),
        [y, ..] if known(y) => (*y).to_string(),
        _ => String::new(),
    }
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
