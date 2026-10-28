//! Preparing against an opponent on the desktop (docs/design/opponent-prep.md): the
//! dialog that picks the database, the player's spellings, the colour and the book;
//! the report screen — the repertoire tree, the openings, the errors, the board of the
//! chosen position and the games through it — and the engine run over the opening
//! positions. The data are the `prep` crate's; the engine job is the same as the game
//! analysis's (`game_review::run`), one at a time for the whole program.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use chess_import::Database;
use chess_import::chessbase::base::{Side, fold};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use game_review::{JobEvent, JobFailure, JobSettings, Judgement};
use prep::{
    BookExit, BookOptions, Candidate, ErrorCounts, GameInfo, Options, Player,
    PositionCache, PrepReport, Tally,
};
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Move, Position, Square};

use crate::board::{Arrow, BoardState, BoardView};
use crate::board_search::{self, Background, PREVIEW_ARROW};
use crate::i18n::{tr, trf};
use crate::reference::RefDb;
use crate::review::{self, DialogAction};
use crate::settings::{REVIEW_QUALITY, Settings};

/// «Prepare against…» as the library asks it: the name typed or picked and the colour
/// of the filter (§2.1).
#[derive(Clone, Debug, Default)]
pub struct PrepEntry {
    pub query: String,
    /// The name id when it was picked from the list.
    pub exact: Option<u32>,
    pub side: Side,
}

/// The colour switch of the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    White,
    Black,
    Both,
}

impl ColorChoice {
    fn side(self) -> Side {
        match self {
            ColorChoice::White => Side::White,
            ColorChoice::Black => Side::Black,
            ColorChoice::Both => Side::Any,
        }
    }

    fn from_side(side: Side) -> Self {
        match side {
            Side::White => ColorChoice::White,
            Side::Black => ColorChoice::Black,
            Side::Any => ColorChoice::Both,
        }
    }

    /// The colours shown, White first.
    fn colours(self) -> Vec<bool> {
        match self {
            ColorChoice::White => vec![true],
            ColorChoice::Black => vec![false],
            ColorChoice::Both => vec![true, false],
        }
    }

    fn label(self) -> &'static str {
        match self {
            ColorChoice::White => tr("White"),
            ColorChoice::Black => tr("Black"),
            ColorChoice::Both => tr("Both"),
        }
    }
}

/// The book: a database of the shelf, none, or the report's own database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookChoice {
    None,
    Database(PathBuf),
    Same,
}

impl BookChoice {
    /// From the setting: an empty path is «none», the report's database «the same»,
    /// `None` the largest database on the shelf but the report's.
    fn from_setting(setting: Option<&Path>, own: &Path, dbs: &[RefDb]) -> BookChoice {
        match setting {
            Some(p) if p.as_os_str().is_empty() => BookChoice::None,
            Some(p) if p == own => BookChoice::Same,
            Some(p) if dbs.iter().any(|d| d.path == p) => BookChoice::Database(p.to_owned()),
            _ => dbs
                .iter()
                .filter(|d| d.path != own)
                .max_by_key(|d| d.bytes)
                .map_or(BookChoice::None, |d| BookChoice::Database(d.path.clone())),
        }
    }

    fn setting(&self, own: &Path) -> PathBuf {
        match self {
            BookChoice::None => PathBuf::new(),
            BookChoice::Database(p) => p.clone(),
            BookChoice::Same => own.to_owned(),
        }
    }

    fn label(&self, own_name: &str, dbs: &[RefDb]) -> String {
        match self {
            BookChoice::None => tr("None").to_owned(),
            BookChoice::Same => trf("This database ({name})", &[("name", &own_name)]),
            BookChoice::Database(p) => dbs
                .iter()
                .find(|d| d.path == *p)
                .map_or_else(|| p.display().to_string(), db_label),
        }
    }
}

/// "Mega Database 2026 · 11.7M games" for the pickers.
fn db_label(d: &RefDb) -> String {
    match &d.db {
        Some(db) => format!("{} · {}", d.name, crate::i18n::games_short(db.len())),
        None => format!("{} · {}", d.name, tr("not open")),
    }
}

/// The book picker: «None», the databases of the shelf but the report's own, «This
/// database». Returns the new choice.
fn book_picker(
    ui: &mut Ui,
    id: &str,
    current: &BookChoice,
    own: &Path,
    own_name: &str,
    dbs: &[RefDb],
    width: f32,
) -> Option<BookChoice> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.label(own_name, dbs))
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(*current == BookChoice::None, tr("None"))
                .clicked()
            {
                picked = Some(BookChoice::None);
            }
            for d in dbs.iter().filter(|d| d.path != own) {
                let choice = BookChoice::Database(d.path.clone());
                if ui.selectable_label(*current == choice, db_label(d)).clicked() {
                    picked = Some(choice);
                }
            }
            if ui
                .selectable_label(
                    *current == BookChoice::Same,
                    trf("This database ({name})", &[("name", &own_name)]),
                )
                .clicked()
            {
                picked = Some(BookChoice::Same);
            }
        })
        .response
        .on_hover_text(tr(
            "A book is a large database that shows up to which move his line is common and where he plays a rare move. A rare move is not a mistake.",
        ));
    picked.filter(|p| p != current)
}

// ----- the dialog ------------------------------------------------------------------

/// What «Open report» starts.
pub struct PrepStart {
    pub path: PathBuf,
    pub player: Player,
    pub side: ColorChoice,
    pub book: BookChoice,
}

pub enum DialogOutcome {
    Open(PrepStart),
    Cancel,
    /// A database of the shelf is needed but not read yet.
    LoadDb(PathBuf),
}

/// The words of a name for matching: folded, without full stops and commas.
fn name_words(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|w| w.trim_matches('.').to_owned())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Splits the names that match a query into those that match it word for word (an
/// initial like «Р.» matches the beginning of a word) and the similar ones, which
/// only start with its words («Вафин» ⊂ «Вафина»): most likely other people.
pub fn split_candidates(query: &str, list: Vec<Candidate>) -> (Vec<Candidate>, Vec<Candidate>) {
    let folded = fold(query);
    let wanted: Vec<(String, bool)> = folded
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(|w| (w.trim_matches('.').to_owned(), w.ends_with('.')))
        .filter(|(w, _)| !w.is_empty())
        .collect();
    let (mut exact, mut similar) = (Vec::new(), Vec::new());
    for c in list {
        let words = name_words(&c.name);
        let whole = wanted.iter().all(|(q, initial)| {
            words
                .iter()
                .any(|w| w == q || (*initial && w.starts_with(q.as_str())))
        });
        if whole {
            exact.push(c);
        } else {
            similar.push(c);
        }
    }
    (exact, similar)
}

/// Up to this many names are offered.
const CANDIDATES: usize = 40;

/// The name search in the background: the names and how many match in all.
type SearchJob = Background<(Vec<Candidate>, usize)>;
/// The count of the ticked names' games by colour (White, Black).
type CountJob = Background<(u32, u32)>;
/// The book step: every game's exit, how many left the book, the counts by position.
type BookJob = Background<(Vec<Option<BookExit>>, u32, HashMap<u64, u32>)>;

pub struct PrepDialog {
    pub path: PathBuf,
    /// The shelf (set by the application every frame).
    pub dbs: Vec<RefDb>,
    query: String,
    /// The query the lists answer.
    answered: Option<String>,
    search: Option<(String, SearchJob)>,
    exact: Vec<Candidate>,
    similar: Vec<Candidate>,
    total: usize,
    ticked: Vec<u32>,
    /// The name the dialog was opened with (ticked by itself when found).
    wanted: Option<u32>,
    side: ColorChoice,
    book: BookChoice,
    /// Games of the ticked names by colour, and the job counting them.
    counts: Option<(Vec<u32>, (u32, u32))>,
    count_job: Option<(Vec<u32>, CountJob)>,
    focus_query: bool,
    typed_at: f64,
    /// `CP_PREP_OPEN`: «Open report» pressed by itself once a name is ticked.
    auto_open: bool,
}

impl PrepDialog {
    pub fn new(path: PathBuf, entry: PrepEntry, settings: &Settings) -> Self {
        PrepDialog {
            book: BookChoice::from_setting(settings.prep.book.as_deref(), &path, &[]),
            path,
            dbs: Vec::new(),
            query: entry.query,
            answered: None,
            search: None,
            exact: Vec::new(),
            similar: Vec::new(),
            total: 0,
            ticked: Vec::new(),
            wanted: entry.exact,
            side: ColorChoice::from_side(entry.side),
            counts: None,
            count_job: None,
            focus_query: true,
            typed_at: f64::NEG_INFINITY,
            auto_open: std::env::var_os("CP_PREP_OPEN").is_some(),
        }
    }

    fn db(&self) -> Option<Arc<Database>> {
        self.dbs
            .iter()
            .find(|d| d.path == self.path)
            .and_then(|d| d.db.clone())
    }

    /// Starts the name search for the query, when the lists do not answer it yet.
    fn search(&mut self, ctx: &egui::Context, db: &Arc<Database>) {
        let query = self.query.trim().to_owned();
        if self.answered.as_deref() == Some(query.as_str())
            || self.search.as_ref().is_some_and(|(q, _)| *q == query)
        {
            return;
        }
        if query.is_empty() {
            self.answered = Some(query);
            self.search = None;
            self.exact.clear();
            self.similar.clear();
            self.total = 0;
            return;
        }
        let (db, q) = (db.clone(), query.clone());
        let now = ctx.input(|i| i.time);
        self.search = Background::spawn(ctx, now, move |_, _| Some(prep::candidates(&db, &q, CANDIDATES)))
            .map(|j| (query, j));
    }

    fn poll(&mut self, ctx: &egui::Context, db: &Arc<Database>) {
        if let Some((q, job)) = &self.search
            && let Some(r) = job.poll()
        {
            let q = q.clone();
            self.search = None;
            if let Some((list, total)) = r {
                let (exact, similar) = split_candidates(&q, list);
                self.exact = exact;
                self.similar = similar;
                self.total = total;
                self.answered = Some(q.clone());
                // Ticked by itself: the name the dialog was opened with, the name equal
                // to the query, else the only name that matches word for word.
                let folded = fold(&q);
                let auto = self
                    .wanted
                    .take()
                    .filter(|id| self.exact.iter().any(|c| c.id == *id))
                    .or_else(|| {
                        self.exact
                            .iter()
                            .find(|c| fold(&c.name) == folded)
                            .map(|c| c.id)
                    })
                    .or_else(|| (self.exact.len() == 1).then(|| self.exact[0].id));
                self.ticked = auto.into_iter().collect();
            }
        }
        // The games by colour of the ticked names.
        if let Some((ids, job)) = &self.count_job
            && let Some(r) = job.poll()
        {
            let ids = ids.clone();
            self.count_job = None;
            if let Some(c) = r {
                self.counts = Some((ids, c));
            }
        }
        let mut ids = self.ticked.clone();
        ids.sort_unstable();
        if !ids.is_empty()
            && self.counts.as_ref().is_none_or(|(c, _)| *c != ids)
            && self.count_job.as_ref().is_none_or(|(c, _)| *c != ids)
        {
            let (db, player) = (db.clone(), Player::new(ids.iter().copied()));
            let now = ctx.input(|i| i.time);
            self.count_job = Background::spawn(ctx, now, move |_, cancel| {
                let found = prep::games_of(&db, &player, Side::Any, cancel)?;
                let white = found.iter().filter(|(_, w)| *w).count() as u32;
                Some((white, found.len() as u32 - white))
            })
            .map(|j| (ids, j));
        }
    }

    /// Draws the dialog; what was chosen, if anything.
    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) -> Option<DialogOutcome> {
        let db = self.db();
        if let Some(db) = &db {
            self.search(ctx, db);
            self.poll(ctx, db);
        }
        let own_name = base_name(&self.path);
        if self.book == BookChoice::None && settings.prep.book.is_none() && !self.dbs.is_empty() {
            // The default book: the largest database on the shelf (once the shelf is known).
            self.book = BookChoice::from_setting(None, &self.path, &self.dbs);
        }
        let mut outcome = None;
        let ready = db.is_some() && !self.ticked.is_empty();
        let ticked_names: Vec<String> = self
            .exact
            .iter()
            .chain(self.similar.iter())
            .filter(|c| self.ticked.contains(&c.id))
            .map(|c| c.name.clone())
            .collect();
        let ticked_games: u32 = self
            .exact
            .iter()
            .chain(self.similar.iter())
            .filter(|c| self.ticked.contains(&c.id))
            .map(|c| c.games)
            .sum();
        let counts = self
            .counts
            .as_ref()
            .filter(|(ids, _)| {
                let mut t = self.ticked.clone();
                t.sort_unstable();
                *ids == t
            })
            .map(|(_, c)| *c);
        let modal = egui::Modal::new(egui::Id::new("prep_dialog")).show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.set_width((screen.width() - 40.0).min(520.0));
            ui.heading(tr("Preparing against an opponent"));
            ui.add_space(6.0);
            let label_w = 60.0;
            // The body scrolls in a low window; the buttons stay under it.
            egui::ScrollArea::vertical()
                .max_height((screen.height() - 140.0).max(120.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([label_w, 20.0], egui::Label::new(tr("Database")));
                        let width = ui.available_width() - 8.0;
                        let mut pick = None;
                        let current = self
                            .dbs
                            .iter()
                            .find(|d| d.path == self.path)
                            .map(db_label)
                            .unwrap_or_else(|| own_name.clone());
                        egui::ComboBox::from_id_salt("prep_dialog_db")
                            .selected_text(current)
                            .width(width)
                            .truncate()
                            .show_ui(ui, |ui| {
                                for d in &self.dbs {
                                    if ui.selectable_label(d.path == self.path, db_label(d)).clicked() {
                                        pick = Some(d.path.clone());
                                    }
                                }
                            });
                        if let Some(p) = pick.filter(|p| *p != self.path) {
                            self.path = p;
                            self.answered = None;
                            self.search = None;
                            self.ticked.clear();
                            self.counts = None;
                            self.count_job = None;
                            self.book = BookChoice::from_setting(
                                settings.prep.book.as_deref(),
                                &self.path,
                                &self.dbs,
                            );
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([label_w, 20.0], egui::Label::new(tr("Player")));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text(tr("Name or part of it"))
                                .desired_width(ui.available_width() - 34.0),
                        );
                        if std::mem::take(&mut self.focus_query) && self.query.is_empty() {
                            r.request_focus();
                        }
                        if r.changed() {
                            self.typed_at = ui.input(|i| i.time);
                            self.ticked.clear();
                        }
                        if !self.query.is_empty()
                            && crate::widgets::small_close_button(ui)
                                .on_hover_text(tr("Clear"))
                                .clicked()
                        {
                            self.query.clear();
                            self.ticked.clear();
                            self.typed_at = ui.input(|i| i.time);
                        }
                    });
                    ui.add_space(2.0);
                    match &db {
                        None if self.dbs.iter().any(|d| d.path == self.path && d.failed.is_some()) => {
                            let why = self
                                .dbs
                                .iter()
                                .find(|d| d.path == self.path)
                                .and_then(|d| d.failed.clone())
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(trf("This database cannot be opened: {e}", &[("e", &why)]))
                                    .color(ui.visuals().warn_fg_color),
                            );
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(RichText::new(tr("Opening the database…")).weak());
                            });
                            outcome = Some(DialogOutcome::LoadDb(self.path.clone()));
                        }
                        Some(_) => self.candidate_list(ui, label_w),
                    }
                    if !self.ticked.is_empty() {
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(trf(
                                "Selected: {names} · {n}",
                                &[
                                    ("names", &ticked_names.join(", ")),
                                    ("n", &crate::i18n::games(ticked_games as usize)),
                                ],
                            ))
                            .weak(),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.add_sized([label_w, 20.0], egui::Label::new(tr("Colour")));
                        ui.radio_value(&mut self.side, ColorChoice::Both, tr("Both"));
                        let (w, b) = match counts {
                            Some((w, b)) => (w.to_string(), b.to_string()),
                            None if self.ticked.is_empty() => ("—".to_owned(), "—".to_owned()),
                            None => ("…".to_owned(), "…".to_owned()),
                        };
                        ui.radio_value(
                            &mut self.side,
                            ColorChoice::White,
                            trf("White — {n}", &[("n", &w)]),
                        );
                        ui.radio_value(
                            &mut self.side,
                            ColorChoice::Black,
                            trf("Black — {n}", &[("n", &b)]),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([label_w, 20.0], egui::Label::new(tr("Book")));
                        let width = ui.available_width() - 8.0;
                        if let Some(b) = book_picker(ui, "prep_dialog_book", &self.book, &self.path, &own_name, &self.dbs, width) {
                            self.book = b;
                            settings.prep.book = Some(self.book.setting(&self.path));
                        }
                    });
                    ui.label(
                        RichText::new(tr(
                            "The book shows where he leaves the theory. «None» — without that column.",
                        ))
                        .weak()
                        .small(),
                    );
                });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let open = ui
                    .add_enabled(ready, egui::Button::new(RichText::new(tr("Open report")).strong()).min_size(vec2(0.0, 30.0)))
                    .on_disabled_hover_text(tr("Tick a player"));
                if open.clicked() {
                    outcome = Some(DialogOutcome::Open(self.start()));
                }
                if ui.button(tr("Cancel")).clicked() {
                    outcome = Some(DialogOutcome::Cancel);
                }
            });
        });
        if modal.should_close() {
            outcome = Some(DialogOutcome::Cancel);
        }
        if outcome.is_none() && ready && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            outcome = Some(DialogOutcome::Open(self.start()));
        }
        if outcome.is_none() && ready && self.auto_open && self.search.is_none() {
            self.auto_open = false;
            outcome = Some(DialogOutcome::Open(self.start()));
        }
        if self.search.is_some() || self.count_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        outcome
    }

    fn start(&self) -> PrepStart {
        PrepStart {
            path: self.path.clone(),
            player: Player::new(self.ticked.iter().copied()),
            side: self.side,
            book: self.book.clone(),
        }
    }

    /// The names that match: tick boxes, the similar names apart.
    fn candidate_list(&mut self, ui: &mut Ui, indent: f32) {
        if self.search.is_some() && self.answered.as_deref() != Some(self.query.trim()) {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new(tr("Searching…")).weak());
            });
            return;
        }
        if self.query.trim().is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.label(RichText::new(tr("Type a name: the players of the database are offered")).weak());
            });
            return;
        }
        if self.exact.is_empty() && self.similar.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.label(RichText::new(tr("No such player in this database")).weak());
            });
            return;
        }
        let rows = self.exact.len() + self.similar.len() + usize::from(!self.similar.is_empty());
        let height = (rows as f32 * 22.0).min(8.0 * 22.0) + 4.0;
        egui::ScrollArea::vertical()
            .id_salt("prep_candidates")
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let row = |ui: &mut Ui, c: &Candidate, ticked: &mut Vec<u32>| {
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        let mut on = ticked.contains(&c.id);
                        let width = ui.available_width() - 90.0;
                        ui.allocate_ui(vec2(width, 20.0), |ui| {
                            if ui.add(egui::Checkbox::new(&mut on, &c.name)).changed() {
                                if on {
                                    ticked.push(c.id);
                                } else {
                                    ticked.retain(|id| *id != c.id);
                                }
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(crate::i18n::games(c.games as usize)).weak());
                        });
                    });
                };
                for c in &self.exact {
                    row(ui, c, &mut self.ticked);
                }
                if !self.similar.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        ui.label(
                            RichText::new(tr("Similar names — most likely other people"))
                                .weak()
                                .small(),
                        );
                    });
                    for c in &self.similar {
                        row(ui, c, &mut self.ticked);
                    }
                }
                if self.total > self.exact.len() + self.similar.len() {
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        ui.label(
                            RichText::new(trf(
                                "{n} more matches…",
                                &[("n", &(self.total - self.exact.len() - self.similar.len()))],
                            ))
                            .weak(),
                        );
                    });
                }
            });
    }
}

