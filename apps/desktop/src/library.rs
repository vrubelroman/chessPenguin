//! Library screen: shelf of databases (PGN, ChessBase, archives) + searchable,
//! sortable game table.

use crate::i18n::{tr, trf};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use chess_core::{GameEntry, PgnDatabase, ScanProgress};
use chess_import::{Format, ImportError, Opened, Unsupported};
use egui::{Color32, RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};

enum Slot {
    Loading {
        progress: Arc<Mutex<ScanProgress>>,
        cancel: Arc<AtomicBool>,
        rx: Receiver<Result<Opened, ImportError>>,
    },
    Ready(Arc<PgnDatabase>),
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
    OpenGame { path: PathBuf, index: usize },
    NewGame,
    PastePgn,
    AddDatabases,
    RemoveDatabase(PathBuf),
    OpenSettings,
    Quit,
}

/// How a database was imported, and what is worth knowing about it.
struct ImportInfo {
    format: Format,
    notes: Vec<String>,
}

pub struct Library {
    slots: HashMap<PathBuf, Slot>,
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
}

impl Library {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            info: HashMap::new(),
            notes_open: None,
            selected_db: None,
            search: String::new(),
            selected_row: None,
            sort: (SortKey::Index, true),
            view: Vec::new(),
            view_key: None,
            scroll_to_selected: false,
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
        if self.selected_db.as_deref() == Some(path) {
            self.selected_row = Some(index);
            self.scroll_to_selected = true;
        }
    }

    pub fn forget(&mut self, path: &Path) {
        self.info.remove(path);
        if let Some(Slot::Loading { cancel, .. }) = self.slots.remove(path) {
            cancel.store(true, Ordering::Relaxed);
        }
        if self.selected_db.as_deref() == Some(path) {
            self.selected_db = None;
            self.view_key = None;
        }
    }

    pub fn database(&self, path: &Path) -> Option<Arc<PgnDatabase>> {
        match self.slots.get(path) {
            Some(Slot::Ready(db)) => Some(db.clone()),
            _ => None,
        }
    }

    fn poll(&mut self) {
        for (path, slot) in self.slots.iter_mut() {
            if let Slot::Loading { rx, .. } = slot
                && let Ok(res) = rx.try_recv()
            {
                *slot = match res {
                    Ok(o) => {
                        self.info.insert(
                            path.clone(),
                            ImportInfo {
                                format: o.format,
                                notes: o.notes,
                            },
                        );
                        Slot::Ready(Arc::new(o.db))
                    }
                    Err(e) => Slot::Failed(import_error(&e)),
                };
                self.view_key = None;
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, databases: &[PathBuf]) -> Vec<LibraryAction> {
        self.poll();
        let mut actions = Vec::new();
        egui::Panel::top("shelf").show(ui, |ui| self.shelf(ui, databases, &mut actions));
        egui::CentralPanel::default().show(ui, |ui| self.table_area(ui, &mut actions));
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
                        ui.label(RichText::new(format!("⚠ {n}")).color(ui.visuals().warn_fg_color));
                    }
                });
            });
        if !open {
            self.notes_open = None;
        }
    }

    fn shelf(&mut self, ui: &mut Ui, databases: &[PathBuf], actions: &mut Vec<LibraryAction>) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading(tr("Databases"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::widgets::close_button(ui).clicked() {
                    actions.push(LibraryAction::Quit);
                }
                if ui.button(tr("⚙ Settings")).clicked() {
                    actions.push(LibraryAction::OpenSettings);
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
            Some(Slot::Ready(db)) => trf("{n} games", &[("n", &db.len())]),
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
            "🗄",
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

    fn table_area(&mut self, ui: &mut Ui, actions: &mut Vec<LibraryAction>) {
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
                    let enc = db.encoding();
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
            if notes > 0
                && ui
                    .button(
                        RichText::new(format!("⚠ {notes}")).color(ui.visuals().warn_fg_color),
                    )
                    .on_hover_text(tr("Import notes"))
                    .clicked()
            {
                self.notes_open = Some(path.clone());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            self.view = filtered_sorted(db.entries(), &self.search, self.sort);
            self.view_key = Some(key);
        }

        // Keyboard: Up/Down move the selection, Enter opens.
        if !ui.ctx().egui_wants_keyboard_input() || search_focused {
            let pos = self
                .selected_row
                .and_then(|r| self.view.iter().position(|&i| i == r));
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
                    (Some(p), true) => (p + 1).min(self.view.len().saturating_sub(1)),
                    (Some(p), false) => p.saturating_sub(1),
                };
                if let Some(&i) = self.view.get(next) {
                    self.selected_row = Some(i);
                    self.scroll_to_selected = true;
                }
            }
            if enter && let Some(index) = self.selected_row {
                actions.push(LibraryAction::OpenGame {
                    path: path.clone(),
                    index,
                });
            }
        }

        let scroll_row = if std::mem::take(&mut self.scroll_to_selected) {
            self.selected_row
                .and_then(|r| self.view.iter().position(|&i| i == r))
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
            (tr("Moves"), SortKey::Moves),
            ("ECO", SortKey::Eco),
            (tr("Event"), SortKey::Event),
            (tr("Date"), SortKey::Date),
        ];
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
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
        let view = &self.view;
        let entries = db.entries();
        let mut clicked = None;
        let mut opened = None;
        let mut sort_click = None;
        table
            .header(26.0, |mut header| {
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
                    for (i, text) in cells.into_iter().enumerate() {
                        row.col(|ui| {
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
                    if resp.clicked() {
                        clicked = Some(e.index);
                    }
                    if resp.double_clicked() {
                        opened = Some(e.index);
                    }
                });
            });
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
            actions.push(LibraryAction::OpenGame { path, index });
        }
    }
}

/// "2010.??.??" → "2010", "1997.05.??" → "05.1997", full dates as "dd.mm.yyyy".
fn display_date(raw: &str) -> String {
    let parts: Vec<&str> = raw.split('.').collect();
    let known = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        [y, m, d] if known(y) && known(m) && known(d) => format!("{d}.{m}.{y}"),
        [y, m, _] if known(y) && known(m) => format!("{m}.{y}"),
        [y, ..] if known(y) => (*y).to_string(),
        _ => String::new(),
    }
}

fn filtered_sorted(entries: &[GameEntry], search: &str, (key, asc): (SortKey, bool)) -> Vec<usize> {
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