/// The file's name without its extension.
fn base_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ----- the screen ------------------------------------------------------------------

/// What the screen asks of the application.
pub enum PrepAction {
    /// «⬅ Library»: the report stays in the stack.
    Back,
    OpenSettings,
    /// A game of the report in the game screen, at move `ply` (0: the start).
    OpenGame {
        path: PathBuf,
        index: usize,
        ply: usize,
    },
    /// A database of the shelf is needed (the book) but not read yet.
    LoadDb(PathBuf),
    /// The engine run started (the live engine pauses) / ended.
    EngineStarted,
    EngineEnded,
    NeedEngine,
    EngineSwitch,
    OpenEngines,
    Toast(String),
    CopyText(String, &'static str),
    /// The line as a new game (no database).
    OpenPgn(String),
    /// The position on the search board of this database.
    FindPosition(PathBuf, Chess, Vec<Move>, bool),
    PlayFrom(Box<crate::play::StartFrom>),
    SettingsChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Repertoire,
    Openings,
    Errors,
}

/// What is chosen: a node of the tree, an opening, an error — the board and the games
/// list follow it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    None,
    Node {
        white: bool,
        node: u32,
    },
    Opening {
        white: bool,
        family: usize,
        variation: Option<usize>,
    },
    Repeated {
        white: bool,
        index: usize,
    },
    Error {
        white: bool,
        index: usize,
    },
}

/// A row of the games list (the names and the event come from the database once).
#[derive(Clone)]
struct ListRow {
    white: String,
    black: String,
    white_elo: u16,
    black_elo: u16,
    result: &'static str,
    event: String,
}

enum JobMsg {
    Event(JobEvent),
    Done,
    Failed(String),
}

struct EngineJob {
    rx: Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    total: usize,
    done: usize,
    threads: u32,
    nodes: u64,
    /// Evaluations since the cache was saved.
    unsaved: usize,
}

impl Drop for EngineJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The position shown on the board: the line to it and the positions along it.
struct Shown {
    sans: Vec<String>,
    ucis: Vec<String>,
    positions: Vec<Chess>,
    /// The ply shown (`positions[ply]`); the last by default, a click on the line
    /// takes the board back along it.
    ply: usize,
    /// Arrows at the last ply (an error: the move played and the better one).
    arrows: Vec<Arrow>,
    /// «after 3.Bb5 · better 3.d4 · +0.3 → −1.5» under the line, for an error.
    note: Option<String>,
    opening: Option<usize>,
    /// The player's colour the position was chosen from (the board's orientation).
    white: bool,
}

/// An error at a node of the tree: by (the key before the move, the move).
#[derive(Clone)]
struct Mark {
    judgement: Judgement,
    best_san: String,
    before: game_review::Eval,
    after: game_review::Eval,
    games: u32,
}

/// A row of the repertoire tree as laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeRow {
    Node { white: bool, node: u32, depth: usize },
    /// «other moves (n)» under a node.
    Others { white: bool, parent: u32, depth: usize, count: usize },
}

/// The default expansion: nodes with at least this many games, this deep.
const EXPAND_GAMES: u32 = 5;
const EXPAND_PLIES: u8 = 6;
/// Fewer games: the percentages are shown weak.
const FEW_GAMES: u32 = 5;
const BOARD_MIN: f32 = 220.0;
const RIGHT_MIN: f32 = 560.0;
const ROW_H: f32 = 24.0;

/// The folder of the prep caches (the reports and the evaluations by position).
fn cache_dir() -> Option<PathBuf> {
    Settings::cache_dir().map(|d| d.join("prep"))
}

pub struct PrepScreen {
    pub path: PathBuf,
    db: Arc<Database>,
    db_name: String,
    pub player: Player,
    pub names: Vec<String>,
    /// On the screen (else kept in the stack under the library).
    pub shown: bool,
    /// The shelf (set by the application every frame; the book comes from it).
    pub dbs: Vec<RefDb>,
    color: ColorChoice,
    plies: usize,
    book: BookChoice,
    report: Option<PrepReport>,
    rows: Vec<ListRow>,
    /// The games read in the background (hundreds in a moment, a Mega filter in seconds).
    build: Option<Background<PrepReport>>,
    build_total: usize,
    book_job: Option<(u64, BookJob)>,
    /// The book that could not be searched (its fingerprint, why).
    book_failed: Option<(u64, String)>,
    book_asked: Option<PathBuf>,
    job: Option<EngineJob>,
    cache: PositionCache,
    nps_per_thread: Option<f64>,
    error: Option<String>,
    dialog_open: bool,
    /// The report changed since it was written to the cache.
    dirty: bool,
    /// Evaluations came in since the last re-judging (done at most once a second).
    pending_refresh: bool,
    last_refresh: f64,
    marks: [HashMap<(u64, String), Mark>; 2],
    tab: Tab,
    branches_from: u32,
    expanded: HashSet<(bool, u32)>,
    others_open: HashSet<(bool, u32)>,
    open_families: HashSet<(bool, usize)>,
    open_repeated: HashSet<(bool, usize)>,
    selection: Selection,
    shown_pos: Option<Shown>,
    /// The user turned the board (else it is the player's colour).
    flipped: Option<bool>,
    board_state: BoardState,
    /// In a low window the board is a panel over the lists, opened by a button.
    board_popup: bool,
    newest_first: bool,
    /// The keyboard is in the games list (else in the tab's table).
    focus_games: bool,
    games_cursor: Option<usize>,
    hover_arrow: Option<(Square, Square)>,
    scroll_to_selected: bool,
    /// The games list's height, once the user dragged the border (else 35 %).
    games_height: Option<f32>,
    /// `CP_PREP_ENGINE`: the engine run started by itself once the report is built.
    auto_engine: bool,
}

/// Plays `ucis` from the start; stops at a move that cannot be read.
fn replay(ucis: &[String]) -> Vec<Chess> {
    let mut pos = Chess::default();
    let mut out = vec![pos.clone()];
    for u in ucis {
        let Some(m) = u.parse::<UciMove>().ok().and_then(|u| u.to_move(&pos).ok()) else {
            break;
        };
        pos.play_unchecked(m);
        out.push(pos.clone());
    }
    out
}

/// "3.Bb5" / "2…Nc6" for move `ply` (0-based), in the chosen notation.
fn short_label(ply: usize, san: &str) -> String {
    let number = ply / 2 + 1;
    let label = if ply.is_multiple_of(2) {
        format!("{number}.{san}")
    } else {
        format!("{number}…{san}")
    };
    crate::san::show(&label).into_owned()
}

/// The line as text: "1.e4 c5 2.Nf3 Nc6".
fn line_text(sans: &[String]) -> String {
    let mut out = String::new();
    for (i, san) in sans.iter().enumerate() {
        if !out.is_empty() {
            out.push(' ');
        }
        if i.is_multiple_of(2) {
            out.push_str(&format!("{}.", i / 2 + 1));
        }
        out.push_str(san);
    }
    crate::san::show_line(&out)
}

/// "…" in the middle when the text is longer than `max` characters: the first and
/// the last moves of a line matter more than the middle.
fn shorten_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(n - keep).collect();
    format!("{head}…{tail}")
}

/// "01.2026 – 10.2026" from the games' dates (empty without dates).
fn period(games: &[GameInfo]) -> String {
    let dated: Vec<&GameInfo> = games.iter().filter(|g| g.date_number() > 0).collect();
    let first = dated.iter().min_by_key(|g| g.date_number());
    let last = dated.iter().max_by_key(|g| g.date_number());
    let month = |g: &GameInfo| -> String {
        let parts: Vec<&str> = g.date.split('.').collect();
        match parts.as_slice() {
            [y, m, ..] if m.chars().all(|c| c.is_ascii_digit()) && !m.is_empty() => format!("{m}.{y}"),
            [y, ..] => (*y).to_string(),
            _ => String::new(),
        }
    };
    match (first, last) {
        (Some(a), Some(b)) => {
            let (a, b) = (month(a), month(b));
            if a == b { a } else { format!("{a} – {b}") }
        }
        _ => String::new(),
    }
}

/// The player's tally as the result bar wants it (White's wins · draws · Black's wins).
fn bar_tally(t: &Tally, white: bool) -> chess_import::position::Tally {
    let (white_wins, black_wins) = if white { (t.wins, t.losses) } else { (t.losses, t.wins) };
    chess_import::position::Tally {
        games: t.games,
        white_wins,
        draws: t.draws,
        black_wins,
        ..Default::default()
    }
}

fn score_text(t: &Tally) -> String {
    t.score().map_or_else(|| "—".to_owned(), |s| format!("{:.0} %", 100.0 * s))
}

/// "+10 =1 −3".
fn tally_hint(t: &Tally) -> String {
    format!("+{} ={} −{}", t.wins, t.draws, t.losses)
}

fn judgement_weight(j: Judgement) -> u32 {
    match j {
        Judgement::Blunder => 3,
        Judgement::Mistake => 2,
        Judgement::Inaccuracy => 1,
        Judgement::Great | Judgement::Brilliant => 0,
    }
}

/// A move's mark as a pill: the symbol in white on the colour of the sign.
fn pill(ui: &mut Ui, j: Judgement) -> egui::Response {
    let text = review::judgement_symbol(j);
    let font = egui::FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let size = vec2(galley.size().x + 8.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 8.0, review::judgement_color(j));
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        Color32::WHITE,
    );
    resp
}

/// "3 ? · 1 ??" in the colours of the signs (nothing when there are none).
fn counts_text(ui: &mut Ui, c: &ErrorCounts) {
    let dark = ui.visuals().dark_mode;
    let mut first = true;
    for (n, j) in [
        (c.inaccuracies, Judgement::Inaccuracy),
        (c.mistakes, Judgement::Mistake),
        (c.blunders, Judgement::Blunder),
    ] {
        if n == 0 {
            continue;
        }
        if !first {
            ui.label(RichText::new("·").weak());
        }
        first = false;
        ui.label(
            RichText::new(format!("{n} {}", review::judgement_symbol(j)))
                .color(review::judgement_text_color(j, dark)),
        );
    }
}

impl PrepScreen {
    pub fn new(start: PrepStart, db: Arc<Database>, settings: &Settings, ctx: &egui::Context) -> Self {
        let dir = cache_dir();
        let cache = dir.as_deref().map(PositionCache::load).unwrap_or_default();
        let plies = settings.prep.plies.clamp(2, 60);
        let options = Options {
            plies,
            side: Side::Any,
        };
        let names = start.player.names(&db);
        let mut screen = PrepScreen {
            db_name: base_name(&start.path),
            path: start.path,
            db,
            player: start.player,
            names,
            shown: true,
            dbs: Vec::new(),
            color: start.side,
            plies,
            book: start.book,
            report: None,
            rows: Vec::new(),
            build: None,
            build_total: 0,
            book_job: None,
            book_failed: None,
            book_asked: None,
            job: None,
            cache,
            nps_per_thread: None,
            error: None,
            dialog_open: false,
            dirty: false,
            pending_refresh: false,
            last_refresh: 0.0,
            marks: [HashMap::new(), HashMap::new()],
            tab: match std::env::var("CP_PREP_TAB").as_deref() {
                Ok("openings") => Tab::Openings,
                Ok("errors") => Tab::Errors,
                _ => Tab::Repertoire,
            },
            branches_from: settings.prep.branches_from.clamp(1, 20),
            expanded: HashSet::new(),
            others_open: HashSet::new(),
            open_families: HashSet::new(),
            open_repeated: HashSet::new(),
            selection: Selection::None,
            shown_pos: None,
            flipped: None,
            board_state: BoardState::default(),
            board_popup: false,
            newest_first: true,
            focus_games: false,
            games_cursor: None,
            hover_arrow: None,
            scroll_to_selected: false,
            games_height: None,
            auto_engine: std::env::var_os("CP_PREP_ENGINE").is_some(),
        };
        let cached = dir
            .as_deref()
            .and_then(|d| PrepReport::load(d, &screen.db, &screen.player, &options));
        match cached {
            Some(r) => screen.take_report(r, settings),
            None => screen.rebuild(ctx),
        }
        screen
    }

    /// The same database and player: the report as it was.
    pub fn matches(&self, path: &Path, player: &Player) -> bool {
        self.path == path && self.player == *player
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// The background work goes on while the report is not on the screen (a game
    /// opened from it, the library): its results are taken in here.
    pub fn poll_quietly(&mut self, ctx: &egui::Context, settings: &Settings) {
        let mut actions = Vec::new();
        self.poll(ctx, settings, &mut actions);
    }

    /// The engine run after an engine was installed for it.
    pub fn start_after_engine(&mut self, settings: &Settings, ctx: &egui::Context) -> Vec<PrepAction> {
        let mut actions = Vec::new();
        self.start_engine(settings, ctx, &mut actions);
        actions
    }

    pub fn title(&self) -> String {
        let name = self.names.first().cloned().unwrap_or_default();
        trf("Preparing against {name}", &[("name", &name)])
    }

    /// Reads the games anew (a new depth; the database read again).
    fn rebuild(&mut self, ctx: &egui::Context) {
        self.job = None;
        self.book_job = None;
        let (db, player) = (self.db.clone(), self.player.clone());
        let options = Options {
            plies: self.plies,
            side: Side::Any,
        };
        self.build_total = 0;
        let now = ctx.input(|i| i.time);
        self.build = Background::spawn(ctx, now, move |progress, cancel| {
            prep::build(&db, &player, &options, progress, cancel)
        });
    }

    /// A report arrived (built or from the cache): the rows of the games list, the
    /// cached evaluations, the default expansion.
    fn take_report(&mut self, mut report: PrepReport, settings: &Settings) {
        let taken = report.take_cached(&self.cache, settings.prep.nodes());
        self.dirty |= taken > 0;
        self.rows = report
            .games
            .iter()
            .map(|g| {
                let r = self.db.row(g.index as usize);
                ListRow {
                    white: r.white,
                    black: r.black,
                    white_elo: r.white_elo,
                    black_elo: r.black_elo,
                    result: r.result,
                    event: r.event,
                }
            })
            .collect();
        self.expanded.clear();
        self.others_open.clear();
        for cr in [&report.white, &report.black] {
            for (i, n) in cr.tree.nodes.iter().enumerate() {
                if n.tally.games >= EXPAND_GAMES && n.ply <= EXPAND_PLIES {
                    self.expanded.insert((cr.white, i as u32));
                }
            }
        }
        self.report = Some(report);
        self.rebuild_marks();
        self.selection = Selection::None;
        self.shown_pos = None;
        self.games_cursor = None;
    }

    /// The errors by node, for the tree and the hover texts.
    fn rebuild_marks(&mut self) {
        let Some(report) = &self.report else { return };
        for (i, cr) in [&report.white, &report.black].into_iter().enumerate() {
            let mut map: HashMap<(u64, String), Mark> = HashMap::new();
            for e in &cr.errors {
                let m = map.entry((e.key, e.uci.clone())).or_insert_with(|| Mark {
                    judgement: e.judgement,
                    best_san: e.best_san.clone(),
                    before: e.before,
                    after: e.after,
                    games: 0,
                });
                m.games += 1;
            }
            self.marks[i] = map;
        }
    }

    fn mark_of(&self, white: bool, key: u64, uci: &str) -> Option<&Mark> {
        self.marks[usize::from(!white)].get(&(key, uci.to_owned()))
    }

    // ----- the background work -----------------------------------------------------

    fn poll(&mut self, ctx: &egui::Context, settings: &Settings, actions: &mut Vec<PrepAction>) {
        if let Some(job) = &self.build
            && let Some(r) = job.poll()
        {
            self.build = None;
            match r {
                Some(report) => self.take_report(report, settings),
                None => self.error = Some(tr("The games could not be read").to_owned()),
            }
        }
        if let Some((fp, job)) = &self.book_job
            && let Some(r) = job.poll()
        {
            let fp = *fp;
            self.book_job = None;
            match r {
                Some((exits, left, counts)) => {
                    if let Some(report) = self.report.as_mut() {
                        for (g, b) in report.games.iter_mut().zip(exits) {
                            g.book = b;
                        }
                        report.book = Some(prep::BookInfo {
                            fingerprint: fp,
                            threshold: prep::DEFAULT_BOOK_THRESHOLD,
                            left,
                        });
                        report.book_counts = counts;
                        self.dirty = true;
                    }
                }
                None => {
                    self.book_failed = Some((fp, tr("the position search failed (not enough memory?)").to_owned()))
                }
            }
        }
        self.poll_engine(ctx, actions);
        self.start_book(ctx, actions);
        if self.auto_engine && self.report.is_some() && self.build.is_none() {
            self.auto_engine = false;
            self.start_engine(settings, ctx, actions);
        }
        if self.build.is_some() || self.book_job.is_some() || self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The book step for the chosen book, once the report and the book are there.
    fn start_book(&mut self, ctx: &egui::Context, actions: &mut Vec<PrepAction>) {
        let Some(report) = &self.report else { return };
        if self.book_job.is_some() || self.build.is_some() {
            return;
        }
        let (book_db, same) = match &self.book {
            BookChoice::None => return,
            BookChoice::Same => (self.db.clone(), true),
            BookChoice::Database(p) => {
                let Some(d) = self.dbs.iter().find(|d| d.path == *p) else {
                    return;
                };
                match &d.db {
                    Some(db) => (db.clone(), false),
                    None => {
                        if d.failed.is_none() && self.book_asked.as_ref() != Some(p) {
                            self.book_asked = Some(p.clone());
                            actions.push(PrepAction::LoadDb(p.clone()));
                        }
                        return;
                    }
                }
            }
        };
        let fp = prep::fingerprint(&book_db);
        if report.book.as_ref().is_some_and(|b| b.fingerprint == fp)
            || self.book_failed.as_ref().is_some_and(|(f, _)| *f == fp)
        {
            return;
        }
        let mut clone = report.clone();
        let options = BookOptions {
            threshold: prep::DEFAULT_BOOK_THRESHOLD,
            same_database: same,
        };
        let now = ctx.input(|i| i.time);
        self.book_job = Background::spawn(ctx, now, move |progress, cancel| {
            clone.find_book_exits(&book_db, &options, progress, cancel)?;
            let exits = clone.games.iter().map(|g| g.book).collect();
            let left = clone.book.as_ref().map_or(0, |b| b.left);
            Some((exits, left, std::mem::take(&mut clone.book_counts)))
        })
        .map(|j| (fp, j));
    }

    /// The book's count in a position, when the book step looked at it.
    fn book_count(&self, key: u64) -> Option<u32> {
        let report = self.report.as_ref()?;
        report.book.as_ref()?;
        report.book_counts.get(&key).copied()
    }

    // ----- the engine ----------------------------------------------------------------

    /// Positions the next run has to evaluate, and how long it takes.
    fn estimate(&self, settings: &Settings) -> (usize, f64) {
        let Some(report) = &self.report else {
            return (0, 0.0);
        };
        let nodes = settings.prep.nodes();
        let missing = report.missing_for(nodes, self.color.side());
        let per_thread = self.nps_per_thread.unwrap_or(review::DEFAULT_NPS);
        (missing, review::estimate_secs(missing, nodes, per_thread, settings.review.threads))
    }

    /// The button: the dialog the first time, at once with the options chosen before.
    fn engine_asked(&mut self, settings: &Settings, ctx: &egui::Context, actions: &mut Vec<PrepAction>) {
        if !crate::engine_offer::EngineOffer::engine_ready(settings) {
            actions.push(PrepAction::NeedEngine);
            return;
        }
        if settings.prep.chosen {
            self.start_engine(settings, ctx, actions);
        } else {
            self.dialog_open = true;
        }
    }

    fn start_engine(&mut self, settings: &Settings, ctx: &egui::Context, actions: &mut Vec<PrepAction>) {
        self.error = None;
        let Some(engine) = settings.active_engine().cloned() else {
            self.dialog_open = true;
            return;
        };
        let Some(path) = engine.executable() else {
            actions.push(PrepAction::NeedEngine);
            return;
        };
        let Some(report) = self.report.as_mut() else { return };
        let nodes = settings.prep.nodes();
        if report.take_cached(&self.cache, nodes) > 0 {
            self.dirty = true;
        }
        let targets = report.targets_for(nodes, self.color.side());
        if targets.is_empty() {
            actions.push(PrepAction::Toast(tr("Every position is already evaluated").to_owned()));
            self.tab = Tab::Errors;
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = targets.len();
        let job_settings = JobSettings {
            threads: settings.review.threads.max(1),
            hash_mb: settings.review.hash_mb,
            nodes,
            eval_file: None,
        };
        let threads = job_settings.threads;
        let (cancel_w, ctx_w) = (cancel.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("prep-engine".into())
            .spawn(move || {
                let result = game_review::run(
                    uci::EngineConfig::new(&path),
                    &job_settings,
                    targets,
                    &cancel_w,
                    &mut |e| {
                        let _ = tx.send(JobMsg::Event(e));
                        ctx_w.request_repaint();
                    },
                );
                let msg = match result {
                    Ok(()) => JobMsg::Done,
                    Err(JobFailure::Start(e) | JobFailure::Engine(e)) => JobMsg::Failed(e),
                    Err(JobFailure::Terminated(reason)) => {
                        JobMsg::Failed(crate::analysis::termination_message(&reason))
                    }
                };
                let _ = tx.send(msg);
                ctx_w.request_repaint();
            });
        if let Err(e) = spawned {
            self.error = Some(e.to_string());
            return;
        }
        self.job = Some(EngineJob {
            rx,
            cancel,
            total,
            done: 0,
            threads,
            nodes,
            unsaved: 0,
        });
        self.tab = Tab::Errors;
        actions.push(PrepAction::EngineStarted);
    }

    fn poll_engine(&mut self, ctx: &egui::Context, actions: &mut Vec<PrepAction>) {
        let Some(job) = self.job.as_mut() else { return };
        let Some(report) = self.report.as_mut() else { return };
        let mut ended = false;
        let mut got = false;
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                JobMsg::Event(e) => {
                    if let JobEvent::Eval { nps, .. } = &e {
                        job.done += 1;
                        if *nps > 0 {
                            self.nps_per_thread = Some(*nps as f64 / f64::from(job.threads));
                        }
                    }
                    if let Some((k, eval)) = report.apply(&e, job.nodes) {
                        self.cache.insert(k, eval);
                        job.unsaved += 1;
                        got = true;
                    }
                }
                JobMsg::Done => ended = true,
                JobMsg::Failed(e) => {
                    self.error = Some(e);
                    ended = true;
                }
            }
        }
        if got {
            self.pending_refresh = true;
            self.dirty = true;
        }
        if job.unsaved >= 200
            && let Some(dir) = cache_dir()
        {
            let _ = self.cache.save(&dir);
            job.unsaved = 0;
        }
        let now = ctx.input(|i| i.time);
        if self.pending_refresh && (ended || now - self.last_refresh > 1.0) {
            report.refresh();
            self.pending_refresh = false;
            self.last_refresh = now;
            self.rebuild_marks();
        }
        if ended {
            self.job = None;
            self.save();
            actions.push(PrepAction::EngineEnded);
        }
    }

    /// «Stop»: what was evaluated stays.
    fn stop_engine(&mut self, actions: &mut Vec<PrepAction>) {
        if self.job.take().is_some() {
            if let Some(r) = self.report.as_mut() {
                r.refresh();
            }
            self.rebuild_marks();
            self.pending_refresh = false;
            self.save();
            actions.push(PrepAction::EngineEnded);
        }
    }

    /// Writes the report and the evaluations to the cache.
    pub fn save(&mut self) {
        let Some(dir) = cache_dir() else { return };
        if let Err(e) = self.cache.save(&dir) {
            eprintln!("prep cache: {}: {e}", dir.display());
        }
        if self.dirty
            && let Some(r) = &self.report
        {
            match r.save(&dir) {
                Ok(()) => self.dirty = false,
                Err(e) => eprintln!("prep report: {}: {e}", dir.display()),
            }
        }
    }

    /// The launch dialog (§4.2).
    fn engine_dialog(&mut self, ctx: &egui::Context, settings: &mut Settings, actions: &mut Vec<PrepAction>) {
        if !self.dialog_open {
            return;
        }
        let mut action = None;
        let moves = self.plies.div_ceil(2);
        let (white_n, black_n) = self
            .report
            .as_ref()
            .map_or((0, 0), |r| (r.white.games.len(), r.black.games.len()));
        let modal = egui::Modal::new(egui::Id::new("prep_engine_dialog")).show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.set_width((screen.width() - 40.0).min(460.0));
            ui.heading(tr("Opening errors"));
            ui.label(
                RichText::new(trf(
                    "The engine evaluates the first {n} moves of every game of the opponent. The games are not changed.",
                    &[("n", &moves)],
                ))
                .weak(),
            );
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height((screen.height() - 160.0).max(100.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if let Some(a) = review::engine_picker(ui, settings, "prep_engine") {
                        action = Some(a);
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Colour"));
                        for (choice, n) in [
                            (ColorChoice::White, white_n),
                            (ColorChoice::Black, black_n),
                            (ColorChoice::Both, white_n + black_n),
                        ] {
                            let label = format!("{} — {}", choice.label(), crate::i18n::games(n));
                            ui.radio_value(&mut self.color, choice, label);
                        }
                    });
                    ui.add_space(4.0);
                    let mut quality = settings.prep.quality;
                    let (mut threads, mut hash) = (settings.review.threads, settings.review.hash_mb);
                    review::options_grid(
                        ui,
                        "prep_grid",
                        &mut quality,
                        &mut threads,
                        &mut hash,
                        Some(tr(
                            "For opening errors “Fast” is usually enough. “Normal” is for adult databases and takes 4 times longer.",
                        )),
                    );
                    settings.prep.quality = quality;
                    settings.review.threads = threads;
                    settings.review.hash_mb = hash;
                    ui.add_space(6.0);
                    let (missing, secs) = self.estimate(settings);
                    let all = self
                        .report
                        .as_ref()
                        .map_or(0, |r| r.distinct_positions_for(self.color.side()));
                    ui.label(trf(
                        "{n} positions, about {time}",
                        &[("n", &missing), ("time", &review::format_duration(secs))],
                    ));
                    let cached = all.saturating_sub(missing);
                    let note = if cached > 0 {
                        trf(
                            "identical positions are counted once; {n} already evaluated earlier",
                            &[("n", &cached)],
                        )
                    } else {
                        tr("identical positions are counted once").to_owned()
                    };
                    ui.label(RichText::new(note).weak().small());
                });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = settings.active_engine().is_some();
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(RichText::new(tr("Find errors")).strong().size(16.0)),
                    )
                    .clicked()
                {
                    action = Some(DialogAction::Start);
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.dialog_open = false;
                }
                if !ready {
                    ui.label(RichText::new(tr("Add an engine first")).color(ui.visuals().warn_fg_color));
                }
            });
        });
        if modal.should_close() {
            self.dialog_open = false;
        }
        if action.is_none()
            && settings.active_engine().is_some()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
        {
            action = Some(DialogAction::Start);
        }
        match action {
            Some(DialogAction::Start) => {
                self.dialog_open = false;
                settings.prep.chosen = true;
                actions.push(PrepAction::SettingsChanged);
                self.start_engine(settings, ctx, actions);
            }
            Some(DialogAction::EngineSwitch) => actions.push(PrepAction::EngineSwitch),
            Some(DialogAction::OpenEngines) => {
                self.dialog_open = false;
                actions.push(PrepAction::OpenEngines);
            }
            None => {}
        }
    }

    // ----- the selection and the board ---------------------------------------------

    fn select(&mut self, sel: Selection) {
        if self.selection == sel {
            return;
        }
        self.selection = sel;
        self.games_cursor = None;
        self.shown_pos = self.shown_for(&self.selection);
        self.board_state.clear();
    }

    /// The position of a selection and the way to it.
    fn shown_for(&self, sel: &Selection) -> Option<Shown> {
        let report = self.report.as_ref()?;
        let from_game = |g: &GameInfo, ply: usize, white: bool| -> Shown {
            let ucis: Vec<String> = g.ucis[..ply.min(g.ucis.len())].to_vec();
            let sans: Vec<String> = g.sans[..ucis.len()].to_vec();
            let positions = replay(&ucis);
            Shown {
                ply: positions.len() - 1,
                sans,
                ucis,
                positions,
                arrows: Vec::new(),
                note: None,
                opening: None,
                white,
            }
        };
        match sel {
            Selection::None => None,
            Selection::Node { white, node } => {
                let cr = report.color(*white);
                let n = cr.tree.nodes.get(*node as usize)?;
                let g = &report.games[*n.games.first()? as usize];
                let mut s = from_game(g, usize::from(n.ply), *white);
                s.opening = n.opening;
                Some(s)
            }
            Selection::Opening {
                white,
                family,
                variation,
            } => {
                let cr = report.color(*white);
                let fam = cr.families.get(*family)?;
                let var = match variation {
                    Some(v) => fam.variations.get(*v)?,
                    None => fam.variations.first()?,
                };
                // The named position: a game of the variation at its named ply; the
                // family's: the shallowest of its variations' names.
                let (g, ply) = match variation {
                    Some(_) => {
                        let g = var
                            .games
                            .iter()
                            .map(|&g| &report.games[g as usize])
                            .find(|g| g.opening == var.opening)?;
                        (g, g.named.unwrap_or(0) as usize)
                    }
                    None if fam.name.is_empty() => {
                        // No name: the start position.
                        let g = var.games.first().map(|&g| &report.games[g as usize])?;
                        (g, 0)
                    }
                    None => {
                        // The family's own position: where its name first appears in
                        // a line (the shallowest among the games).
                        let mut best: Option<(&GameInfo, usize)> = None;
                        for v in &fam.variations {
                            for &gi in &v.games {
                                let g = &report.games[gi as usize];
                                if g.opening.is_none() {
                                    continue;
                                }
                                let ply = family_ply(g, &fam.name).unwrap_or(g.named.unwrap_or(0) as usize);
                                if best.is_none_or(|(_, p)| ply < p) {
                                    best = Some((g, ply));
                                }
                            }
                        }
                        best?
                    }
                };
                let mut s = from_game(g, ply, *white);
                s.opening = var.opening;
                Some(s)
            }
            Selection::Repeated { white, index } => {
                let cr = report.color(*white);
                let r = cr.repeated.get(*index)?;
                let g = &report.games[*r.games.first()? as usize];
                let mut s = from_game(g, r.ply as usize, *white);
                s.arrows = error_arrows(&s.positions[s.ply], &r.uci, cr.errors.iter().find(|e| e.key == r.key && e.uci == r.uci).map(|e| e.best_uci.as_str()), r.judgement);
                s.note = cr
                    .errors
                    .iter()
                    .find(|e| e.key == r.key && e.uci == r.uci)
                    .map(error_note);
                s.opening = r.opening;
                Some(s)
            }
            Selection::Error { white, index } => {
                let cr = report.color(*white);
                let e = cr.errors.get(*index)?;
                let g = &report.games[e.game as usize];
                let mut s = from_game(g, e.ply as usize, *white);
                s.arrows = error_arrows(&s.positions[s.ply], &e.uci, Some(&e.best_uci), e.judgement);
                s.note = Some(error_note(e));
                s.opening = e.opening;
                Some(s)
            }
        }
    }

    /// The games of the selection (indices into the report), newest first.
    fn games_of(&self) -> Vec<u32> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut list: Vec<u32> = match &self.selection {
            Selection::None => self
                .color
                .colours()
                .iter()
                .flat_map(|&w| report.color(w).games.iter().copied())
                .collect(),
            Selection::Node { white, node } => report
                .color(*white)
                .tree
                .nodes
                .get(*node as usize)
                .map(|n| n.games.clone())
                .unwrap_or_default(),
            Selection::Opening {
                white,
                family,
                variation,
            } => report
                .color(*white)
                .families
                .get(*family)
                .map(|f| match variation {
                    Some(v) => f.variations.get(*v).map(|v| v.games.clone()).unwrap_or_default(),
                    None => f.variations.iter().flat_map(|v| v.games.iter().copied()).collect(),
                })
                .unwrap_or_default(),
            Selection::Repeated { white, index } => report
                .color(*white)
                .repeated
                .get(*index)
                .map(|r| r.games.clone())
                .unwrap_or_default(),
            Selection::Error { white, index } => report
                .color(*white)
                .errors
                .get(*index)
                .map(|e| vec![e.game])
                .unwrap_or_default(),
        };
        if self.newest_first {
            list.sort_by_key(|&g| std::cmp::Reverse(report.games[g as usize].date_number()));
        }
        list
    }

    /// The move of the selection to open a game at.
    fn selection_ply(&self) -> usize {
        self.shown_pos.as_ref().map_or(0, |s| s.ucis.len())
    }

    /// The player's colour the board is seen from.
    fn board_white(&self) -> bool {
        let base = self
            .shown_pos
            .as_ref()
            .map_or(self.color != ColorChoice::Black, |s| s.white);
        match self.flipped {
            Some(f) => f == base,
            None => base,
        }
    }

    // ----- the frame -----------------------------------------------------------------

    /// Draws the screen; what the application has to do.
    pub fn show(&mut self, ui: &mut Ui, settings: &mut Settings) -> Vec<PrepAction> {
        let mut actions = Vec::new();
        let ctx = ui.ctx().clone();
        self.poll(&ctx, settings, &mut actions);
        self.hover_arrow = None;
        let typing = ctx.egui_wants_keyboard_input();
        let overlay = crate::app::overlay_open(&ctx) || self.dialog_open;

        egui::Panel::top("prep_header").show(ui, |ui| {
            ui.add_space(4.0);
            self.header(ui, &mut actions);
            ui.add_space(4.0);
        });
        let content = ctx.content_rect();
        let (w, h) = (content.width(), content.height() - 44.0);
        let stacked = w < BOARD_MIN + 24.0 + RIGHT_MIN || h < 600.0;
        let low = stacked && h < 720.0;
        egui::Panel::top("prep_controls").show(ui, |ui| {
            ui.add_space(4.0);
            self.controls(ui, settings, low, &mut actions);
            ui.add_space(2.0);
            self.summary(ui, stacked);
            ui.add_space(4.0);
        });
        if self.report.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.building(ui, &mut actions));
        } else if stacked {
            if !low {
                egui::Panel::top("prep_board_stack").show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.allocate_ui(vec2(240.0, 240.0), |ui| self.board(ui, settings, 240.0));
                        ui.vertical(|ui| self.board_info(ui, &mut actions));
                    });
                    ui.add_space(4.0);
                });
            }
            egui::CentralPanel::default().show(ui, |ui| self.right_column(ui, settings, &mut actions));
            if low && self.board_popup {
                self.board_overlay(&ctx, settings, &mut actions);
            }
        } else {
            let board_side = (h - 120.0).min(420.0).min(w - BOARD_MIN - 24.0 - RIGHT_MIN + BOARD_MIN).max(BOARD_MIN).floor();
            egui::Panel::left("prep_board")
                .exact_size(board_side + 24.0)
                .resizable(false)
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        self.board_caption(ui);
                        self.board(ui, settings, board_side);
                        self.board_info(ui, &mut actions);
                    });
                });
            egui::CentralPanel::default().show(ui, |ui| self.right_column(ui, settings, &mut actions));
        }
        self.engine_dialog(&ctx, settings, &mut actions);
        if !typing && !overlay {
            self.keyboard(&ctx, settings, &mut actions);
        }
        actions
    }

    fn header(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        let gap = ui.spacing().item_spacing.x;
        let button_w = |ui: &Ui, text: &str| {
            text_width(ui, text, egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.x + gap
        };
        let row = ui.available_width();
        let compact = row < 700.0;
        let left = button_w(ui, if compact { "⬅" } else { tr("⬅ Library") }) + 2.0 * gap;
        let right = button_w(ui, if compact { "⚙" } else { tr("⚙ Settings") });
        let title_room = (row - left - right).max(80.0);
        ui.horizontal(|ui| {
            if ui
                .button(if compact { "⬅" } else { tr("⬅ Library") })
                .on_hover_text(tr("Back to the game list (Esc, Ctrl+W)"))
                .clicked()
            {
                actions.push(PrepAction::Back);
            }
            ui.separator();
            let name = self.names.first().cloned().unwrap_or_default();
            let more = self.names.len().saturating_sub(1);
            let mut parts = Vec::new();
            if let Some(r) = &self.report {
                parts.push(crate::i18n::games(r.games.len()));
                let p = period(&r.games);
                if !p.is_empty() {
                    parts.push(p);
                }
            }
            parts.push(self.db_name.clone());
            let title = if compact {
                name.clone()
            } else {
                trf("Preparing against {name}", &[("name", &name)])
            };
            ui.allocate_ui(vec2(title_room, 24.0), |ui| {
                ui.horizontal(|ui| {
                    ui.set_max_width(title_room);
                    ui.add(egui::Label::new(RichText::new(title).strong().size(15.0)).truncate());
                    if more > 0 {
                        ui.label(RichText::new(trf("+{n} spelling", &[("n", &more)])).weak())
                            .on_hover_text(self.names.join("\n"));
                    }
                    let skipped = self.report.as_ref().map_or(0, |r| r.skipped);
                    let r = ui.add(
                        egui::Label::new(RichText::new(format!("· {}", parts.join(" · "))).weak()).truncate(),
                    );
                    if skipped > 0 {
                        r.on_hover_text(trf("not counted: from a set position — {n}", &[("n", &skipped)]));
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if compact { "⚙" } else { tr("⚙ Settings") })
                    .on_hover_text(tr("Settings (Ctrl+,)"))
                    .clicked()
                {
                    actions.push(PrepAction::OpenSettings);
                }
            });
        });
    }

    /// The colour switch, the book, the depth and the engine button or its progress.
    fn controls(&mut self, ui: &mut Ui, settings: &mut Settings, low: bool, actions: &mut Vec<PrepAction>) {
        let width = ui.available_width();
        ui.horizontal(|ui| {
            for choice in [ColorChoice::White, ColorChoice::Black, ColorChoice::Both] {
                if ui.selectable_value(&mut self.color, choice, choice.label()).clicked() {
                    // A selection of a colour no longer shown is dropped.
                    let keep = match &self.selection {
                        Selection::None => true,
                        Selection::Node { white, .. }
                        | Selection::Opening { white, .. }
                        | Selection::Repeated { white, .. }
                        | Selection::Error { white, .. } => self.color.colours().contains(white),
                    };
                    if !keep {
                        self.select(Selection::None);
                    }
                }
            }
            ui.separator();
            let book_w = if width < 900.0 { 150.0 } else { 240.0 };
            if width >= 640.0 {
                ui.label(tr("Book"));
            }
            let own_name = self.db_name.clone();
            if let Some(b) = book_picker(ui, "prep_book", &self.book, &self.path, &own_name, &self.dbs, book_w) {
                self.book = b;
                self.book_failed = None;
                self.book_asked = None;
                settings.prep.book = Some(self.book.setting(&self.path));
                actions.push(PrepAction::SettingsChanged);
            }
            if let Some((_, why)) = &self.book_failed {
                ui.label(RichText::new(trf("Book: could not open — {reason}", &[("reason", why)])).color(ui.visuals().warn_fg_color));
            }
            ui.separator();
            let moves = self.plies.div_ceil(2);
            let depth_label = if width < 760.0 {
                format!("{moves}")
            } else {
                trf("Opening: {n} moves", &[("n", &moves)])
            };
            let mut new_plies = None;
            egui::ComboBox::from_id_salt("prep_plies")
                .selected_text(depth_label)
                .show_ui(ui, |ui| {
                    for m in [6usize, 10, 15] {
                        if ui
                            .selectable_label(self.plies == m * 2, trf("Opening: {n} moves", &[("n", &m)]))
                            .clicked()
                        {
                            new_plies = Some(m * 2);
                        }
                    }
                })
                .response
                .on_hover_text(tr("How deep the tree and the engine look"));
            if let Some(p) = new_plies.filter(|p| *p != self.plies) {
                self.plies = p;
                settings.prep.plies = p;
                actions.push(PrepAction::SettingsChanged);
                self.save();
                self.report = None;
                let dir = cache_dir();
                let options = Options {
                    plies: p,
                    side: Side::Any,
                };
                let cached = dir
                    .as_deref()
                    .and_then(|d| PrepReport::load(d, &self.db, &self.player, &options));
                match cached {
                    Some(r) => self.take_report(r, settings),
                    None => self.rebuild(ui.ctx()),
                }
                self.selection = Selection::None;
                self.shown_pos = None;
            }
            if low {
                ui.separator();
                if ui
                    .selectable_label(self.board_popup, if width < 760.0 { "♙" } else { tr("♙ Board") })
                    .on_hover_text(tr("The board of the chosen position"))
                    .clicked()
                {
                    self.board_popup = !self.board_popup;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.engine_control(ui, settings, width, actions);
            });
        });
    }

    /// The engine button, the progress while it runs, the state after.
    fn engine_control(&mut self, ui: &mut Ui, settings: &Settings, width: f32, actions: &mut Vec<PrepAction>) {
        if let Some(job) = &self.job {
            let (done, total) = (job.done, job.total);
            if ui.button(tr("Stop")).on_hover_text("Shift+A").clicked() {
                self.stop_engine(actions);
                return;
            }
            let left = total.saturating_sub(done);
            let per_thread = self.nps_per_thread.unwrap_or(review::DEFAULT_NPS);
            let secs = review::estimate_secs(left, job.nodes, per_thread, job.threads);
            let text = if width < 760.0 {
                format!("{done}/{total}")
            } else {
                trf(
                    "Finding errors: {done} of {total} positions · ≈ {time}",
                    &[("done", &done), ("total", &total), ("time", &review::format_duration(secs))],
                )
            };
            ui.add(
                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                    .desired_width((width * 0.35).clamp(90.0, 360.0))
                    .text(text),
            );
            return;
        }
        let Some(report) = &self.report else { return };
        let (missing, secs) = self.estimate(settings);
        let time = review::format_duration(secs);
        let done_before = report.engine.is_some();
        let label = if missing == 0 && done_before {
            tr("Errors found").to_owned()
        } else if width < 700.0 {
            format!("▶ ≈ {time}")
        } else if width < 900.0 {
            trf("▶ Errors ≈ {time}", &[("time", &time)])
        } else if done_before {
            trf("Analyse {n} more positions ≈ {time}", &[("n", &missing), ("time", &time)])
        } else {
            trf("▶ Find opening errors ≈ {time}", &[("time", &time)])
        };
        let hint = match &report.engine {
            Some(e) => {
                let quality = REVIEW_QUALITY
                    .iter()
                    .find(|q| q.1 == e.nodes)
                    .map_or_else(|| review::format_count(e.nodes), |q| tr(q.0).to_owned());
                trf(
                    "Errors found · {engine} · {quality} · {n} positions",
                    &[("engine", &e.name), ("quality", &quality), ("n", &report.evals.len())],
                )
            }
            None => tr("The engine finds the opening errors in his games (Shift+A)").to_owned(),
        };
        if missing == 0 && done_before {
            ui.label(RichText::new(label).weak()).on_hover_text(hint);
        } else if crate::widgets::primary(ui, &label).on_hover_text(hint).clicked() {
            self.engine_asked(settings, ui.ctx(), actions);
        }
        if ui
            .button("▾")
            .on_hover_text(tr("Options…"))
            .clicked()
        {
            self.dialog_open = true;
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new("⚠").color(ui.visuals().error_fg_color))
                .on_hover_text(e);
        }
    }

    /// A line per colour: games, score, opponents, the last game, the book.
    fn summary(&self, ui: &mut Ui, stacked: bool) {
        let Some(report) = &self.report else { return };
        let colours = if stacked { self.color.colours() } else { vec![true, false] };
        for white in colours {
            let cr = report.color(white);
            let tally = &cr.tree.nodes[0].tally;
            let weak = !self.color.colours().contains(&white);
            let title = if white { tr("As White") } else { tr("As Black") };
            ui.horizontal_wrapped(|ui| {
                let text = |ui: &mut Ui, s: String| {
                    let rt = RichText::new(s);
                    ui.label(if weak { rt.weak() } else { rt });
                };
                text(ui, format!("{title}  {}", tally.games));
                if tally.games == 0 {
                    return;
                }
                text(ui, format!("· {} ({})", score_text(tally), tally_hint(tally)));
                if !stacked {
                    if let Some(elo) = cr.tree.nodes[0].average_elo() {
                        text(ui, format!("· {}", trf("opponents ≈ {elo}", &[("elo", &elo)])));
                    }
                    let last = crate::library::display_date(&cr.tree.nodes[0].last_date);
                    if !last.is_empty() {
                        text(ui, format!("· {}", trf("last {date}", &[("date", &last)])));
                    }
                }
                if report.book.is_some() {
                    let mut plies: Vec<u32> = cr
                        .games
                        .iter()
                        .filter_map(|&g| report.games[g as usize].book.map(|b| b.ply))
                        .collect();
                    if !plies.is_empty() {
                        plies.sort_unstable();
                        let median = plies[plies.len() / 2];
                        let (own, other) = cr.book_exits(&report.games);
                        let p = 100 * own / (own + other).max(1);
                        text(
                            ui,
                            format!(
                                "· {}",
                                trf(
                                    "leaves the book at move {n}, first in {p} % of games",
                                    &[("n", &median.div_ceil(2).max(1)), ("p", &p)]
                                )
                            ),
                        );
                    }
                } else if self.book_job.is_some() {
                    text(ui, format!("· {}", tr("book…")));
                }
                if !stacked && report.engine.is_some() && cr.counts.total() > 0 {
                    let games_with: HashSet<u32> = cr.errors.iter().map(|e| e.game).collect();
                    let n = games_with.len();
                    let p = 100 * n / cr.games.len().max(1);
                    let c = &cr.counts;
                    ui.label(RichText::new("·").weak());
                    let line = trf(
                        "Opening errors: {i} inaccuracies · {m} mistakes · {b} blunders — in {g} games ({p} %)",
                        &[
                            ("i", &c.inaccuracies),
                            ("m", &c.mistakes),
                            ("b", &c.blunders),
                            ("g", &n),
                            ("p", &p),
                        ],
                    );
                    let rt = RichText::new(line);
                    ui.label(if weak { rt.weak() } else { rt });
                }
            });
        }
        let shown: usize = self
            .color
            .colours()
            .iter()
            .map(|&w| report.color(w).games.len())
            .sum();
        if shown > 0 && shown < 10 {
            ui.label(
                RichText::new(trf(
                    "Few games ({n}): percentages and scores are unreliable",
                    &[("n", &shown)],
                ))
                .color(ui.visuals().warn_fg_color),
            );
        }
    }

    /// The report is being built: its progress.
    fn building(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                if ui.button(tr("Try again")).clicked() {
                    self.error = None;
                    self.rebuild(ui.ctx());
                }
                return;
            }
            let Some(job) = &self.build else { return };
            let done = job.progress();
            let total = self.build_total.max(done).max(1);
            ui.add(egui::Spinner::new().size(18.0));
            let p = 100 * done / total;
            ui.label(trf("Collecting games… {p} %", &[("p", &p.min(99))]));
            if ui.button(tr("Cancel")).clicked() {
                actions.push(PrepAction::Back);
            }
        });
    }

    // ----- the board -----------------------------------------------------------------

    /// «Black to move · after 3.Bb5» over the board.
    fn board_caption(&self, ui: &mut Ui) {
        let text = match &self.shown_pos {
            Some(s) => {
                let pos = &s.positions[s.ply];
                let side = if pos.turn() == Color::White {
                    tr("White to move")
                } else {
                    tr("Black to move")
                };
                match s.ply.checked_sub(1).and_then(|p| s.sans.get(p).map(|san| short_label(p, san))) {
                    Some(m) => format!("{side} · {}", trf("after {move}", &[("move", &m)])),
                    None => side.to_owned(),
                }
            }
            None => tr("Start position").to_owned(),
        };
        ui.label(RichText::new(text).weak());
    }

    fn board(&mut self, ui: &mut Ui, settings: &Settings, side: f32) {
        let start = Chess::default();
        let (pos, last_move, arrows): (&Chess, Option<(Square, Square)>, Vec<Arrow>) = match &self.shown_pos {
            Some(s) => {
                let pos = &s.positions[s.ply];
                let last = s
                    .ply
                    .checked_sub(1)
                    .and_then(|p| s.ucis.get(p))
                    .and_then(|u| u.parse::<UciMove>().ok())
                    .and_then(|u| u.to_move(&s.positions[s.ply - 1]).ok())
                    .and_then(|m| Some((m.from()?, crate::board::ui_target(&m))));
                let mut arrows = if s.ply + 1 == s.positions.len() { s.arrows.clone() } else { Vec::new() };
                if let Some((from, to)) = self.hover_arrow {
                    arrows.push(Arrow {
                        from,
                        to,
                        color: PREVIEW_ARROW,
                        weight: 1.0,
                    });
                }
                (pos, last, arrows)
            }
            None => {
                let mut arrows = Vec::new();
                if let Some((from, to)) = self.hover_arrow {
                    arrows.push(Arrow {
                        from,
                        to,
                        color: PREVIEW_ARROW,
                        weight: 1.0,
                    });
                }
                (&start, None, arrows)
            }
        };
        let view = BoardView {
            pos,
            orientation: if self.board_white() { Color::White } else { Color::Black },
            last_move,
            arrows: &arrows,
            circles: &[],
            badge: None,
            interactive: false,
            show_coords: settings.show_coords,
            theme: settings.board_theme.theme(),
            pieces: settings.piece_set,
            animation: settings.animation.seconds(),
            premove: None,
            auto_queen: settings.auto_queen,
            alert: None,
        };
        ui.allocate_ui(vec2(side, side), |ui| {
            view.show(ui, &mut self.board_state);
        });
    }

    /// Under the board: the line (a click takes the board along it), the opening, the
    /// note of an error, «Open as a game» and the menu.
    fn board_info(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        ui.add_space(4.0);
        let Some(report) = &self.report else { return };
        let Some(s) = &self.shown_pos else {
            ui.label(RichText::new(tr("Choose a move in the tree, an opening or an error: its position is shown here")).weak());
            return;
        };
        let mut goto = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if s.sans.is_empty() {
                ui.label(RichText::new(tr("Start position")).weak());
            }
            for (i, san) in s.sans.iter().enumerate() {
                let label = short_label(i, san);
                let current = i + 1 == s.ply;
                let text = if current {
                    RichText::new(label).strong()
                } else {
                    RichText::new(label)
                };
                if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() {
                    goto = Some(i + 1);
                }
            }
        });
        if let Some(o) = s.opening.and_then(|o| report.openings.get(o)) {
            ui.label(RichText::new(format!("{} · {}", o.name, o.eco)).weak().small());
        }
        if let Some(note) = &s.note {
            ui.label(RichText::new(note).small());
        }
        let (sans, ucis, pos, white, ply) = (s.sans.clone(), s.ucis.clone(), s.positions[s.ply].clone(), s.white, s.ply);
        ui.horizontal(|ui| {
            if ui
                .button(tr("Open as a game"))
                .on_hover_text(tr("The line as a new game, to add moves and comments"))
                .clicked()
            {
                actions.push(PrepAction::OpenPgn(self.line_pgn(&sans[..ply], white)));
            }
            ui.menu_button("⋯", |ui| {
                if ui.button(tr("Play on from here…")).clicked() {
                    let moves: Vec<Move> = {
                        let mut p = Chess::default();
                        ucis[..ply]
                            .iter()
                            .filter_map(|u| {
                                let m = u.parse::<UciMove>().ok()?.to_move(&p).ok()?;
                                p.play_unchecked(m);
                                Some(m)
                            })
                            .collect()
                    };
                    let last = moves.last().and_then(|m| Some((m.from()?, crate::board::ui_target(m))));
                    let after = ply.checked_sub(1).map(|p| prep::move_label(p, &sans[p]));
                    actions.push(PrepAction::PlayFrom(Box::new(crate::play::StartFrom {
                        start_fen: None,
                        moves,
                        position: pos.clone(),
                        last,
                        after: after.clone(),
                        after_shown: after.map(|a| crate::san::show(&a).into_owned()),
                        game: Some(self.title()),
                        null_dropped: false,
                    })));
                    ui.close();
                }
                if ui.button(tr("Find this position in a database (Ctrl+Shift+F)")).clicked() {
                    let mut p = Chess::default();
                    let moves: Vec<Move> = ucis[..ply]
                        .iter()
                        .filter_map(|u| {
                            let m = u.parse::<UciMove>().ok()?.to_move(&p).ok()?;
                            p.play_unchecked(m);
                            Some(m)
                        })
                        .collect();
                    actions.push(PrepAction::FindPosition(self.path.clone(), Chess::default(), moves, !self.board_white()));
                    ui.close();
                }
                if ui.button(tr("Copy FEN")).clicked() {
                    let fen = shakmaty::fen::Fen::from_position(&pos, shakmaty::EnPassantMode::Legal).to_string();
                    actions.push(PrepAction::CopyText(fen, tr("FEN copied")));
                    ui.close();
                }
                if ui.button(tr("Copy moves (PGN)")).clicked() {
                    actions.push(PrepAction::CopyText(self.line_pgn(&sans[..ply], white), tr("PGN copied")));
                    ui.close();
                }
            });
        });
        if let Some(p) = goto
            && let Some(s) = self.shown_pos.as_mut()
        {
            s.ply = p;
            self.board_state.clear();
        }
    }

    /// The line as a PGN game: the player on his side, the opponent unnamed.
    fn line_pgn(&self, sans: &[String], white: bool) -> String {
        let name = self.names.first().cloned().unwrap_or_default();
        let (w, b) = if white { (name, "?".to_owned()) } else { ("?".to_owned(), name) };
        let mut moves = String::new();
        for (i, san) in sans.iter().enumerate() {
            if i.is_multiple_of(2) {
                moves.push_str(&format!("{}. ", i / 2 + 1));
            }
            moves.push_str(san);
            moves.push(' ');
        }
        format!("[Event \"{}\"]\n[White \"{w}\"]\n[Black \"{b}\"]\n[Result \"*\"]\n\n{moves}*\n", self.title())
    }

    /// In a low window: the board over the right bottom of the screen.
    fn board_overlay(&mut self, ctx: &egui::Context, settings: &Settings, actions: &mut Vec<PrepAction>) {
        let screen = ctx.content_rect();
        let size = vec2(264.0, 300.0);
        let rect = egui::Rect::from_min_size(screen.right_bottom() - size, size);
        let area = egui::Area::new(egui::Id::new("prep_board_overlay"))
            .order(egui::Order::Middle)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                let frame = egui::Frame::side_top_panel(ui.style())
                    .stroke(ui.visuals().window_stroke)
                    .shadow(egui::Shadow {
                        offset: [-6, -6],
                        blur: 16,
                        spread: 0,
                        color: Color32::from_black_alpha(110),
                    });
                frame.show(ui, |ui| {
                    ui.set_min_size(size - vec2(16.0, 16.0));
                    ui.set_max_size(size - vec2(16.0, 16.0));
                    self.board_caption(ui);
                    self.board(ui, settings, 240.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("Open as a game")).clicked()
                            && let Some(s) = &self.shown_pos
                        {
                            actions.push(PrepAction::OpenPgn(self.line_pgn(&s.sans[..s.ply], s.white)));
                        }
                    });
                });
            });
        if area.response.clicked_elsewhere() {
            self.board_popup = false;
        }
    }

    // ----- the right column ---------------------------------------------------------

    /// The tabs over the games list, the border between them draggable. (A nested
    /// bottom panel would not do: in a top-down `Ui` egui's available rect ignores
    /// the cursor's bottom, so the content above overlaps it.)
    fn right_column(&mut self, ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<PrepAction>) {
        let height = ui.available_height();
        let width = ui.available_width();
        let least = 4.0 * ROW_H + 60.0;
        let most = (height - 160.0).max(least);
        let games_h = self
            .games_height
            .unwrap_or(height * 0.35)
            .clamp(least, most)
            .floor();
        let tabs_h = (height - games_h - 10.0).max(0.0);
        ui.allocate_ui_with_layout(vec2(width, tabs_h), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_height(tabs_h);
            ui.set_max_height(tabs_h);
            self.tabs(ui, settings, actions);
        });
        // The border: dragged, it gives the games list more or less.
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 10.0), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if resp.dragged() {
            self.games_height = Some((games_h - resp.drag_delta().y).clamp(least, most));
        }
        let stroke = if resp.hovered() || resp.dragged() {
            ui.visuals().widgets.hovered.fg_stroke
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        };
        ui.painter().hline(rect.x_range(), rect.center().y, stroke);
        ui.allocate_ui_with_layout(vec2(width, games_h), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_height(games_h);
            ui.set_max_height(games_h);
            self.games_list(ui, actions);
        });
    }

    fn tabs(&mut self, ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<PrepAction>) {
        {
            ui.horizontal(|ui| {
                for (tab, label, key) in [
                    (Tab::Repertoire, tr("Repertoire"), "Ctrl+1"),
                    (Tab::Openings, tr("Openings"), "Ctrl+2"),
                    (Tab::Errors, tr("Errors"), "Ctrl+3"),
                ] {
                    ui.selectable_value(&mut self.tab, tab, label).on_hover_text(key);
                }
                if self.tab == Tab::Repertoire {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(tr("Collapse")).clicked() {
                            self.collapse_selected();
                        }
                        if ui.small_button(tr("Expand all")).clicked() {
                            self.expand_selected();
                        }
                        let mut n = self.branches_from;
                        egui::ComboBox::from_id_salt("prep_branches")
                            .selected_text(trf("Branches from {n} games", &[("n", &n)]))
                            .show_ui(ui, |ui| {
                                for k in [2u32, 3, 5] {
                                    ui.selectable_value(&mut n, k, trf("Branches from {n} games", &[("n", &k)]));
                                }
                            })
                            .response
                            .on_hover_text(tr("Moves with fewer games fold into «other moves»"));
                        if n != self.branches_from {
                            self.branches_from = n;
                            settings.prep.branches_from = n;
                            actions.push(PrepAction::SettingsChanged);
                        }
                    });
                }
            });
            ui.add_space(2.0);
            match self.tab {
                Tab::Repertoire => self.tree_table(ui),
                Tab::Openings => self.openings_table(ui),
                Tab::Errors => self.errors_tab(ui, settings, actions),
            }
        }
    }

    fn expand_selected(&mut self) {
        let Some(report) = &self.report else { return };
        let roots: Vec<(bool, u32)> = match &self.selection {
            Selection::Node { white, node } => vec![(*white, *node)],
            _ => self.color.colours().iter().map(|&w| (w, 0)).collect(),
        };
        for (white, root) in roots {
            let tree = &report.color(white).tree;
            let mut stack = vec![root];
            while let Some(n) = stack.pop() {
                self.expanded.insert((white, n));
                self.others_open.insert((white, n));
                stack.extend(tree.nodes[n as usize].children.iter().copied());
            }
        }
    }

    fn collapse_selected(&mut self) {
        let Some(report) = &self.report else { return };
        match &self.selection {
            Selection::Node { white, node } => {
                let tree = &report.color(*white).tree;
                let mut stack = vec![*node];
                while let Some(n) = stack.pop() {
                    self.expanded.remove(&(*white, n));
                    self.others_open.remove(&(*white, n));
                    stack.extend(tree.nodes[n as usize].children.iter().copied());
                }
            }
            _ => {
                self.expanded.retain(|(_, n)| *n == 0);
                self.others_open.clear();
            }
        }
    }

    /// The rows of the tree as laid out for the colour switch and the expansion.
    fn tree_rows(&self) -> Vec<TreeRow> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for white in self.color.colours() {
            let tree = &report.color(white).tree;
            if tree.nodes.is_empty() {
                continue;
            }
            // (node, depth, after the main children: the others)
            let mut stack: Vec<TreeRow> = vec![TreeRow::Node {
                white,
                node: 0,
                depth: 0,
            }];
            while let Some(row) = stack.pop() {
                rows.push(row);
                let (node, depth, listed) = match row {
                    TreeRow::Node { node, depth, .. } => {
                        if !self.expanded.contains(&(white, node)) {
                            continue;
                        }
                        (node, depth, false)
                    }
                    TreeRow::Others { parent, depth, .. } => {
                        if !self.others_open.contains(&(white, parent)) {
                            continue;
                        }
                        // The folded moves go under the «other moves» row.
                        (parent, depth, true)
                    }
                };
                let n = &tree.nodes[node as usize];
                let (main, rest): (Vec<u32>, Vec<u32>) = n
                    .children
                    .iter()
                    .copied()
                    .partition(|&c| tree.nodes[c as usize].tally.games >= self.branches_from);
                // Pushed in reverse: the stack pops the first child first.
                if listed {
                    for &c in rest.iter().rev() {
                        stack.push(TreeRow::Node {
                            white,
                            node: c,
                            depth: depth + 1,
                        });
                    }
                } else {
                    if !rest.is_empty() {
                        stack.push(TreeRow::Others {
                            white,
                            parent: node,
                            depth: depth + 1,
                            count: rest.len(),
                        });
                    }
                    for &c in main.iter().rev() {
                        stack.push(TreeRow::Node {
                            white,
                            node: c,
                            depth: depth + 1,
                        });
                    }
                }
            }
        }
        rows
    }

    fn tree_table(&mut self, ui: &mut Ui) {
        let rows = self.tree_rows();
        if rows.len() <= 1 {
            self.no_games_note(ui);
        }
        let Some(report) = self.report.as_ref() else { return };
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let has_book = report.book.is_some() || self.book_job.is_some();
        let mut used = 180.0 + 56.0 + 40.0 + 50.0 + 4.0 * gap;
        let mut fits = |w: f32, wanted: bool| {
            let ok = wanted && used + w + gap <= width;
            if ok {
                used += w + gap;
            }
            ok
        };
        let show_opening = fits(160.0, true);
        let show_bar = fits(64.0, true);
        let show_last = fits(84.0, true);
        let show_book = fits(80.0, has_book);
        let book_title = if self.book == BookChoice::Same { tr("In database") } else { tr("Book") };
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("prep_tree")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true));
        if show_opening {
            table = table.column(Column::remainder().at_least(160.0).clip(true));
        }
        table = table.column(Column::exact(56.0)).column(Column::exact(40.0));
        table = table.column(Column::exact(if show_bar { 110.0 } else { 50.0 }));
        if show_last {
            table = table.column(Column::exact(84.0));
        }
        if show_book {
            table = table.column(Column::exact(80.0));
        }
        let selected_row = rows.iter().position(|r| match (r, &self.selection) {
            (TreeRow::Node { white, node, .. }, Selection::Node { white: sw, node: sn }) => white == sw && node == sn,
            _ => false,
        });
        if self.scroll_to_selected
            && let Some(i) = selected_row
        {
            table = table.scroll_to_row(i, None);
        }
        self.scroll_to_selected = false;
        let mut clicked: Option<TreeRow> = None;
        let mut toggled: Option<TreeRow> = None;
        let mut hovered: Option<(bool, u32)> = None;
        let threshold = report.book.as_ref().map_or(prep::DEFAULT_BOOK_THRESHOLD, |b| b.threshold);
        table
            .header(ROW_H, |mut h| {
                head_col(&mut h, tr("Move"), tr("His moves plain, the opponents' weak; ⇄ — the position is reached by another move order too"));
                if show_opening {
                    head_col(&mut h, tr("Opening"), "");
                }
                head_col(&mut h, tr("Games"), "");
                head_col(&mut h, "%", tr("Share of the games of the move above"));
                head_col(&mut h, tr("Score"), tr("The opponent's points in these games"));
                if show_last {
                    head_col(&mut h, tr("Last"), "");
                }
                if show_book {
                    head_col(&mut h, book_title, tr("Games of the book in this position; the first number under the threshold is where he leaves the book"));
                }
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let r = rows[row.index()];
                    row.set_selected(Some(row.index()) == selected_row);
                    match r {
                        TreeRow::Others { white, parent, depth, count } => {
                            let open = self.others_open.contains(&(white, parent));
                            row.col(|ui| {
                                ui.add_space(depth as f32 * 14.0);
                                let glyph = if open { "▾" } else { "▸" };
                                if ui.add(egui::Label::new(RichText::new(glyph).weak()).sense(Sense::click())).clicked() {
                                    toggled = Some(r);
                                }
                                ui.label(RichText::new(trf("other moves ({n})", &[("n", &count)])).weak());
                            });
                            let rest = 2 + usize::from(show_opening) + 1 + usize::from(show_last) + usize::from(show_book);
                            for _ in 0..rest {
                                row.col(|_| {});
                            }
                        }
                        TreeRow::Node { white, node, depth } => {
                            let cr = report.color(white);
                            let n = &cr.tree.nodes[node as usize];
                            let parent = n.parent.map(|p| &cr.tree.nodes[p as usize]);
                            let has_children = !n.children.is_empty();
                            let open = self.expanded.contains(&(white, node));
                            let few = n.tally.games < FEW_GAMES;
                            row.col(|ui| {
                                ui.add_space(depth as f32 * 14.0);
                                let glyph = if !has_children { " " } else if open { "▾" } else { "▸" };
                                let g = ui.add(egui::Label::new(RichText::new(glyph).weak()).sense(Sense::click()));
                                if g.clicked() && has_children {
                                    toggled = Some(r);
                                }
                                if n.parent.is_none() {
                                    let title = if white { tr("As White") } else { tr("As Black") };
                                    ui.label(RichText::new(format!("{title} · {}", n.tally.games)).strong());
                                    return;
                                }
                                let ply = usize::from(n.ply) - 1;
                                let players = ply.is_multiple_of(2) == white;
                                let label = short_label(ply, &n.san);
                                let text = if players { RichText::new(label) } else { RichText::new(label).weak() };
                                let l = ui.add(egui::Label::new(text).truncate());
                                if !show_opening && let Some(o) = n.opening.filter(|_| n.named).and_then(|o| report.openings.get(o)) {
                                    l.on_hover_text(format!("{} · {}", o.name, o.eco));
                                }
                                if n.transposition {
                                    let others: Vec<String> = cr
                                        .tree
                                        .nodes
                                        .iter()
                                        .enumerate()
                                        .filter(|(i, m)| m.key == n.key && *i != node as usize)
                                        .map(|(i, _)| line_text(&cr.tree.path(i).iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()))
                                        .collect();
                                    ui.label(RichText::new("⇄").weak()).on_hover_text(trf(
                                        "Reached by another move order too: {orders}",
                                        &[("orders", &others.join("; "))],
                                    ));
                                }
                                if let Some(p) = parent
                                    && let Some(m) = self.mark_of(white, p.key, &n.uci)
                                {
                                    let best = crate::san::show(&m.best_san).into_owned();
                                    pill(ui, m.judgement).on_hover_text(format!(
                                        "{}: {}",
                                        review::judgement_name(m.judgement),
                                        trf(
                                            "Mistake: better {best} ({before} → {after}) · in {n} games",
                                            &[("best", &best), ("before", &m.before.format()), ("after", &m.after.format()), ("n", &m.games)],
                                        )
                                        .trim_start_matches(&format!("{}: ", tr("Mistake")))
                                    ));
                                    if m.games > 1 {
                                        ui.label(RichText::new(format!("×{}", m.games)).weak().small());
                                    }
                                }
                            });
                            if show_opening {
                                row.col(|ui| {
                                    // The name where it first appears (the table names
                                    // 2.Nf3 «Sicilian Defense» again: not repeated).
                                    let new_name = n.named
                                        && parent.is_none_or(|p| {
                                            p.opening.map(|o| report.openings[o].name.as_str())
                                                != n.opening.map(|o| report.openings[o].name.as_str())
                                        });
                                    if new_name && let Some(o) = n.opening.and_then(|o| report.openings.get(o)) {
                                        ui.add(egui::Label::new(RichText::new(&o.name).weak()).truncate())
                                            .on_hover_text(format!("{} · {}", o.name, o.eco));
                                        ui.label(RichText::new(&o.eco).weak().small());
                                    }
                                });
                            }
                            row.col(|ui| {
                                ui.label(n.tally.games.to_string());
                            });
                            row.col(|ui| {
                                if let Some(p) = parent {
                                    let pct = 100 * n.tally.games / p.tally.games.max(1);
                                    let t = RichText::new(format!("{pct}"));
                                    let l = ui.label(if few { t.weak() } else { t });
                                    if few {
                                        l.on_hover_text(tr("Few games: percentages are unreliable"));
                                    }
                                }
                            });
                            row.col(|ui| {
                                let t = RichText::new(score_text(&n.tally));
                                let l = ui.label(if few { t.weak() } else { t });
                                l.on_hover_text(tally_hint(&n.tally));
                                if show_bar {
                                    let (rect, _) = ui.allocate_exact_size(vec2(56.0, 14.0), Sense::hover());
                                    let painter_alpha = if few { 0.5 } else { 1.0 };
                                    let _ = painter_alpha;
                                    board_search::results_bar(ui, rect, &bar_tally(&n.tally, white), ui.visuals().text_color());
                                }
                            });
                            if show_last {
                                row.col(|ui| {
                                    let d = crate::library::display_date(&n.last_date);
                                    ui.label(RichText::new(if d.is_empty() { "—".to_owned() } else { d }).weak());
                                });
                            }
                            if show_book {
                                row.col(|ui| {
                                    match self.book_count(n.key) {
                                        Some(c) if c < threshold => {
                                            let parent_in = parent.and_then(|p| self.book_count(p.key)).is_none_or(|pc| pc >= threshold);
                                            let t = RichText::new(crate::large_db::grouped(c as usize));
                                            let l = ui.label(if parent_in { t.color(ui.visuals().warn_fg_color) } else { t.weak() });
                                            if parent_in {
                                                let book = match &self.book {
                                                    BookChoice::Database(p) => base_name(p),
                                                    _ => self.db_name.clone(),
                                                };
                                                l.on_hover_text(trf(
                                                    "Here he leaves the book: in “{book}” this position occurs {n} times",
                                                    &[("book", &book), ("n", &c)],
                                                ));
                                            }
                                        }
                                        Some(c) => {
                                            ui.label(RichText::new(crate::large_db::grouped(c as usize)).weak());
                                        }
                                        None if self.book_job.is_some() => {
                                            ui.label(RichText::new("…").weak());
                                        }
                                        None => {}
                                    }
                                });
                            }
                            let resp = row.response();
                            if resp.hovered() {
                                hovered = Some((white, node));
                            }
                            if resp.double_clicked() && has_children {
                                toggled = Some(r);
                            } else if resp.clicked() {
                                clicked = Some(r);
                            }
                        }
                    }
                });
            });
        // A child of the chosen node under the pointer: its move as an arrow.
        if let (Some((white, node)), Selection::Node { white: sw, node: sn }) = (hovered, &self.selection)
            && white == *sw
        {
            let cr = report.color(white);
            let n = &cr.tree.nodes[node as usize];
            if n.parent == Some(*sn)
                && let Some(s) = &self.shown_pos
                && s.ply + 1 == s.positions.len()
                && let Some(m) = n.uci.parse::<UciMove>().ok().and_then(|u| u.to_move(&s.positions[s.ply]).ok())
            {
                self.hover_arrow = m.from().map(|f| (f, crate::board::ui_target(&m)));
            }
        }
        if let Some(t) = toggled {
            match t {
                TreeRow::Node { white, node, .. } => {
                    if !self.expanded.remove(&(white, node)) {
                        self.expanded.insert((white, node));
                    }
                }
                TreeRow::Others { white, parent, .. } => {
                    if !self.others_open.remove(&(white, parent)) {
                        self.others_open.insert((white, parent));
                    }
                }
            }
        } else if let Some(TreeRow::Node { white, node, .. }) = clicked {
            self.focus_games = false;
            self.select(Selection::Node { white, node });
        } else if let Some(TreeRow::Others { white, parent, .. }) = clicked
            && !self.others_open.remove(&(white, parent))
        {
            self.others_open.insert((white, parent));
        }
    }

    /// «No games as Black. As White — 138 · Show White» when the colour has none.
    fn no_games_note(&mut self, ui: &mut Ui) {
        let Some(report) = &self.report else { return };
        let (shown, other) = match self.color {
            ColorChoice::White => (true, false),
            ColorChoice::Black => (false, true),
            ColorChoice::Both => return,
        };
        if !report.color(shown).games.is_empty() {
            return;
        }
        let other_n = report.color(other).games.len();
        let (colour, other_name) = if shown { (tr("White"), tr("Black")) } else { (tr("Black"), tr("White")) };
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(trf("No games as {colour}. As {other} — {n}", &[("colour", &colour), ("other", &other_name), ("n", &other_n)]));
            if other_n > 0 && ui.link(trf("Show {other}", &[("other", &other_name)])).clicked() {
                self.color = ColorChoice::from_side(if other { Side::White } else { Side::Black });
            }
        });
    }

    // ----- the openings --------------------------------------------------------------

    fn openings_table(&mut self, ui: &mut Ui) {
        let Some(report) = self.report.as_ref() else { return };
        // (white, family, variation): the rows as laid out.
        let mut rows: Vec<(bool, usize, Option<usize>)> = Vec::new();
        for white in self.color.colours() {
            let cr = report.color(white);
            for (f, fam) in cr.families.iter().enumerate() {
                rows.push((white, f, None));
                if self.open_families.contains(&(white, f)) {
                    for v in 0..fam.variations.len() {
                        rows.push((white, f, Some(v)));
                    }
                }
            }
        }
        if rows.is_empty() {
            self.no_games_note(ui);
            return;
        }
        let both = self.color == ColorChoice::Both;
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let mut used = 200.0 + 56.0 + 40.0 + 50.0 + 4.0 * gap + if both { 90.0 + gap } else { 0.0 };
        let mut fits = |w: f32, wanted: bool| {
            let ok = wanted && used + w + gap <= width;
            if ok {
                used += w + gap;
            }
            ok
        };
        // The errors stay as long as possible; the first moves give way first.
        let show_errors = fits(90.0, report.engine.is_some());
        let show_bar = fits(64.0, true);
        let show_eco = fits(64.0, true);
        let show_last = fits(84.0, true);
        let show_moves = fits(160.0, true);
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("prep_openings")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if both {
            table = table.column(Column::exact(90.0));
        }
        table = table.column(Column::remainder().at_least(200.0).clip(true));
        if show_eco {
            table = table.column(Column::exact(64.0));
        }
        if show_moves {
            table = table.column(Column::remainder().at_least(160.0).clip(true));
        }
        table = table.column(Column::exact(56.0)).column(Column::exact(40.0));
        table = table.column(Column::exact(if show_bar { 110.0 } else { 50.0 }));
        if show_last {
            table = table.column(Column::exact(84.0));
        }
        if show_errors {
            table = table.column(Column::exact(90.0));
        }
        let selected_row = rows.iter().position(|&(w, f, v)| {
            self.selection == Selection::Opening { white: w, family: f, variation: v }
        });
        if self.scroll_to_selected
            && let Some(i) = selected_row
        {
            table = table.scroll_to_row(i, None);
        }
        self.scroll_to_selected = false;
        let mut clicked = None;
        let mut toggled = None;
        table
            .header(ROW_H, |mut h| {
                if both {
                    head_col(&mut h, tr("Colour"), "");
                }
                head_col(&mut h, tr("Opening"), "");
                if show_eco {
                    head_col(&mut h, "ECO", "");
                }
                if show_moves {
                    head_col(&mut h, tr("First moves"), "");
                }
                head_col(&mut h, tr("Games"), "");
                head_col(&mut h, "%", "");
                head_col(&mut h, tr("Score"), tr("The opponent's points in these games"));
                if show_last {
                    head_col(&mut h, tr("Last"), "");
                }
                if show_errors {
                    head_col(&mut h, tr("Errors"), "");
                }
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let (white, f, v) = rows[row.index()];
                    row.set_selected(Some(row.index()) == selected_row);
                    let cr = report.color(white);
                    let fam = &cr.families[f];
                    let colour_games = cr.games.len() as u32;
                    let (tally, errors, games): (&Tally, &ErrorCounts, Vec<u32>) = match v {
                        Some(v) => {
                            let var = &fam.variations[v];
                            (&var.tally, &var.errors, var.games.clone())
                        }
                        None => (&fam.tally, &fam.errors, fam.variations.iter().flat_map(|v| v.games.iter().copied()).collect()),
                    };
                    let few = tally.games < FEW_GAMES;
                    if both {
                        row.col(|ui| {
                            if v.is_none() {
                                ui.label(RichText::new(if white { tr("♙ White") } else { tr("♟ Black") }).weak());
                            }
                        });
                    }
                    row.col(|ui| match v {
                        None => {
                            let open = self.open_families.contains(&(white, f));
                            let glyph = if open { "▾" } else { "▸" };
                            if ui.add(egui::Label::new(RichText::new(glyph).weak()).sense(Sense::click())).clicked() {
                                toggled = Some((white, f));
                            }
                            let name = if fam.name.is_empty() { tr("no name").to_owned() } else { fam.name.clone() };
                            ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                        }
                        Some(v) => {
                            ui.add_space(18.0);
                            let name = fam.variations[v]
                                .opening
                                .and_then(|o| report.openings.get(o))
                                .map(|o| o.variation().map_or_else(|| o.name.clone(), str::to_owned));
                            match name {
                                Some(n) => {
                                    ui.add(egui::Label::new(n).truncate());
                                }
                                None => {
                                    ui.label(RichText::new(tr("no deeper name")).weak());
                                }
                            }
                        }
                    });
                    if show_eco {
                        row.col(|ui| {
                            let eco = match v {
                                Some(v) => fam.variations[v].opening.and_then(|o| report.openings.get(o)).map(|o| o.eco.clone()),
                                None => {
                                    let mut ecos: Vec<&str> = fam
                                        .variations
                                        .iter()
                                        .filter_map(|v| v.opening.and_then(|o| report.openings.get(o)))
                                        .map(|o| o.eco.as_str())
                                        .collect();
                                    ecos.sort_unstable();
                                    ecos.dedup();
                                    match (ecos.first(), ecos.last()) {
                                        (Some(a), Some(b)) if a != b => Some(format!("{a}–{b}")),
                                        (Some(a), _) => Some((*a).to_owned()),
                                        _ => None,
                                    }
                                }
                            };
                            if let Some(e) = eco {
                                ui.label(RichText::new(e).weak());
                            }
                        });
                    }
                    if show_moves {
                        row.col(|ui| {
                            let line = match v {
                                Some(v) => fam.variations[v].opening.and_then(|o| report.openings.get(o)).map(|o| o.line.clone()),
                                None => family_line(report, fam),
                            };
                            if let Some(l) = line {
                                let shown = crate::san::show_line(&shorten_middle(&l, 40));
                                ui.add(egui::Label::new(RichText::new(shown).weak()).truncate())
                                    .on_hover_text(crate::san::show_line(&l));
                            }
                        });
                    }
                    row.col(|ui| {
                        ui.label(tally.games.to_string());
                    });
                    row.col(|ui| {
                        let of = match v {
                            Some(_) => fam.tally.games,
                            None => colour_games,
                        };
                        let pct = 100 * tally.games / of.max(1);
                        let t = RichText::new(format!("{pct}"));
                        ui.label(if few { t.weak() } else { t });
                    });
                    row.col(|ui| {
                        let t = RichText::new(score_text(tally));
                        ui.label(if few { t.weak() } else { t }).on_hover_text(tally_hint(tally));
                        if show_bar {
                            let (rect, _) = ui.allocate_exact_size(vec2(56.0, 14.0), Sense::hover());
                            board_search::results_bar(ui, rect, &bar_tally(tally, white), ui.visuals().text_color());
                        }
                    });
                    if show_last {
                        row.col(|ui| {
                            let last = games
                                .iter()
                                .map(|&g| &report.games[g as usize])
                                .max_by_key(|g| g.date_number())
                                .map(|g| crate::library::display_date(&g.date))
                                .unwrap_or_default();
                            ui.label(RichText::new(if last.is_empty() { "—".to_owned() } else { last }).weak());
                        });
                    }
                    if show_errors {
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            counts_text(ui, errors);
                        });
                    }
                    let resp = row.response();
                    if resp.double_clicked() && v.is_none() {
                        toggled = Some((white, f));
                    } else if resp.clicked() {
                        clicked = Some((white, f, v));
                    }
                });
            });
        if let Some(key) = toggled {
            if !self.open_families.remove(&key) {
                self.open_families.insert(key);
            }
        } else if let Some((white, family, variation)) = clicked {
            self.focus_games = false;
            self.select(Selection::Opening { white, family, variation });
        }
    }

    // ----- the errors ----------------------------------------------------------------

    fn errors_tab(&mut self, ui: &mut Ui, settings: &Settings, actions: &mut Vec<PrepAction>) {
        let Some(report) = self.report.as_ref() else { return };
        let moves = self.plies.div_ceil(2);
        if report.engine.is_none() && self.job.is_none() {
            // The invitation (§5).
            let (positions, secs) = self.estimate(settings);
            let games: usize = self.color.colours().iter().map(|&w| report.color(w).games.len()).sum();
            let colour = match self.color {
                ColorChoice::White => tr("as White"),
                ColorChoice::Black => tr("as Black"),
                ColorChoice::Both => tr("in both colours"),
            };
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.set_max_width(520.0);
                ui.label(trf(
                    "The engine has not searched for errors yet. It will check the first {n} moves of {games} games {colour} — {positions} positions, about {time}.",
                    &[
                        ("n", &moves),
                        ("games", &games),
                        ("colour", &colour),
                        ("positions", &positions),
                        ("time", &review::format_duration(secs)),
                    ],
                ));
                ui.add_space(12.0);
                if crate::widgets::primary(ui, tr("▶ Find opening errors")).clicked() {
                    self.engine_asked(settings, ui.ctx(), actions);
                }
            });
            return;
        }
        let dark = ui.visuals().dark_mode;
        let (mut ask_engine, mut open_dialog) = (false, false);
        // The state line and the summary per colour.
        ui.horizontal_wrapped(|ui| {
            match (&self.job, &report.engine) {
                (Some(job), _) => {
                    let left = job.total.saturating_sub(job.done);
                    let per_thread = self.nps_per_thread.unwrap_or(review::DEFAULT_NPS);
                    let secs = review::estimate_secs(left, job.nodes, per_thread, job.threads);
                    ui.label(
                        RichText::new(trf(
                            "Checked {done} of {total} positions",
                            &[("done", &job.done), ("total", &job.total)],
                        ))
                        .weak(),
                    );
                    ui.label(RichText::new(format!("· ≈ {}", review::format_duration(secs))).weak());
                }
                (None, Some(e)) => {
                    let quality = REVIEW_QUALITY
                        .iter()
                        .find(|q| q.1 == e.nodes)
                        .map_or_else(|| review::format_count(e.nodes), |q| tr(q.0).to_owned());
                    ui.label(
                        RichText::new(trf(
                            "Errors found · {engine} · {quality} · {n} positions",
                            &[("engine", &e.name), ("quality", &quality), ("n", &report.evals.len())],
                        ))
                        .weak(),
                    );
                    let (missing, secs) = self.estimate(settings);
                    if missing > 0 {
                        ui.label(RichText::new("·").weak());
                        if ui
                            .link(trf("Analyse {n} more positions ≈ {time}", &[("n", &missing), ("time", &review::format_duration(secs))]))
                            .clicked()
                        {
                            ask_engine = true;
                        }
                    }
                    if settings.prep.quality == 0 {
                        ui.label(RichText::new("·").weak());
                        if ui.link(tr("More precisely…")).on_hover_text(tr("The same positions at «Normal» quality")).clicked() {
                            open_dialog = true;
                        }
                    }
                }
                (None, None) => {}
            }
        });
        if ask_engine {
            self.engine_asked(settings, ui.ctx(), actions);
            return;
        }
        if open_dialog {
            self.dialog_open = true;
        }
        let Some(report) = self.report.as_ref() else { return };
        for white in self.color.colours() {
            let cr = report.color(white);
            let games_with: HashSet<u32> = cr.errors.iter().map(|e| e.game).collect();
            let c = &cr.counts;
            let title = if white { tr("As White") } else { tr("As Black") };
            let p = 100 * games_with.len() / cr.games.len().max(1);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{title}:")).strong());
                let text = trf(
                    "{i} inaccuracies · {m} mistakes · {b} blunders — in {g} games of {t} ({p} %)",
                    &[
                        ("i", &c.inaccuracies),
                        ("m", &c.mistakes),
                        ("b", &c.blunders),
                        ("g", &games_with.len()),
                        ("t", &cr.games.len()),
                        ("p", &p),
                    ],
                );
                let l = ui.label(text);
                if self.job.is_some() {
                    l.on_hover_text(tr("of the checked positions"));
                }
            });
        }
        let total_errors: u32 = self.color.colours().iter().map(|&w| report.color(w).counts.total()).sum();
        let serious: u32 = self
            .color
            .colours()
            .iter()
            .map(|&w| {
                let c = &report.color(w).counts;
                c.mistakes + c.blunders
            })
            .sum();
        if self.job.is_none() && serious == 0 {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(trf("No errors in the first {n} moves ({i} inaccuracies).", &[("n", &moves), ("i", &(total_errors - serious))]));
                if self.plies < 30 && ui.link(trf("Deeper: {n} moves", &[("n", &15)])).clicked() {
                    // The depth picker's choice, as if chosen there.
                    self.plies = 30;
                    self.report = None;
                    self.rebuild(ui.ctx());
                    actions.push(PrepAction::SettingsChanged);
                }
            });
            if total_errors == 0 {
                return;
            }
        }
        ui.add_space(6.0);
        let both = self.color == ColorChoice::Both;
        egui::ScrollArea::vertical()
            .id_salt("prep_errors")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().interaction.selectable_labels = false;
                self.repeated_list(ui, both, dark, actions);
                ui.add_space(10.0);
                self.by_opening_list(ui, both, dark, actions);
            });
    }

    /// «Repeated errors — the same position, the same move».
    fn repeated_list(&mut self, ui: &mut Ui, both: bool, dark: bool, actions: &mut Vec<PrepAction>) {
        let Some(report) = self.report.as_ref() else { return };
        let mut list: Vec<(bool, usize)> = Vec::new();
        for white in self.color.colours() {
            for i in 0..report.color(white).repeated.len() {
                list.push((white, i));
            }
        }
        if both {
            list.sort_by(|a, b| {
                let ra = &report.color(a.0).repeated[a.1];
                let rb = &report.color(b.0).repeated[b.1];
                rb.games.len().cmp(&ra.games.len()).then(judgement_weight(rb.judgement).cmp(&judgement_weight(ra.judgement)))
            });
        }
        ui.label(RichText::new(tr("Repeated errors — the same position, the same move")).strong());
        if list.is_empty() {
            ui.label(RichText::new(tr("None so far")).weak());
            return;
        }
        let width = ui.available_width();
        let show_opening = width > 700.0;
        let show_score = width > 560.0;
        let mut clicked = None;
        let mut opened = None;
        let mut toggled = None;
        egui::Grid::new("prep_repeated")
            .num_columns(5 + usize::from(show_score) + usize::from(show_opening) + usize::from(both))
            .striped(true)
            .spacing(vec2(10.0, 4.0))
            .show(ui, |ui| {
                let head = |ui: &mut Ui, t: &str| {
                    ui.label(RichText::new(t).strong());
                };
                head(ui, "");
                if both {
                    head(ui, tr("Colour"));
                }
                head(ui, tr("Position"));
                head(ui, tr("Played"));
                head(ui, tr("Better"));
                head(ui, "×");
                if show_score {
                    head(ui, tr("Score"));
                }
                if show_opening {
                    head(ui, tr("Opening"));
                }
                ui.end_row();
                for &(white, i) in &list {
                    let cr = report.color(white);
                    let r = &cr.repeated[i];
                    let selected = self.selection == Selection::Repeated { white, index: i };
                    let open = self.open_repeated.contains(&(white, i));
                    let g = &report.games[r.games[0] as usize];
                    let glyph = if open { "▾" } else { "▸" };
                    if ui.add(egui::Label::new(RichText::new(glyph).weak()).sense(Sense::click())).clicked() {
                        toggled = Some((white, i));
                    }
                    if both {
                        ui.label(RichText::new(if white { tr("♙ White") } else { tr("♟ Black") }).weak());
                    }
                    let before = (r.ply as usize)
                        .checked_sub(1)
                        .map(|p| short_label(p, &g.sans[p]))
                        .map_or_else(|| tr("start").to_owned(), |m| trf("after {move}", &[("move", &m)]));
                    let side = if r.ply.is_multiple_of(2) { tr("White to move") } else { tr("Black to move") };
                    let pos_text = format!("{before} · {side}");
                    let t = RichText::new(pos_text);
                    let resp = ui.add(egui::Label::new(if selected { t.strong() } else { t }).sense(Sense::click()));
                    if resp.clicked() {
                        clicked = Some((white, i));
                    }
                    if resp.double_clicked() {
                        opened = Some((r.games[0], r.ply as usize));
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(short_label(r.ply as usize, &r.san));
                        pill(ui, r.judgement).on_hover_text(review::judgement_name(r.judgement));
                    });
                    ui.label(short_label(r.ply as usize, &r.best_san));
                    ui.label(r.times().to_string());
                    if show_score {
                        ui.label(RichText::new(tally_hint(&r.tally)).weak());
                    }
                    if show_opening {
                        let name = r.opening.and_then(|o| report.openings.get(o)).map(|o| format!("{} · {}", o.name, o.eco)).unwrap_or_default();
                        ui.label(RichText::new(name).weak());
                    }
                    ui.end_row();
                    if open {
                        for &gi in &r.games {
                            let g = &report.games[gi as usize];
                            let row = &self.rows[gi as usize];
                            ui.label("");
                            if both {
                                ui.label("");
                            }
                            let text = format!("{} — {}", row.white, row.black);
                            let resp = ui.add(egui::Label::new(RichText::new(text).weak()).sense(Sense::click()).truncate());
                            if resp.double_clicked() {
                                opened = Some((gi, r.ply as usize));
                            }
                            ui.label(RichText::new(result_text(row.result)).weak());
                            ui.label(RichText::new(crate::library::display_date(&g.date)).weak());
                            ui.end_row();
                        }
                    }
                }
            });
        let _ = dark;
        let opened = opened.map(|(gi, ply)| (report.games[gi as usize].index as usize, ply));
        if let Some(k) = toggled
            && !self.open_repeated.remove(&k)
        {
            self.open_repeated.insert(k);
        }
        if let Some((white, index)) = clicked {
            self.focus_games = false;
            self.select(Selection::Repeated { white, index });
        }
        if let Some((index, ply)) = opened {
            actions.push(PrepAction::OpenGame {
                path: self.path.clone(),
                index,
                ply,
            });
        }
    }

    /// «By opening»: the variations with errors, the worst first, their errors by game.
    fn by_opening_list(&mut self, ui: &mut Ui, both: bool, dark: bool, actions: &mut Vec<PrepAction>) {
        let Some(report) = self.report.as_ref() else { return };
        ui.label(RichText::new(tr("By opening")).strong());
        // (white, family, variation, weight)
        let mut vars: Vec<(bool, usize, usize, u32)> = Vec::new();
        for white in self.color.colours() {
            let cr = report.color(white);
            for (f, fam) in cr.families.iter().enumerate() {
                for (v, var) in fam.variations.iter().enumerate() {
                    if var.errors.total() == 0 {
                        continue;
                    }
                    let weight = var.error_list.iter().map(|&e| judgement_weight(cr.errors[e as usize].judgement)).sum();
                    vars.push((white, f, v, weight));
                }
            }
        }
        vars.sort_by_key(|v| std::cmp::Reverse(v.3));
        if vars.is_empty() {
            ui.label(RichText::new(tr("None so far")).weak());
            return;
        }
        let mut clicked = None;
        let mut opened = None;
        let mut toggled = None;
        for (white, f, v, _) in vars {
            let cr = report.color(white);
            let var = &cr.families[f].variations[v];
            let name = var
                .opening
                .and_then(|o| report.openings.get(o))
                .map_or_else(|| tr("no name").to_owned(), |o| format!("{} · {}", o.name, o.eco));
            let open = self.open_families.contains(&(white, f * 1000 + v + 100_000));
            ui.horizontal(|ui| {
                let glyph = if open { "▾" } else { "▸" };
                if ui.add(egui::Label::new(RichText::new(glyph).weak()).sense(Sense::click())).clicked() {
                    toggled = Some((white, f * 1000 + v + 100_000));
                }
                if both {
                    ui.label(RichText::new(if white { "♙" } else { "♟" }).weak());
                }
                let selected = self.selection == Selection::Opening { white, family: f, variation: Some(v) };
                let t = RichText::new(name);
                let resp = ui.add(egui::Label::new(if selected { t.strong() } else { t }).sense(Sense::click()).truncate());
                if resp.clicked() {
                    clicked = Some(Selection::Opening { white, family: f, variation: Some(v) });
                }
                if resp.double_clicked() {
                    toggled = Some((white, f * 1000 + v + 100_000));
                }
                ui.label(RichText::new(format!("· {}", crate::i18n::games(var.games.len()))).weak());
                ui.label(RichText::new("·").weak());
                ui.spacing_mut().item_spacing.x = 4.0;
                counts_text(ui, &var.errors);
            });
            if open {
                for &ei in &var.error_list {
                    let e = &cr.errors[ei as usize];
                    let g = &report.games[e.game as usize];
                    let row = &self.rows[e.game as usize];
                    let selected = self.selection == Selection::Error { white, index: ei as usize };
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let t = RichText::new(short_label(e.ply as usize, &e.san));
                        let resp = ui.add(egui::Label::new(if selected { t.strong() } else { t }).sense(Sense::click()));
                        pill(ui, e.judgement).on_hover_text(format!(
                            "{} · {} → {}",
                            review::judgement_name(e.judgement),
                            e.before.format(),
                            e.after.format()
                        ));
                        ui.add_space(6.0);
                        ui.label(RichText::new(trf("better {best}", &[("best", &crate::san::show(&e.best_san))])).weak());
                        ui.add_space(8.0);
                        let names = format!("{} — {}", row.white, row.black);
                        let r2 = ui.add(egui::Label::new(RichText::new(names).weak()).sense(Sense::click()).truncate());
                        ui.label(RichText::new(result_text(row.result)).weak());
                        ui.label(RichText::new(crate::library::display_date(&g.date)).weak());
                        if resp.clicked() || r2.clicked() {
                            clicked = Some(Selection::Error { white, index: ei as usize });
                        }
                        if resp.double_clicked() || r2.double_clicked() {
                            opened = Some((e.game, e.ply as usize));
                        }
                    });
                }
            }
        }
        let _ = dark;
        let opened = opened.map(|(gi, ply)| (report.games[gi as usize].index as usize, ply));
        if let Some(k) = toggled
            && !self.open_families.remove(&k)
        {
            self.open_families.insert(k);
        }
        if let Some(sel) = clicked {
            self.focus_games = false;
            self.select(sel);
        }
        if let Some((index, ply)) = opened {
            actions.push(PrepAction::OpenGame {
                path: self.path.clone(),
                index,
                ply,
            });
        }
    }

    // ----- the games list ------------------------------------------------------------

    fn games_list(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        let Some(report) = self.report.as_ref() else { return };
        let list = self.games_of();
        let title = match &self.selection {
            Selection::None => trf("Games · {n}", &[("n", &list.len())]),
            Selection::Node { white, node } => {
                let n = &report.color(*white).tree.nodes[*node as usize];
                match n.ply {
                    0 => trf("Games · {n}", &[("n", &list.len())]),
                    p => trf("Games · after {move} · {n}", &[("move", &short_label(usize::from(p) - 1, &n.san)), ("n", &list.len())]),
                }
            }
            Selection::Opening { white, family, variation } => {
                let fam = &report.color(*white).families[*family];
                let name = match variation {
                    Some(v) => fam.variations[*v].opening.and_then(|o| report.openings.get(o)).map(|o| o.name.clone()).unwrap_or_else(|| fam.name.clone()),
                    None => fam.name.clone(),
                };
                trf("Games · {opening} · {n}", &[("opening", &name), ("n", &list.len())])
            }
            Selection::Repeated { white, index } => {
                let r = &report.color(*white).repeated[*index];
                trf("Games · {move} · {n}", &[("move", &format!("{} {}", short_label(r.ply as usize, &r.san), review::judgement_symbol(r.judgement))), ("n", &list.len())])
            }
            Selection::Error { white, index } => {
                let e = &report.color(*white).errors[*index];
                trf("Games · {move} · {n}", &[("move", &format!("{} {}", short_label(e.ply as usize, &e.san), review::judgement_symbol(e.judgement))), ("n", &list.len())])
            }
        };
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            if self.selection != Selection::None
                && ui.small_button("🗙").on_hover_text(tr("All games")).clicked()
            {
                clear = true;
            }
        });
        if clear {
            self.select(Selection::None);
            return;
        }
        if list.is_empty() {
            return;
        }
        let ply = self.selection_ply();
        let width = ui.available_width();
        let show_event = width > 820.0;
        let show_then = width > 600.0;
        let show_date = width > 480.0;
        let show_elo = width > 400.0;
        let show_errors = report.engine.is_some() && width > 540.0;
        let columns = 3 + 2 * usize::from(show_elo) + usize::from(show_date) + usize::from(show_then) + usize::from(show_event) + usize::from(show_errors);
        let fixed = 44.0
            + if show_elo { 88.0 } else { 0.0 }
            + if show_date { 84.0 } else { 0.0 }
            + if show_errors { 80.0 } else { 0.0 }
            + match (show_then, show_event) {
                (true, true) => 150.0,
                (true, false) => 100.0,
                _ => 0.0,
            }
            + if show_event { 100.0 } else { 0.0 }
            + ui.spacing().item_spacing.x * columns as f32;
        let player_w = ((width - fixed) / 2.0).clamp(60.0, 170.0);
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("prep_games_table")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(player_w).clip(true));
        if show_elo {
            table = table.column(Column::exact(44.0));
        }
        table = table.column(Column::exact(player_w).clip(true));
        if show_elo {
            table = table.column(Column::exact(44.0));
        }
        table = table.column(Column::exact(44.0));
        if show_date {
            table = table.column(Column::exact(84.0));
        }
        if show_then && show_event {
            table = table.column(Column::exact(150.0).clip(true));
        } else if show_then {
            table = table.column(Column::remainder().at_least(100.0).clip(true));
        }
        if show_event {
            table = table.column(Column::remainder().at_least(100.0).clip(true));
        }
        if show_errors {
            table = table.column(Column::exact(80.0).clip(true));
        }
        if self.scroll_to_selected
            && let Some(c) = self.games_cursor
        {
            table = table.scroll_to_row(c, None);
            self.scroll_to_selected = false;
        }
        let mut hovered = None;
        let mut clicked = None;
        let mut opened = None;
        let mut sort_click = false;
        let newest = self.newest_first;
        let player_names: HashSet<&str> = self.names.iter().map(String::as_str).collect();
        table
            .header(ROW_H, |mut h| {
                head_col(&mut h, tr("White"), "");
                if show_elo {
                    head_col(&mut h, "Elo", "");
                }
                head_col(&mut h, tr("Black"), "");
                if show_elo {
                    head_col(&mut h, "Elo", "");
                }
                head_col(&mut h, tr("Res."), "");
                if show_date {
                    h.col(|ui| {
                        let arrow = if newest { " ⏷" } else { " ⏶" };
                        if ui
                            .add(egui::Button::new(RichText::new(format!("{}{arrow}", tr("Date"))).strong()).frame(false))
                            .on_hover_text(tr("Newest first"))
                            .clicked()
                        {
                            sort_click = true;
                        }
                    });
                }
                if show_then {
                    h.col(|ui| {
                        ui.label(RichText::new(tr("Then")).strong())
                            .on_hover_text(tr("How the game went on from this position"));
                    });
                }
                if show_event {
                    head_col(&mut h, tr("Event"), "");
                }
                if show_errors {
                    head_col(&mut h, tr("Errors"), tr("His marked moves in the opening of this game"));
                }
            })
            .body(|body| {
                body.rows(ROW_H, list.len(), |mut row| {
                    let gi = list[row.index()];
                    let g = &report.games[gi as usize];
                    let r = &self.rows[gi as usize];
                    row.set_selected(self.games_cursor == Some(row.index()));
                    let name = |ui: &mut Ui, n: &str| {
                        let t = RichText::new(n);
                        ui.add(egui::Label::new(if player_names.contains(n) { t.strong() } else { t }).truncate());
                    };
                    let elo = |e: u16| if e > 0 { e.to_string() } else { String::new() };
                    row.col(|ui| name(ui, &r.white));
                    if show_elo {
                        row.col(|ui| {
                            ui.label(RichText::new(elo(r.white_elo)).weak());
                        });
                    }
                    row.col(|ui| name(ui, &r.black));
                    if show_elo {
                        row.col(|ui| {
                            ui.label(RichText::new(elo(r.black_elo)).weak());
                        });
                    }
                    row.col(|ui| {
                        ui.label(result_text(r.result));
                    });
                    if show_date {
                        row.col(|ui| {
                            ui.label(RichText::new(crate::library::display_date(&g.date)).weak());
                        });
                    }
                    if show_then {
                        row.col(|ui| {
                            let then: Vec<String> = g.sans.iter().skip(ply).take(3).cloned().collect();
                            let mut text = String::new();
                            for (k, san) in then.iter().enumerate() {
                                let p = ply + k;
                                if k == 0 || p.is_multiple_of(2) {
                                    text.push_str(&short_label(p, san));
                                } else {
                                    text.push_str(san);
                                }
                                text.push(' ');
                            }
                            ui.add(egui::Label::new(crate::san::show_line(text.trim_end())).truncate());
                        });
                    }
                    if show_event {
                        row.col(|ui| {
                            ui.add(egui::Label::new(RichText::new(&r.event).italics().weak()).truncate());
                        });
                    }
                    if show_errors {
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let cr = report.color(g.white);
                            for e in cr.errors.iter().filter(|e| e.game == gi) {
                                let number = e.ply as usize / 2 + 1;
                                ui.label(RichText::new(format!("{number}.")).small());
                                pill(ui, e.judgement).on_hover_text(format!(
                                    "{} {} — {}",
                                    short_label(e.ply as usize, &e.san),
                                    review::judgement_name(e.judgement),
                                    trf("better {best}", &[("best", &crate::san::show(&e.best_san))])
                                ));
                            }
                        });
                    }
                    let resp = row.response().on_hover_text(tr("Double click: open this game at this position"));
                    if resp.hovered() {
                        hovered = Some(gi);
                    }
                    if resp.double_clicked() {
                        opened = Some(gi);
                    } else if resp.clicked() {
                        clicked = Some(row.index());
                    }
                });
            });
        if sort_click {
            self.newest_first = !self.newest_first;
        }
        // The game's move from the shown position, as an arrow.
        if let Some(gi) = hovered
            && let Some(s) = &self.shown_pos
            && s.ply + 1 == s.positions.len()
        {
            let g = &report.games[gi as usize];
            if let Some(m) = g.ucis.get(ply).and_then(|u| u.parse::<UciMove>().ok()).and_then(|u| u.to_move(&s.positions[s.ply]).ok()) {
                self.hover_arrow = m.from().map(|f| (f, crate::board::ui_target(&m)));
            }
        }
        if let Some(i) = clicked {
            self.games_cursor = Some(i);
            self.focus_games = true;
        }
        if let Some(gi) = opened {
            actions.push(PrepAction::OpenGame {
                path: self.path.clone(),
                index: report.games[gi as usize].index as usize,
                ply,
            });
        }
    }

    // ----- the keyboard --------------------------------------------------------------

    fn keyboard(&mut self, ctx: &egui::Context, settings: &Settings, actions: &mut Vec<PrepAction>) {
        let ctrl = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let alt = |k| KeyboardShortcut::new(Modifiers::ALT, k);
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::Num1))) {
            self.tab = Tab::Repertoire;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::Num2))) {
            self.tab = Tab::Openings;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::Num3))) {
            self.tab = Tab::Errors;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&alt(Key::Num1))) {
            self.color = ColorChoice::White;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&alt(Key::Num2))) {
            self.color = ColorChoice::Black;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&alt(Key::Num3))) {
            self.color = ColorChoice::Both;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::W))) {
            actions.push(PrepAction::Back);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F)) {
            let base = self.shown_pos.as_ref().map_or(self.color != ColorChoice::Black, |s| s.white);
            self.flipped = Some(!self.board_white() == base);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::A)) {
            if self.job.is_some() {
                self.stop_engine(actions);
            } else if self.report.is_some() {
                self.engine_asked(settings, ctx, actions);
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::F)))
            && let Some(s) = &self.shown_pos
        {
            let mut p = Chess::default();
            let moves: Vec<Move> = s.ucis[..s.ply]
                .iter()
                .filter_map(|u| {
                    let m = u.parse::<UciMove>().ok()?.to_move(&p).ok()?;
                    p.play_unchecked(m);
                    Some(m)
                })
                .collect();
            actions.push(PrepAction::FindPosition(self.path.clone(), Chess::default(), moves, !self.board_white()));
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.board_popup {
                self.board_popup = false;
            } else if self.selection != Selection::None {
                self.select(Selection::None);
            } else {
                actions.push(PrepAction::Back);
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
            self.focus_games = !self.focus_games;
            if self.focus_games && self.games_cursor.is_none() {
                self.games_cursor = Some(0);
            }
        }
        let (up, down, left, right, enter, space, home, end) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Space),
                i.consume_key(Modifiers::NONE, Key::Home),
                i.consume_key(Modifiers::NONE, Key::End),
            )
        });
        if !(up || down || left || right || enter || space || home || end) {
            return;
        }
        if self.focus_games {
            let list = self.games_of();
            if list.is_empty() {
                return;
            }
            let cur = self.games_cursor.unwrap_or(0);
            let next = if up {
                cur.saturating_sub(1)
            } else if down {
                (cur + 1).min(list.len() - 1)
            } else if home {
                0
            } else if end {
                list.len() - 1
            } else {
                cur
            };
            if next != cur || self.games_cursor.is_none() {
                self.games_cursor = Some(next);
                self.scroll_to_selected = true;
            }
            if enter
                && let Some(r) = &self.report
            {
                actions.push(PrepAction::OpenGame {
                    path: self.path.clone(),
                    index: r.games[list[next] as usize].index as usize,
                    ply: self.selection_ply(),
                });
            }
            return;
        }
        match self.tab {
            Tab::Repertoire => self.tree_keys(up, down, left, right, enter, space, home, end),
            Tab::Openings => self.openings_keys(up, down, left, right, enter, space, home, end),
            Tab::Errors => {
                if enter && let Some(r) = &self.report {
                    let open = match &self.selection {
                        Selection::Repeated { white, index } => {
                            let rep = &r.color(*white).repeated[*index];
                            Some((rep.games[0], rep.ply as usize))
                        }
                        Selection::Error { white, index } => {
                            let e = &r.color(*white).errors[*index];
                            Some((e.game, e.ply as usize))
                        }
                        _ => None,
                    };
                    if let Some((gi, ply)) = open {
                        actions.push(PrepAction::OpenGame {
                            path: self.path.clone(),
                            index: r.games[gi as usize].index as usize,
                            ply,
                        });
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tree_keys(&mut self, up: bool, down: bool, left: bool, right: bool, enter: bool, space: bool, home: bool, end: bool) {
        let rows = self.tree_rows();
        if rows.is_empty() {
            return;
        }
        let at = rows.iter().position(|r| match (r, &self.selection) {
            (TreeRow::Node { white, node, .. }, Selection::Node { white: sw, node: sn }) => white == sw && node == sn,
            _ => false,
        });
        let go = |screen: &mut Self, i: usize| {
            if let Some(TreeRow::Node { white, node, .. }) = rows.get(i).copied() {
                screen.select(Selection::Node { white, node });
                screen.scroll_to_selected = true;
            }
        };
        let Some(at) = at else {
            if up || down || home || end || right {
                go(self, 0);
            }
            return;
        };
        let Some(TreeRow::Node { white, node, .. }) = rows.get(at).copied() else {
            return;
        };
        let has_children = self
            .report
            .as_ref()
            .is_some_and(|r| !r.color(white).tree.nodes[node as usize].children.is_empty());
        let open = self.expanded.contains(&(white, node));
        if down {
            let mut next = at + 1;
            while next < rows.len() && matches!(rows[next], TreeRow::Others { .. }) {
                next += 1;
            }
            if next < rows.len() {
                go(self, next);
            }
        } else if up {
            let mut prev = at;
            while prev > 0 {
                prev -= 1;
                if matches!(rows[prev], TreeRow::Node { .. }) {
                    go(self, prev);
                    break;
                }
            }
        } else if home {
            go(self, 0);
        } else if end {
            let last = rows.iter().rposition(|r| matches!(r, TreeRow::Node { .. })).unwrap_or(0);
            go(self, last);
        } else if right {
            if has_children && !open {
                self.expanded.insert((white, node));
            } else if has_children {
                go(self, at + 1);
            }
        } else if left {
            if open {
                self.expanded.remove(&(white, node));
            } else if let Some(p) = self.report.as_ref().and_then(|r| r.color(white).tree.nodes[node as usize].parent) {
                self.select(Selection::Node { white, node: p });
                self.scroll_to_selected = true;
            }
        } else if space {
            if has_children && !self.expanded.remove(&(white, node)) {
                self.expanded.insert((white, node));
            }
        } else if enter {
            self.focus_games = true;
            self.games_cursor = Some(0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn openings_keys(&mut self, up: bool, down: bool, left: bool, right: bool, enter: bool, space: bool, home: bool, end: bool) {
        let Some(report) = &self.report else { return };
        let mut rows: Vec<(bool, usize, Option<usize>)> = Vec::new();
        for white in self.color.colours() {
            let cr = report.color(white);
            for (f, fam) in cr.families.iter().enumerate() {
                rows.push((white, f, None));
                if self.open_families.contains(&(white, f)) {
                    for v in 0..fam.variations.len() {
                        rows.push((white, f, Some(v)));
                    }
                }
            }
        }
        if rows.is_empty() {
            return;
        }
        let at = rows.iter().position(|&(w, f, v)| self.selection == Selection::Opening { white: w, family: f, variation: v });
        let go = |screen: &mut Self, i: usize| {
            if let Some(&(white, family, variation)) = rows.get(i) {
                screen.select(Selection::Opening { white, family, variation });
                screen.scroll_to_selected = true;
            }
        };
        let Some(at) = at else {
            if up || down || home || end || right {
                go(self, 0);
            }
            return;
        };
        let (white, f, v) = rows[at];
        if down {
            go(self, (at + 1).min(rows.len() - 1));
        } else if up {
            go(self, at.saturating_sub(1));
        } else if home {
            go(self, 0);
        } else if end {
            go(self, rows.len() - 1);
        } else if right || space {
            if v.is_none() && !self.open_families.remove(&(white, f)) {
                self.open_families.insert((white, f));
            }
        } else if left {
            if v.is_some() {
                go(self, rows.iter().position(|&(w, ff, vv)| w == white && ff == f && vv.is_none()).unwrap_or(at));
            } else {
                self.open_families.remove(&(white, f));
            }
        } else if enter {
            self.focus_games = true;
            self.games_cursor = Some(0);
        }
    }
}

/// The arrows of an error: the move played in the colour of its sign, the better move
/// in green.
fn error_arrows(pos: &Chess, played: &str, best: Option<&str>, j: Judgement) -> Vec<Arrow> {
    let mut out = Vec::new();
    let to_arrow = |u: &str, color: Color32| -> Option<Arrow> {
        let m = u.parse::<UciMove>().ok()?.to_move(pos).ok()?;
        Some(Arrow {
            from: m.from()?,
            to: crate::board::ui_target(&m),
            color,
            weight: 1.0,
        })
    };
    if let Some(a) = to_arrow(played, review::judgement_color(j).gamma_multiply(0.85)) {
        out.push(a);
    }
    if let Some(a) = best.and_then(|b| to_arrow(b, crate::board::shape_preview_color(chess_core::ShapeColor::Green))) {
        out.push(a);
    }
    out
}

/// «6…Nb6 ? — better 6…e6 · 0.0 → −1.2».
fn error_note(e: &prep::OpeningError) -> String {
    let played = format!("{} {}", short_label(e.ply as usize, &e.san), review::judgement_symbol(e.judgement));
    trf(
        "{played} — better {best} · {before} → {after}",
        &[
            ("played", &played),
            ("best", &short_label(e.ply as usize, &e.best_san)),
            ("before", &e.before.format()),
            ("after", &e.after.format()),
        ],
    )
}

/// The table's line to the family's own position: the shallowest position of the
/// family's games whose name is of the family ("1. e4 c5" for the Sicilian, whatever
/// the variations); the shortest variation's line when there is none.
fn family_line(report: &PrepReport, fam: &prep::Family) -> Option<String> {
    let mut best: Option<&'static prep::Opening> = None;
    for v in &fam.variations {
        for &gi in &v.games {
            let g = &report.games[gi as usize];
            for &k in &g.keys {
                let Some(o) = prep::openings::by_position(k).map(prep::openings::get) else {
                    continue;
                };
                if o.family() == fam.name && best.is_none_or(|b| o.plies < b.plies) {
                    best = Some(o);
                }
            }
        }
    }
    best.map(|o| o.line.clone()).or_else(|| {
        fam.variations
            .iter()
            .filter_map(|v| v.opening.and_then(|o| report.openings.get(o)))
            .min_by_key(|o| o.plies)
            .map(|o| o.line.clone())
    })
}

/// The ply of the first position of a game's line named with `family`.
fn family_ply(g: &GameInfo, family: &str) -> Option<usize> {
    (0..g.keys.len()).find(|&i| {
        prep::openings::by_position(g.keys[i])
            .map(prep::openings::get)
            .is_some_and(|o| o.family() == family)
    })
}

/// A header cell: the title in bold, a tooltip when there is one.
fn head_col(h: &mut egui_extras::TableRow<'_, '_>, text: &str, hint: &str) {
    h.col(|ui| {
        let l = ui.label(RichText::new(text).strong());
        if !hint.is_empty() {
            l.on_hover_text(hint);
        }
    });
}

fn result_text(r: &str) -> &str {
    if r == "1/2-1/2" { "½-½" } else { r }
}

fn text_width(ui: &Ui, text: &str, style: egui::TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
        .size()
        .x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: u32, name: &str) -> Candidate {
        Candidate {
            id,
            name: name.to_owned(),
            games: 1,
        }
    }

    #[test]
    fn candidates_split_into_whole_words_and_similar_names() {
        let list = vec![
            c(1, "Вафин Радель"),
            c(2, "Vafin, Radel"),
            c(3, "Вафина Раяна"),
            c(4, "Вафин Р."),
        ];
        let (exact, similar) = split_candidates("Вафин", list.clone());
        assert_eq!(exact.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 4]);
        assert_eq!(similar.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2, 3]);
        // An initial matches the beginning of a word; the full stop is ignored.
        let (exact, _) = split_candidates("Вафин Р.", list.clone());
        assert_eq!(exact.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 4]);
        let (exact, similar) = split_candidates("вафин радель", list.clone());
        assert_eq!(exact.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(similar.len(), 3);
        // Commas and case do not matter.
        let (exact, _) = split_candidates("radel vafin", list);
        assert_eq!(exact.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn labels_and_lines() {
        assert_eq!(short_label(0, "e4"), "1.e4");
        assert_eq!(short_label(1, "c5"), "1…c5");
        assert_eq!(short_label(4, "Bb5"), "3.Bb5");
        let sans: Vec<String> = ["e4", "c5", "Nf3"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(line_text(&sans), "1.e4 c5 2.Nf3");
        assert_eq!(shorten_middle("1. e4 c5 2. Nf3", 100), "1. e4 c5 2. Nf3");
        assert_eq!(shorten_middle("abcdefghij", 7), "abc…hij");
        let ucis: Vec<String> = ["e2e4", "c7c5", "g1f3"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(replay(&ucis).len(), 4);
        assert_eq!(replay(&["zz".to_owned()]).len(), 1);
    }

    #[test]
    fn the_period_of_the_games() {
        use prep::Outcome;
        let game = |date: &str| GameInfo {
            index: 0,
            white: true,
            opponent: String::new(),
            opponent_elo: 0,
            date: date.to_owned(),
            outcome: Outcome::Unknown,
            ucis: Vec::new(),
            sans: Vec::new(),
            keys: Vec::new(),
            opening: None,
            named: None,
            book: None,
        };
        assert_eq!(period(&[game("2026.10.18"), game("2026.01.03"), game("")]), "01.2026 – 10.2026");
        assert_eq!(period(&[game("2026.10.18")]), "10.2026");
        assert_eq!(period(&[game("")]), "");
    }

    #[test]
    fn book_choice_from_the_setting() {
        let own = Path::new("/a/own.pgn");
        let dbs = vec![
            RefDb {
                path: PathBuf::from("/a/own.pgn"),
                name: "own".into(),
                db: None,
                failed: None,
                bytes: 500,
            },
            RefDb {
                path: PathBuf::from("/a/mega.cbh"),
                name: "mega".into(),
                db: None,
                failed: None,
                bytes: 5000,
            },
            RefDb {
                path: PathBuf::from("/a/small.pgn"),
                name: "small".into(),
                db: None,
                failed: None,
                bytes: 10,
            },
        ];
        assert_eq!(BookChoice::from_setting(None, own, &dbs), BookChoice::Database(PathBuf::from("/a/mega.cbh")));
        assert_eq!(BookChoice::from_setting(Some(Path::new("")), own, &dbs), BookChoice::None);
        assert_eq!(BookChoice::from_setting(Some(own), own, &dbs), BookChoice::Same);
        assert_eq!(
            BookChoice::from_setting(Some(Path::new("/a/small.pgn")), own, &dbs),
            BookChoice::Database(PathBuf::from("/a/small.pgn"))
        );
        // A database no longer on the shelf: the default again.
        assert_eq!(BookChoice::from_setting(Some(Path::new("/gone.pgn")), own, &dbs), BookChoice::Database(PathBuf::from("/a/mega.cbh")));
        assert_eq!(BookChoice::Same.setting(own), own);
        assert_eq!(BookChoice::None.setting(own), PathBuf::new());
    }
}
