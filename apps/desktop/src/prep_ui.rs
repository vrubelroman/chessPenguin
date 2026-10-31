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
use game_review::{JobEvent, JobSettings, Judgement};
use prep::{
    BookExit, BookOptions, Candidate, ErrorCounts, GameInfo, Options, Period, Player,
    PositionCache, PrepReport, Tally,
};
use serde::{Deserialize, Serialize};
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Move, Position, Square};

use crate::board::{Arrow, BoardState, BoardView};
use crate::board_search::{Background, PREVIEW_ARROW};
use crate::i18n::{tr, trf};
use crate::large_db::grouped;
use crate::reference::RefDb;
use crate::review;
use crate::settings::Settings;

/// «Prepare against…» as the library asks it: the name typed or picked and the colour
/// of the filter (§2.1).
#[derive(Clone, Debug, Default)]
pub struct PrepEntry {
    pub query: String,
    /// The name id when it was picked from the list.
    pub exact: Option<u32>,
    pub side: Side,
}

/// A table's order: the column and the direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortBy<K> {
    pub key: K,
    pub ascending: bool,
}

impl<K: Copy + PartialEq> SortBy<K> {
    /// A click on the header of `key`: the same column turns the order round, another
    /// one starts in its natural direction (names A–Z, numbers and dates high first).
    fn click(&mut self, key: K, ascending_first: bool) {
        if self.key == key {
            self.ascending = !self.ascending;
        } else {
            *self = SortBy {
                key,
                ascending: ascending_first,
            };
        }
    }

    /// `ord` (ascending) in this table's direction.
    fn apply(&self, ord: std::cmp::Ordering) -> std::cmp::Ordering {
        if self.ascending { ord } else { ord.reverse() }
    }
}

/// The columns the repertoire tree orders the moves of a position by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeSort {
    Games,
    Share,
    Score,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpeningSort {
    Name,
    Eco,
    Games,
    Share,
    Score,
    Last,
    Errors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatedSort {
    Move,
    Times,
    Score,
    Opening,
}

/// «By opening» of the errors tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariationSort {
    Games,
    Errors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameSort {
    White,
    WhiteElo,
    Black,
    BlackElo,
    Result,
    Date,
    Then,
    Event,
    Errors,
}

/// The order of every table of the screen (kept in the settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrepSorts {
    pub tree: SortBy<TreeSort>,
    pub openings: SortBy<OpeningSort>,
    pub repeated: SortBy<RepeatedSort>,
    pub variations: SortBy<VariationSort>,
    pub games: SortBy<GameSort>,
}

impl Default for PrepSorts {
    fn default() -> Self {
        // Most played, most repeated, worst, newest first.
        PrepSorts {
            tree: SortBy {
                key: TreeSort::Games,
                ascending: false,
            },
            openings: SortBy {
                key: OpeningSort::Games,
                ascending: false,
            },
            repeated: SortBy {
                key: RepeatedSort::Times,
                ascending: false,
            },
            variations: SortBy {
                key: VariationSort::Errors,
                ascending: false,
            },
            games: SortBy {
                key: GameSort::Date,
                ascending: false,
            },
        }
    }
}

/// A sort column of the errors tab (its two parts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortKeyOf {
    Repeated(RepeatedSort),
    Variation(VariationSort),
}

/// A value a table is ordered by.
#[derive(Clone, Debug, PartialEq, PartialOrd)]
enum SortVal {
    Num(f64),
    Text(String),
}

fn num(x: impl Into<f64>) -> SortVal {
    SortVal::Num(x.into())
}

/// `a` against `b` (a value never compared to another kind is equal).
fn cmp_val(a: &SortVal, b: &SortVal) -> std::cmp::Ordering {
    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
}

/// A score for ordering: games without a result last when high first.
fn score_val(t: &Tally) -> SortVal {
    num(t.score().unwrap_or(-1.0))
}

/// A sortable column's header: the title with ▲/▼ when the table is ordered by it;
/// a click is noted in `clicked`.
fn sort_col<K: Copy + PartialEq>(
    h: &mut egui_extras::TableRow<'_, '_>,
    text: &str,
    hint: &str,
    key: K,
    sort: SortBy<K>,
    clicked: &mut Option<K>,
) {
    h.col(|ui| {
        let r = crate::widgets::sort_header(ui, text, (sort.key == key).then_some(sort.ascending));
        let r = if hint.is_empty() {
            r
        } else {
            r.on_hover_text(hint)
        };
        if r.clicked() {
            *clicked = Some(key);
        }
    });
}

/// The colour switch of the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorChoice {
    White,
    Black,
    Both,
}

impl ColorChoice {
    pub fn side(self) -> Side {
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
    pub fn from_setting(setting: Option<&Path>, own: &Path, dbs: &[RefDb]) -> BookChoice {
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

    pub fn setting(&self, own: &Path) -> PathBuf {
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
    dbs: &[RefDb],
    width: f32,
    with_caption: bool,
) -> Option<BookChoice> {
    let mut picked = None;
    let own_name = base_name(own);
    let own_name = own_name.as_str();
    let label = current.label(own_name, dbs);
    // Without its caption beside it, «None» alone would say nothing.
    let shown = if with_caption {
        trf("Book: {name}", &[("name", &label)])
    } else {
        label
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
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

// ----- the period ------------------------------------------------------------------

/// The periods offered, in the menu's order.
const PERIODS: [Period; 8] = [
    Period::All,
    Period::Months(3),
    Period::Months(6),
    Period::Months(12),
    Period::Months(24),
    Period::Last(50),
    Period::Last(100),
    Period::Last(200),
];

/// The period in a narrow row: «3 mo.», «Last 100».
fn period_short(p: Period) -> String {
    match p {
        Period::All => tr("All").to_owned(),
        Period::Months(12) => tr("1 yr").to_owned(),
        Period::Months(24) => tr("2 yrs").to_owned(),
        Period::Months(m) => trf("{n} mo.", &[("n", &m)]),
        Period::Last(n) => trf("Last {n}", &[("n", &n)]),
    }
}

/// `yyyymmdd` as «18.10.2026», «10.2026», «2026».
fn shown_date(d: u32) -> String {
    match (d / 10000, d / 100 % 100, d % 100) {
        (0, ..) => String::new(),
        (y, 0, _) => y.to_string(),
        (y, m, 0) => format!("{m:02}.{y}"),
        (y, m, day) => format!("{day:02}.{m:02}.{y}"),
    }
}

fn period_label(p: Period) -> String {
    match p {
        Period::All => tr("All games").to_owned(),
        Period::Months(12) => tr("Last year").to_owned(),
        Period::Months(24) => tr("Last 2 years").to_owned(),
        Period::Months(3) => tr("Last 3 months").to_owned(),
        Period::Months(6) => tr("Last 6 months").to_owned(),
        Period::Months(m) => trf("Last {n} months", &[("n", &m)]),
        Period::Last(n) => trf("Last {n} games", &[("n", &n)]),
    }
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
    /// The book; `None` until the shelf is known (the setting names a database of it).
    book: Option<BookChoice>,
    /// Games of the ticked names by colour, and the job counting them.
    counts: Option<(Vec<u32>, (u32, u32))>,
    count_job: Option<(Vec<u32>, CountJob)>,
    focus_query: bool,
    typed_at: f64,
    /// `CP_PREP_OPEN`: «Open report» pressed by itself once a name is ticked.
    auto_open: bool,
}

impl PrepDialog {
    pub fn new(path: PathBuf, entry: PrepEntry) -> Self {
        PrepDialog {
            book: None,
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
        self.search = Background::spawn(ctx, now, move |_, _| {
            Some(prep::candidates(&db, &q, CANDIDATES))
        })
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
        if self.book.is_none() && !self.dbs.is_empty() {
            // The book chosen last time, else the largest database on the shelf: once
            // the shelf is known.
            self.book = Some(BookChoice::from_setting(
                settings.prep.book.as_deref(),
                &self.path,
                &self.dbs,
            ));
        }
        let book = self.book.clone().unwrap_or(BookChoice::None);
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
                            self.book = Some(BookChoice::from_setting(
                                settings.prep.book.as_deref(),
                                &self.path,
                                &self.dbs,
                            ));
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
                        ui.radio_value(&mut self.side, ColorChoice::Both, tr("Both"));
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([label_w, 20.0], egui::Label::new(tr("Book")));
                        let width = ui.available_width() - 8.0;
                        if let Some(b) = book_picker(ui, "prep_dialog_book", &book, &self.path, &self.dbs, width, false) {
                            settings.prep.book = Some(b.setting(&self.path));
                            self.book = Some(b);
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
        if outcome.is_none()
            && ready
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
        {
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
            book: self.book.clone().unwrap_or(BookChoice::None),
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
                ui.label(
                    RichText::new(tr("Type a name: the players of the database are offered"))
                        .weak(),
                );
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

// ----- the saved preparations -----------------------------------------------------

/// What the «Preparations» window asks of the application.
pub enum ListOutcome {
    /// The saved preparation of this id.
    Open(String),
    /// «New preparation…»: the dialog for the chosen database.
    New,
    Close,
}

/// The library's «Preparations»: the saved ones, to open, rename or delete.
pub struct PrepList {
    items: Vec<(String, crate::prep_saved::SavedPrep)>,
    /// The entry being renamed and the name typed.
    renaming: Option<(String, String)>,
    /// The entry asked about before deleting.
    deleting: Option<String>,
    error: Option<String>,
}

impl PrepList {
    pub fn new() -> Self {
        let mut list = PrepList {
            items: Vec::new(),
            renaming: None,
            deleting: None,
            error: None,
        };
        list.reload();
        list
    }

    fn reload(&mut self) {
        self.items = crate::prep_saved::dir()
            .as_deref()
            .map(crate::prep_saved::list)
            .unwrap_or_default();
    }

    /// `can_new`: a database is chosen to prepare in.
    pub fn show(&mut self, ctx: &egui::Context, can_new: bool) -> Option<ListOutcome> {
        let mut out = None;
        // Escape while renaming ends the renaming only.
        let was_renaming = self.renaming.is_some();
        let modal = egui::Modal::new(egui::Id::new("prep_list")).show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.set_width((screen.width() - 40.0).min(560.0));
            ui.heading(tr("Preparations"));
            ui.add_space(6.0);
            if self.items.is_empty() {
                ui.label(
                    RichText::new(tr(
                        "Nothing saved yet. Prepare against a player and press «Save» on the report.",
                    ))
                    .weak(),
                );
            }
            egui::ScrollArea::vertical()
                .max_height((screen.height() - 180.0).max(120.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (id, item) in self.items.clone() {
                        self.row(ui, &id, &item, &mut out);
                        ui.separator();
                    }
                });
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(ui.visuals().warn_fg_color));
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Close")).clicked() {
                    out = Some(ListOutcome::Close);
                }
                if ui
                    .add_enabled(can_new, egui::Button::new(tr("♟ New preparation…")))
                    .on_disabled_hover_text(tr("Add a database to the shelf first"))
                    .on_hover_text(tr("Choose a player in the current database"))
                    .clicked()
                {
                    out = Some(ListOutcome::New);
                }
            });
        });
        if modal.should_close() && !was_renaming {
            out = out.or(Some(ListOutcome::Close));
        }
        out
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        id: &str,
        item: &crate::prep_saved::SavedPrep,
        out: &mut Option<ListOutcome>,
    ) {
        let found = item.path.exists();
        if self.renaming.as_ref().is_some_and(|(r, _)| r == id) {
            self.rename_row(ui, id);
            return;
        }
        let deleting = self.deleting.as_deref() == Some(id);
        // The buttons' room first: the texts are cut to what is left.
        let gap = ui.spacing().item_spacing.x;
        let pad = 2.0 * ui.spacing().button_padding.x;
        let bw = |t: &str| text_width(ui, t, egui::TextStyle::Button) + pad + gap;
        let buttons = if deleting {
            text_width(ui, tr("Delete?"), egui::TextStyle::Body)
                + gap
                + bw(tr("Delete"))
                + bw(tr("No"))
        } else {
            bw(tr("Open")) + bw("✏") + bw("🗑")
        };
        let text_w = (ui.available_width() - buttons - 2.0 * gap).max(80.0);
        ui.horizontal(|ui| {
            let mut open = false;
            ui.allocate_ui(vec2(text_w, 0.0), |ui| {
                ui.set_max_width(text_w);
                ui.vertical(|ui| {
                    let title = ui
                        .add(
                            egui::Label::new(RichText::new(&item.title).strong())
                                .truncate()
                                .sense(Sense::click()),
                        )
                        .on_hover_text(tr("Double-click to open"));
                    open |= found && title.double_clicked();
                    let mut about = vec![base_name(&item.path)];
                    // «за 2 года · 8 партий», «последние 100»: the number once.
                    match item.period {
                        Period::All => about.push(crate::i18n::games(item.games as usize)),
                        Period::Last(n) => about.push(trf("last {n}", &[("n", &n)])),
                        p => {
                            let label = period_label(p);
                            let mut chars = label.chars();
                            let lower = chars
                                .next()
                                .map(|c| c.to_lowercase().chain(chars).collect())
                                .unwrap_or_default();
                            about.push(lower);
                            about.push(crate::i18n::games(item.games as usize));
                        }
                    }
                    about.push(
                        match item.color {
                            ColorChoice::White => tr("as White"),
                            ColorChoice::Black => tr("as Black"),
                            ColorChoice::Both => tr("both colours"),
                        }
                        .to_owned(),
                    );
                    ui.add(egui::Label::new(RichText::new(about.join(" · ")).weak()).truncate());
                    let when = chrono::DateTime::<chrono::Local>::from(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(item.saved),
                    )
                    .format("%d.%m.%Y")
                    .to_string();
                    let errors = if item.evaluated {
                        let e = &item.errors;
                        trf(
                            "errors: ?? {b} · ? {m} · ?! {i}",
                            &[
                                ("b", &e.blunders),
                                ("m", &e.mistakes),
                                ("i", &e.inaccuracies),
                            ],
                        )
                    } else {
                        tr("errors not searched").to_owned()
                    };
                    let path = item.path.display().to_string();
                    if found {
                        let line =
                            format!("{errors} · {}", trf("saved {date}", &[("date", &when)]));
                        ui.add(egui::Label::new(RichText::new(line).small().weak()).truncate());
                    } else {
                        ui.add(
                            egui::Label::new(
                                RichText::new(trf(
                                    "database not found: {path}",
                                    &[("path", &path)],
                                ))
                                .small()
                                .color(ui.visuals().warn_fg_color),
                            )
                            .truncate(),
                        )
                        .on_hover_text(path);
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if deleting {
                    if ui.button(tr("No")).clicked() {
                        self.deleting = None;
                    }
                    if ui.button(tr("Delete")).clicked() {
                        self.deleting = None;
                        if let Some(dir) = crate::prep_saved::dir()
                            && let Err(e) = crate::prep_saved::delete(&dir, id)
                        {
                            self.error = Some(e.to_string());
                        }
                        self.reload();
                    }
                    ui.label(tr("Delete?"));
                    return;
                }
                if ui.button("🗑").on_hover_text(tr("Delete")).clicked() {
                    self.deleting = Some(id.to_owned());
                }
                if ui.button("✏").on_hover_text(tr("Rename")).clicked() {
                    self.renaming = Some((id.to_owned(), item.title.clone()));
                }
                open |= ui
                    .add_enabled(found, egui::Button::new(tr("Open")))
                    .on_disabled_hover_text(tr(
                        "The database is not found: put the file back or delete the preparation",
                    ))
                    .clicked();
            });
            if open {
                *out = Some(ListOutcome::Open(id.to_owned()));
            }
        });
    }

    /// The name being typed: Enter or «OK» keeps it, Escape or «Cancel» does not.
    fn rename_row(&mut self, ui: &mut Ui, id: &str) {
        let mut done = None;
        ui.horizontal(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let pad = 2.0 * ui.spacing().button_padding.x;
            let buttons = text_width(ui, tr("OK"), egui::TextStyle::Button)
                + text_width(ui, tr("Cancel"), egui::TextStyle::Button)
                + 2.0 * (pad + gap);
            if let Some((_, text)) = self.renaming.as_mut() {
                let r = ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width((ui.available_width() - buttons - gap).max(80.0)),
                );
                r.request_focus();
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    done = Some(true);
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    done = Some(false);
                }
            }
            if ui.button(tr("OK")).clicked() {
                done = Some(true);
            }
            if ui.button(tr("Cancel")).clicked() {
                done = Some(false);
            }
        });
        if let Some(ok) = done {
            let (_, text) = self.renaming.take().unwrap_or_default();
            let text = text.trim();
            if ok
                && !text.is_empty()
                && let Some(dir) = crate::prep_saved::dir()
            {
                if let Err(e) = crate::prep_saved::rename(&dir, id, text) {
                    self.error = Some(e.to_string());
                }
                self.reload();
            }
        }
    }
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

use crate::review::JobMsg;

struct EngineJob {
    runner: crate::review::JobRunner,
    rx: Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    total: usize,
    done: usize,
    /// Engine processes of the run.
    processes: usize,
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
    Node {
        white: bool,
        node: u32,
        depth: usize,
    },
    /// «other moves (n)» under a node.
    Others {
        white: bool,
        parent: u32,
        depth: usize,
        count: usize,
    },
}

/// A row of the errors tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrRow {
    /// «Repeated errors — …» / «By opening».
    RepeatedTitle,
    OpeningsTitle,
    /// A repeated error (colour, index into `repeated`) and one of its games.
    Repeated {
        white: bool,
        index: usize,
    },
    RepeatedGame {
        white: bool,
        index: usize,
        game: u32,
    },
    /// A variation with errors (colour, family, variation) and one of its errors.
    Variation {
        white: bool,
        family: usize,
        variation: usize,
    },
    Error {
        white: bool,
        index: usize,
    },
}

impl ErrRow {
    fn is_title(self) -> bool {
        matches!(self, ErrRow::RepeatedTitle | ErrRow::OpeningsTitle)
    }
}

/// The engine part of the controls.
enum EngineTexts {
    Running,
    /// A button to start or go on, and a note beside it.
    Button {
        label: String,
        note: Option<String>,
    },
    /// Done: a quiet label.
    Done {
        label: String,
        note: Option<String>,
    },
}

/// Where the engine run stands for the colour(s) shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineState {
    Running,
    /// No evaluation yet.
    NotRun,
    /// Some positions evaluated (a run stopped, a new depth, new games).
    Partial {
        done: usize,
        total: usize,
    },
    Complete {
        total: usize,
    },
}

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
    /// This screen's own number (`engine_offer::next_serial`).
    pub serial: u64,
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
    period: Period,
    book: BookChoice,
    /// Kept in «Preparations» (`prep_saved`): every change is written there too.
    saved: bool,
    /// A period asked for where the settings are not at hand (applied in `show`).
    want_period: Option<Period>,
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
    open_variations: HashSet<(bool, usize, usize)>,
    /// The row of the errors tab the keyboard is on.
    err_cursor: Option<ErrRow>,
    /// The fingerprint of the chosen book, once it is open (`None`: no book), and the
    /// book and its database it was taken from.
    book_fp: Option<u64>,
    book_fp_of: Option<(BookChoice, usize)>,
    /// Inaccuracies shown (`settings.prep.inaccuracies`, read every frame).
    show_inaccuracies: bool,
    /// The board stands over the lists (a narrow or low window); the window is low.
    stacked: bool,
    low_window: bool,
    selection: Selection,
    shown_pos: Option<Shown>,
    /// The user turned the board (else it is the player's colour).
    flipped: Option<bool>,
    board_state: BoardState,
    /// In a low window the board is a panel over the lists, opened by a button.
    board_popup: bool,
    /// The order of each table (the settings' `prep.sorts`, written back on change).
    sorts: PrepSorts,
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

/// The table's line ("1. e4 c5 2. Nf3") as the program writes lines: moves with
/// their numbers joined ("1.e4", "c5", "2.Nf3").
fn table_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut number: Option<&str> = None;
    for tok in line.split_whitespace() {
        if tok.ends_with('.')
            && tok
                .trim_end_matches('.')
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            number = Some(tok);
        } else {
            out.push(match number.take() {
                Some(n) if n.ends_with("...") => format!("{}…{tok}", n.trim_end_matches('.')),
                Some(n) => format!("{n}{tok}"),
                None => tok.to_owned(),
            });
        }
    }
    out
}

/// The moves in `width`: whole moves, the first and the last kept, «…» for the middle
/// ones that do not fit (they matter less).
fn fit_line(ui: &Ui, tokens: &[String], width: f32) -> String {
    let fits = |t: &str| text_width(ui, t, egui::TextStyle::Body) <= width;
    let full = crate::san::show_line(&tokens.join(" "));
    if fits(&full) || tokens.len() < 3 {
        return full;
    }
    // Drop moves from the middle until the rest fits.
    let mut head = tokens.len().div_ceil(2);
    let mut tail = tokens.len() - head;
    loop {
        if head > 1 && head >= tail {
            head -= 1;
        } else if tail > 1 {
            tail -= 1;
        } else {
            return crate::san::show_line(&format!("{} … {}", tokens[0], tokens[tokens.len() - 1]));
        }
        let text = format!(
            "{} … {}",
            tokens[..head].join(" "),
            tokens[tokens.len() - tail..].join(" ")
        );
        let text = crate::san::show_line(&text);
        if fits(&text) {
            return text;
        }
    }
}

/// "01.2026 – 10.2026" from the games' dates (empty without dates).
fn period(games: &[GameInfo]) -> String {
    let dated: Vec<&GameInfo> = games.iter().filter(|g| g.date_number() > 0).collect();
    let first = dated.iter().min_by_key(|g| g.date_number());
    let last = dated.iter().max_by_key(|g| g.date_number());
    let month = |g: &GameInfo| -> String {
        let parts: Vec<&str> = g.date.split('.').collect();
        match parts.as_slice() {
            [y, m, ..] if m.chars().all(|c| c.is_ascii_digit()) && !m.is_empty() => {
                format!("{m}.{y}")
            }
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

/// The results as a bar, by lightness (White's wins light, draws grey, Black's wins
/// dark, as on the search board) with no numbers in it: the score beside it is the
/// player's, the percentages are in its tooltip. Faint over few games.
fn score_bar(ui: &mut Ui, t: &Tally, white: bool, few: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(56.0, 12.0), Sense::hover());
    let (white_wins, black_wins) = if white {
        (t.wins, t.losses)
    } else {
        (t.losses, t.wins)
    };
    let decided = white_wins + t.draws + black_wins;
    let fade = |c: Color32| if few { c.gamma_multiply(0.45) } else { c };
    let painter = ui.painter();
    let outline = if ui.visuals().dark_mode {
        Color32::from_gray(0x70)
    } else {
        ui.visuals().weak_text_color()
    };
    if decided > 0 {
        let mut x = rect.left();
        for (n, fill) in [
            (white_wins, Color32::from_gray(0xe8)),
            (t.draws, Color32::from_gray(0x8c)),
            (black_wins, Color32::from_gray(0x2b)),
        ] {
            let w = rect.width() * n as f32 / decided as f32;
            if w > 0.0 {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x, rect.top()),
                        egui::pos2(x + w, rect.bottom()),
                    ),
                    0.0,
                    fade(fill),
                );
            }
            x += w;
        }
    }
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, fade(outline)),
        egui::StrokeKind::Inside,
    );
    if decided > 0 {
        let pct = |n: u32| 100 * n / decided;
        resp.on_hover_text(trf(
            "White wins {w} % · draws {d} % · Black wins {b} %",
            &[
                ("w", &pct(white_wins)),
                ("d", &pct(t.draws)),
                ("b", &pct(black_wins)),
            ],
        ));
    }
}

fn score_text(t: &Tally) -> String {
    t.score()
        .map_or_else(|| "—".to_owned(), |s| format!("{:.0} %", 100.0 * s))
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
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let size = vec2(galley.size().x + 8.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, 8.0, review::judgement_color(j));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    resp
}

/// The width of [`counts_text`].
fn errors_text_width(ui: &Ui, c: &ErrorCounts) -> f32 {
    let parts: Vec<String> = [
        (c.inaccuracies, Judgement::Inaccuracy),
        (c.mistakes, Judgement::Mistake),
        (c.blunders, Judgement::Blunder),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, j)| format!("{n} {}", review::judgement_symbol(*j)))
    .collect();
    text_width(ui, &parts.join(" · "), egui::TextStyle::Body) + 8.0 * parts.len() as f32
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
    pub fn new(
        start: PrepStart,
        db: Arc<Database>,
        settings: &Settings,
        ctx: &egui::Context,
    ) -> Self {
        let dir = cache_dir();
        let mut cache = dir.as_deref().map(PositionCache::load).unwrap_or_default();
        // A saved preparation of this player: its depth and games, its report.
        let saved_dir = crate::prep_saved::dir();
        let names = start.player.names(&db);
        let saved_id = crate::prep_saved::id(&start.path, &names);
        let saved = saved_dir
            .as_deref()
            .and_then(|d| crate::prep_saved::get(d, &saved_id));
        let saved_report = saved
            .as_ref()
            .and(saved_dir.as_deref())
            .and_then(|d| crate::prep_saved::report(d, &saved_id));
        let plies = saved
            .as_ref()
            .map_or(settings.prep.plies, |s| s.plies)
            .clamp(2, 60);
        let period = saved.as_ref().map_or(settings.prep.period, |s| s.period);
        let options = Options {
            plies,
            side: Side::Any,
            period,
        };
        // The saved report as it is, when the database is the same; else its
        // evaluations serve the report made again (new games of the database).
        let fresh = saved_report.and_then(|r| {
            if r.search != game_review::SEARCH_VERSION {
                return None;
            }
            if r.fingerprint == prep::fingerprint(&db)
                && r.options() == options
                && r.player == start.player
            {
                return Some(r);
            }
            for (k, e) in r.evals {
                cache.insert(k, e);
            }
            None
        });
        let mut screen = PrepScreen {
            serial: crate::engine_offer::next_serial(),
            db_name: base_name(&start.path),
            path: start.path,
            db,
            player: start.player,
            names,
            shown: true,
            dbs: Vec::new(),
            color: start.side,
            plies,
            period,
            book: start.book,
            saved: saved.is_some(),
            want_period: None,
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
            open_variations: HashSet::new(),
            err_cursor: None,
            book_fp: None,
            book_fp_of: None,
            show_inaccuracies: settings.prep.inaccuracies,
            stacked: false,
            low_window: false,
            selection: Selection::None,
            shown_pos: None,
            flipped: None,
            board_state: BoardState::default(),
            board_popup: false,
            sorts: settings.prep.sorts.clone(),
            focus_games: false,
            games_cursor: None,
            hover_arrow: None,
            scroll_to_selected: false,
            games_height: None,
            auto_engine: std::env::var_os("CP_PREP_ENGINE").is_some(),
        };
        let cached = fresh.or_else(|| {
            dir.as_deref()
                .and_then(|d| PrepReport::load(d, &screen.db, &screen.player, &options))
        });
        match cached {
            Some(r) => screen.take_report(r),
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
    pub fn start_after_engine(&mut self, ctx: &egui::Context) -> Vec<PrepAction> {
        let mut actions = Vec::new();
        self.start_engine(ctx, &mut actions);
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
        let options = self.options();
        self.build_total = 0;
        let now = ctx.input(|i| i.time);
        self.build = Background::spawn(ctx, now, move |progress, cancel| {
            prep::build(&db, &player, &options, progress, cancel)
        });
    }

    /// A report arrived (built or from the cache): the rows of the games list, the
    /// cached evaluations, the default expansion.
    fn take_report(&mut self, mut report: PrepReport) {
        let taken = report.take_cached(&self.cache);
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
        // Everything folded: the user opens what he wants (the roots too).
        self.expanded.clear();
        self.others_open.clear();
        self.open_families.clear();
        self.open_variations.clear();
        self.open_repeated.clear();
        // A book step of the report this one replaces is not this report's.
        self.book_job = None;
        self.report = Some(report);
        self.rebuild_marks();
        self.selection = Selection::None;
        self.shown_pos = None;
        self.games_cursor = None;
        self.err_cursor = None;
    }

    /// The same player asked for again from the dialog: its colour and book.
    pub fn reopen(&mut self, side: ColorChoice, book: BookChoice) {
        self.shown = true;
        // Deleted from «Preparations» while this screen was hidden: «Save» again.
        self.saved &= crate::prep_saved::dir().is_some_and(|d| {
            crate::prep_saved::get(&d, &crate::prep_saved::id(&self.path, &self.names)).is_some()
        });
        self.set_color(side);
        if book != self.book {
            self.book = book;
            self.book_failed = None;
            self.book_asked = None;
        }
    }

    fn set_color(&mut self, color: ColorChoice) {
        // The colour a saved preparation opens with.
        self.dirty |= self.saved && color != self.color;
        self.color = color;
        // A selection of a colour no longer shown is dropped.
        let keep = match &self.selection {
            Selection::None => true,
            Selection::Node { white, .. }
            | Selection::Opening { white, .. }
            | Selection::Repeated { white, .. }
            | Selection::Error { white, .. } => color.colours().contains(white),
        };
        if !keep {
            self.select(Selection::None);
        }
    }

    /// Games of the colour(s) shown.
    fn shown_games(&self) -> usize {
        self.report.as_ref().map_or(0, |r| {
            self.color
                .colours()
                .iter()
                .map(|&w| r.color(w).games.len())
                .sum()
        })
    }

    /// What the report is built with.
    fn options(&self) -> Options {
        Options {
            plies: self.plies,
            side: Side::Any,
            period: self.period,
        }
    }

    /// Another depth: the report of that depth from the cache, else built anew.
    fn set_plies(&mut self, plies: usize, settings: &mut Settings, ctx: &egui::Context) {
        if plies == self.plies {
            return;
        }
        self.stop_engine(&mut Vec::new());
        self.save();
        self.plies = plies;
        settings.prep.plies = plies;
        self.load_or_build(ctx);
    }

    /// Other games of the player: the report of them from the cache, else built anew.
    fn set_period(&mut self, period: Period, settings: &mut Settings, ctx: &egui::Context) {
        if period == self.period {
            return;
        }
        self.stop_engine(&mut Vec::new());
        self.save();
        self.period = period;
        settings.prep.period = period;
        self.load_or_build(ctx);
    }

    /// The report of the current options from the cache, else built anew.
    fn load_or_build(&mut self, ctx: &egui::Context) {
        self.report = None;
        let options = self.options();
        let cached = cache_dir()
            .as_deref()
            .and_then(|d| PrepReport::load(d, &self.db, &self.player, &options));
        match cached {
            Some(r) => {
                self.take_report(r);
                self.dirty |= self.saved;
            }
            None => self.rebuild(ctx),
        }
    }

    /// The report's book is the one chosen (else its column and line are not shown).
    fn book_ok(&self) -> bool {
        let Some(report) = &self.report else {
            return false;
        };
        self.book_fp.is_some() && report.book.as_ref().map(|b| b.fingerprint) == self.book_fp
    }

    /// The judgement is shown (inaccuracies can be hidden).
    fn visible(&self, j: Judgement) -> bool {
        self.show_inaccuracies || j != Judgement::Inaccuracy
    }

    /// The counts as shown.
    fn shown_counts(&self, c: &ErrorCounts) -> ErrorCounts {
        let mut c = *c;
        if !self.show_inaccuracies {
            c.inaccuracies = 0;
        }
        c
    }

    /// The engine run for the colour(s) shown.
    fn engine_state(&self) -> EngineState {
        if self.job.is_some() {
            return EngineState::Running;
        }
        let Some(report) = &self.report else {
            return EngineState::NotRun;
        };
        if !report.evaluated() {
            return EngineState::NotRun;
        }
        let side = self.color.side();
        let total = report.distinct_positions_for(side);
        let missing = report.missing_for(side);
        if missing == 0 {
            EngineState::Complete { total }
        } else {
            EngineState::Partial {
                done: total - missing,
                total,
            }
        }
    }

    /// Judges the errors anew (`f` changes the evaluations): the lists of errors are
    /// rebuilt, so a chosen error is found again in them by what it is, not by its
    /// place (else the choice is dropped).
    fn rejudge(&mut self, f: impl FnOnce(&mut PrepReport)) {
        let Some(report) = self.report.as_mut() else {
            return;
        };
        // What the selection and the errors tab's row are, by content.
        let error_id = |r: &PrepReport, white: bool, i: usize| {
            r.color(white).errors.get(i).map(|e| (e.game, e.ply))
        };
        let repeated_id = |r: &PrepReport, white: bool, i: usize| {
            r.color(white)
                .repeated
                .get(i)
                .map(|x| (x.key, x.uci.clone()))
        };
        let sel = match &self.selection {
            Selection::Error { white, index } => {
                error_id(report, *white, *index).map(|id| (*white, Ok(id)))
            }
            Selection::Repeated { white, index } => {
                repeated_id(report, *white, *index).map(|id| (*white, Err(id)))
            }
            _ => None,
        };
        let cursor = match self.err_cursor {
            Some(ErrRow::Error { white, index }) => {
                error_id(report, white, index).map(|id| (white, Ok(id)))
            }
            Some(ErrRow::Repeated { white, index }) => {
                repeated_id(report, white, index).map(|id| (white, Err(id)))
            }
            _ => None,
        };
        let open: Vec<(bool, (u64, String))> = self
            .open_repeated
            .iter()
            .filter_map(|&(w, i)| repeated_id(report, w, i).map(|id| (w, id)))
            .collect();
        f(report);
        let find = |r: &PrepReport,
                    white: bool,
                    id: &Result<(u32, u32), (u64, String)>|
         -> Option<usize> {
            match id {
                Ok((game, ply)) => r
                    .color(white)
                    .errors
                    .iter()
                    .position(|e| e.game == *game && e.ply == *ply),
                Err((key, uci)) => r
                    .color(white)
                    .repeated
                    .iter()
                    .position(|x| x.key == *key && x.uci == *uci),
            }
        };
        if let Some((white, id)) = &sel {
            let found = find(report, *white, id);
            self.selection = match (found, id) {
                (Some(index), Ok(_)) => Selection::Error {
                    white: *white,
                    index,
                },
                (Some(index), Err(_)) => Selection::Repeated {
                    white: *white,
                    index,
                },
                (None, _) => Selection::None,
            };
            if found.is_none() {
                self.shown_pos = None;
            }
        }
        if let Some((white, id)) = &cursor {
            self.err_cursor = match (find(report, *white, id), id) {
                (Some(index), Ok(_)) => Some(ErrRow::Error {
                    white: *white,
                    index,
                }),
                (Some(index), Err(_)) => Some(ErrRow::Repeated {
                    white: *white,
                    index,
                }),
                (None, _) => None,
            };
        } else if matches!(self.err_cursor, Some(ErrRow::RepeatedGame { .. })) {
            self.err_cursor = None;
        }
        self.open_repeated = open
            .into_iter()
            .filter_map(|(w, (key, uci))| {
                let i = report
                    .color(w)
                    .repeated
                    .iter()
                    .position(|x| x.key == key && x.uci == uci)?;
                Some((w, i))
            })
            .collect();
        self.rebuild_marks();
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
                Some(report) => {
                    self.take_report(report);
                    // A saved preparation follows its new games and depth.
                    self.dirty |= self.saved;
                }
                None => self.error = Some(tr("The games could not be read").to_owned()),
            }
        }
        self.show_inaccuracies = settings.prep.inaccuracies;
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
                    self.book_failed = Some((
                        fp,
                        tr("the position search failed (not enough memory?)").to_owned(),
                    ))
                }
            }
        }
        self.poll_engine(ctx, actions);
        self.start_book(ctx, actions);
        if self.auto_engine && self.report.is_some() && self.build.is_none() {
            self.auto_engine = false;
            self.start_engine(ctx, actions);
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
        let book = match &self.book {
            BookChoice::None => None,
            BookChoice::Same => Some((self.db.clone(), true)),
            BookChoice::Database(p) => match self.dbs.iter().find(|d| d.path == *p) {
                Some(d) => match &d.db {
                    Some(db) => Some((db.clone(), false)),
                    None => {
                        if d.failed.is_none() && self.book_asked.as_ref() != Some(p) {
                            self.book_asked = Some(p.clone());
                            actions.push(PrepAction::LoadDb(p.clone()));
                        }
                        None
                    }
                },
                None => None,
            },
        };
        let Some((book_db, same)) = book else {
            self.book_fp = None;
            self.book_fp_of = None;
            return;
        };
        // The fingerprint reads the file's data: once per book and database read.
        let of = (self.book.clone(), Arc::as_ptr(&book_db) as usize);
        if self.book_fp_of.as_ref() != Some(&of) {
            self.book_fp = Some(prep::fingerprint(&book_db));
            self.book_fp_of = Some(of);
        }
        let Some(fp) = self.book_fp else { return };
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
        if !self.book_ok() {
            return None;
        }
        self.report.as_ref()?.book_counts.get(&key).copied()
    }

    // ----- the engine ----------------------------------------------------------------

    /// Positions the next run has to evaluate, and how long it takes.
    fn estimate(&self) -> (usize, f64) {
        let Some(report) = &self.report else {
            return (0, 0.0);
        };
        let missing = report.missing_for(self.color.side());
        (missing, self.secs_for(missing, review::processes()))
    }

    /// Seconds `positions` take in `processes` engine processes.
    fn secs_for(&self, positions: usize, processes: usize) -> f64 {
        let nps = self.nps_per_thread.unwrap_or(review::DEFAULT_NPS);
        game_review::estimate_secs(positions, nps, processes)
    }

    /// The button: the dialog the first time, at once with the colour chosen before.
    fn engine_asked(
        &mut self,
        settings: &Settings,
        ctx: &egui::Context,
        actions: &mut Vec<PrepAction>,
    ) {
        if crate::review_engine::engine().is_none() {
            actions.push(PrepAction::NeedEngine);
            return;
        }
        if settings.prep.chosen {
            self.start_engine(ctx, actions);
        } else {
            self.dialog_open = true;
        }
    }

    /// Starts the search for opening errors: Stockfish 19 (`review_engine`) with the fixed
    /// search of the game analysis (`game_review::run`), the same on every computer.
    fn start_engine(&mut self, ctx: &egui::Context, actions: &mut Vec<PrepAction>) {
        self.error = None;
        let Some(path) = crate::review_engine::engine() else {
            actions.push(PrepAction::NeedEngine);
            return;
        };
        let cache = std::mem::take(&mut self.cache);
        let mut taken = 0;
        self.rejudge(|r| taken = r.take_cached(&cache));
        self.cache = cache;
        self.dirty |= taken > 0;
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let targets = report.targets_for(self.color.side());
        if targets.is_empty() {
            actions.push(PrepAction::Toast(
                tr("Every position is already evaluated").to_owned(),
            ));
            self.tab = Tab::Errors;
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = targets.len();
        let job_settings = JobSettings::new(review::processes(), None);
        let processes = job_settings.processes;
        let spawned = review::spawn_job(
            "prep-engine",
            path,
            job_settings,
            targets,
            cancel.clone(),
            tx,
            ctx.clone(),
        );
        let runner = match spawned {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        self.job = Some(EngineJob {
            runner,
            rx,
            cancel,
            total,
            done: 0,
            processes,
            unsaved: 0,
        });
        self.tab = Tab::Errors;
        actions.push(PrepAction::EngineStarted);
    }

    fn poll_engine(&mut self, ctx: &egui::Context, actions: &mut Vec<PrepAction>) {
        let Some(job) = self.job.as_mut() else { return };
        let Some(report) = self.report.as_mut() else {
            return;
        };
        let mut ended = false;
        let mut got = false;
        job.runner.pump();
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                JobMsg::Event(e) => {
                    if let JobEvent::Eval { nps, .. } = &e {
                        job.done += 1;
                        // One engine thread per process.
                        if *nps > 0 {
                            self.nps_per_thread = Some(*nps as f64);
                        }
                    }
                    if let Some((k, eval)) = report.apply(&e) {
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
            self.pending_refresh = false;
            self.last_refresh = now;
            self.rejudge(PrepReport::refresh);
        }
        if ended {
            self.job = None;
            self.save();
            actions.push(PrepAction::EngineEnded);
        }
    }

    /// Stops the engine run (quitting): what was evaluated is kept.
    pub fn stop(&mut self) {
        self.stop_engine(&mut Vec::new());
    }

    /// «Stop»: what was evaluated stays.
    fn stop_engine(&mut self, actions: &mut Vec<PrepAction>) {
        if self.job.take().is_some() {
            self.rejudge(PrepReport::refresh);
            self.pending_refresh = false;
            self.save();
            actions.push(PrepAction::EngineEnded);
        }
    }

    /// Writes the report and the evaluations to the cache, and to «Preparations» when
    /// the preparation is kept there.
    pub fn save(&mut self) {
        let Some(dir) = cache_dir() else { return };
        if let Err(e) = self.cache.save(&dir) {
            eprintln!("prep cache: {}: {e}", dir.display());
        }
        if self.dirty
            && let Some(r) = &self.report
        {
            let mut ok = true;
            if let Err(e) = r.save(&dir) {
                ok = false;
                eprintln!("prep report: {}: {e}", dir.display());
            }
            // Deleted from «Preparations» meanwhile: not written back.
            self.saved &= crate::prep_saved::dir().is_some_and(|d| {
                crate::prep_saved::get(&d, &crate::prep_saved::id(&self.path, &self.names))
                    .is_some()
            });
            if self.saved
                && let Err(e) = self.keep()
            {
                ok = false;
                eprintln!("prep saved: {e}");
            }
            if ok {
                self.dirty = false;
            }
        }
    }

    /// Writes the preparation to «Preparations» (its name in the list kept).
    fn keep(&self) -> std::io::Result<()> {
        let dir =
            crate::prep_saved::dir().ok_or_else(|| std::io::Error::other("no data folder"))?;
        let Some(report) = &self.report else {
            return Ok(());
        };
        let id = crate::prep_saved::id(&self.path, &self.names);
        let title = crate::prep_saved::get(&dir, &id)
            .map(|m| m.title)
            .unwrap_or_else(|| self.names.first().cloned().unwrap_or_default());
        let mut errors = ErrorCounts::default();
        for c in [&report.white, &report.black] {
            errors.inaccuracies += c.counts.inaccuracies;
            errors.mistakes += c.counts.mistakes;
            errors.blunders += c.counts.blunders;
        }
        let meta = crate::prep_saved::SavedPrep {
            title,
            path: self.path.clone(),
            player: self.player.clone(),
            names: self.names.clone(),
            plies: self.plies,
            period: self.period,
            color: self.color,
            book: self.book.setting(&self.path),
            saved: crate::prep_saved::now(),
            games: report.games.len() as u32,
            errors,
            evaluated: report.evaluated(),
        };
        crate::prep_saved::save(&dir, &meta, report)
    }

    /// «💾 Save»: the preparation goes to «Preparations» and is kept there from now on.
    fn save_to_list(&mut self, actions: &mut Vec<PrepAction>) {
        self.saved = true;
        match self.keep() {
            Ok(()) => actions.push(PrepAction::Toast(
                tr("Saved: the library's «Preparations» opens it").to_owned(),
            )),
            Err(e) => {
                self.saved = false;
                actions.push(PrepAction::Toast(trf(
                    "Could not save: {e}",
                    &[("e", &e.to_string())],
                )));
            }
        }
    }

    /// The launch dialog (§4.2).
    fn engine_dialog(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        actions: &mut Vec<PrepAction>,
    ) {
        if !self.dialog_open {
            return;
        }
        let mut start = false;
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
                    ui.add_space(6.0);
                    let (missing, secs) = self.estimate();
                    let all = self
                        .report
                        .as_ref()
                        .map_or(0, |r| r.distinct_positions_for(self.color.side()));
                    ui.label(trf(
                        "{n} positions, about {time}",
                        &[("n", &grouped(missing)), ("time", &review::format_duration(secs))],
                    ));
                    let cached = all.saturating_sub(missing);
                    let note = if cached > 0 {
                        trf(
                            "identical positions are counted once; {n} already evaluated earlier",
                            &[("n", &grouped(cached))],
                        )
                    } else {
                        tr("identical positions are counted once").to_owned()
                    };
                    ui.label(RichText::new(note).weak().small());
                });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new(tr("Find errors")).strong().size(16.0))
                    .clicked()
                {
                    start = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.dialog_open = false;
                }
            });
        });
        if modal.should_close() {
            self.dialog_open = false;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            start = true;
        }
        if start {
            self.dialog_open = false;
            settings.prep.chosen = true;
            actions.push(PrepAction::SettingsChanged);
            self.engine_asked(settings, ctx, actions);
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
                                let ply = family_ply(g, &fam.name)
                                    .unwrap_or(g.named.unwrap_or(0) as usize);
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
                s.arrows = error_arrows(
                    &s.positions[s.ply],
                    &r.uci,
                    cr.errors
                        .iter()
                        .find(|e| e.key == r.key && e.uci == r.uci)
                        .map(|e| e.best_uci.as_str()),
                    r.judgement,
                );
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
                s.arrows =
                    error_arrows(&s.positions[s.ply], &e.uci, Some(&e.best_uci), e.judgement);
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
        let list: Vec<u32> = match &self.selection {
            Selection::None => self
                .color
                .colours()
                .iter()
                .flat_map(|&w| report.color(w).games.iter().copied())
                .collect(),
            // The games of the position, by any move order.
            Selection::Node { white, node } => {
                let tree = &report.color(*white).tree;
                tree.nodes
                    .get(*node as usize)
                    .map(|n| tree.position_games(n.key))
                    .unwrap_or_default()
            }
            Selection::Opening {
                white,
                family,
                variation,
            } => report
                .color(*white)
                .families
                .get(*family)
                .map(|f| match variation {
                    Some(v) => f
                        .variations
                        .get(*v)
                        .map(|v| v.games.clone())
                        .unwrap_or_default(),
                    None => f
                        .variations
                        .iter()
                        .flat_map(|v| v.games.iter().copied())
                        .collect(),
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
        let sort = self.sorts.games;
        let val = |gi: u32| -> SortVal {
            let g = &report.games[gi as usize];
            let r = &self.rows[gi as usize];
            match sort.key {
                GameSort::White => SortVal::Text(fold(&r.white)),
                GameSort::Black => SortVal::Text(fold(&r.black)),
                GameSort::WhiteElo => num(r.white_elo),
                GameSort::BlackElo => num(r.black_elo),
                GameSort::Result => SortVal::Text(r.result.to_owned()),
                GameSort::Date => num(g.date_number()),
                GameSort::Then => SortVal::Text(
                    g.sans
                        .iter()
                        .skip(self.game_ply(gi))
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                GameSort::Event => SortVal::Text(fold(&r.event)),
                GameSort::Errors => num(report
                    .color(g.white)
                    .errors
                    .iter()
                    .filter(|e| e.game == gi && self.visible(e.judgement))
                    .map(|e| judgement_weight(e.judgement))
                    .sum::<u32>()),
            }
        };
        let mut keyed: Vec<(SortVal, u32)> = list.into_iter().map(|g| (val(g), g)).collect();
        keyed.sort_by(|a, b| sort.apply(cmp_val(&a.0, &b.0)));
        keyed.into_iter().map(|(_, g)| g).collect()
    }

    /// The move of the selection to open a game at.
    fn selection_ply(&self) -> usize {
        self.shown_pos.as_ref().map_or(0, |s| s.ucis.len())
    }

    /// Where game `gi` reaches the chosen position: a node's position may come by
    /// another move order (another ply) in some of its games.
    fn game_ply(&self, gi: u32) -> usize {
        if let (Selection::Node { white, node }, Some(report)) = (&self.selection, &self.report)
            && let Some(n) = report.color(*white).tree.nodes.get(*node as usize)
            && let Some(p) = report.games[gi as usize]
                .keys
                .iter()
                .position(|k| *k == n.key)
        {
            return p;
        }
        self.selection_ply()
    }

    /// The move to open game `gi` at: the chosen position, or after the error.
    fn open_ply(&self, gi: u32) -> usize {
        let after = matches!(
            self.selection,
            Selection::Error { .. } | Selection::Repeated { .. }
        );
        self.game_ply(gi) + usize::from(after)
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
        self.stacked = stacked;
        self.low_window = h < 600.0;
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
                        ui.vertical(|ui| {
                            self.board_caption(ui);
                            self.board_info(ui, &mut actions);
                        });
                    });
                    ui.add_space(4.0);
                });
            }
            egui::CentralPanel::default()
                .show(ui, |ui| self.right_column(ui, settings, &mut actions));
            if low && self.board_popup {
                self.board_overlay(&ctx, settings, &mut actions);
            }
        } else {
            let board_side = (h - 120.0)
                .min(420.0)
                .min(w - BOARD_MIN - 24.0 - RIGHT_MIN + BOARD_MIN)
                .max(BOARD_MIN)
                .floor();
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
            egui::CentralPanel::default()
                .show(ui, |ui| self.right_column(ui, settings, &mut actions));
        }
        self.engine_dialog(&ctx, settings, &mut actions);
        if let Some(p) = self.want_period.take() {
            self.set_period(p, settings, &ctx);
            actions.push(PrepAction::SettingsChanged);
        }
        if !typing && !overlay {
            self.keyboard(&ctx, settings, &mut actions);
        }
        if settings.prep.sorts != self.sorts {
            settings.prep.sorts = self.sorts.clone();
            actions.push(PrepAction::SettingsChanged);
        }
        actions
    }

    fn header(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        let gap = ui.spacing().item_spacing.x;
        let button_w = |ui: &Ui, text: &str| {
            text_width(ui, text, egui::TextStyle::Button)
                + 2.0 * ui.spacing().button_padding.x
                + gap
        };
        let row = ui.available_width();
        let compact = row < 700.0;
        let left = button_w(ui, if compact { "⬅" } else { tr("⬅ Library") }) + 2.0 * gap;
        let save_label = match (self.saved, compact) {
            (false, false) => tr("★ Save"),
            (true, false) => tr("★ Saved"),
            (_, true) => "★",
        };
        let right =
            button_w(ui, if compact { "⚙" } else { tr("⚙ Settings") }) + button_w(ui, save_label);
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
                parts.push(crate::i18n::games(self.shown_games()));
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
                        egui::Label::new(RichText::new(format!("· {}", parts.join(" · "))).weak())
                            .truncate(),
                    );
                    if skipped > 0 {
                        r.on_hover_text(trf(
                            "not counted: from a set position — {n}",
                            &[("n", &skipped)],
                        ));
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
                let games = self.shown_games();
                if self.saved {
                    // A quiet mark, not a button that looks unavailable.
                    ui.label(RichText::new(save_label).color(ui.visuals().selection.bg_fill))
                        .on_hover_text(tr(
                            "Kept in the library's «Preparations»: changes are saved by themselves",
                        ));
                } else if ui
                    .add_enabled(games > 0, egui::Button::new(save_label))
                    .on_hover_text(tr(
                        "Keep this preparation with the errors found: the library's «Preparations» opens it again",
                    ))
                    .clicked()
                {
                    self.save_to_list(actions);
                }
            });
        });
    }

    /// The colour switch, the book, the depth and the engine button or its progress.
    fn controls(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        low: bool,
        actions: &mut Vec<PrepAction>,
    ) {
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let pad = 2.0 * ui.spacing().button_padding.x;
        let state = self.engine_state();
        // The engine part on the right is measured first: the rest gets what it leaves.
        let engine = self.engine_texts(state, width);
        let engine_w = match &engine {
            EngineTexts::Running => {
                (width * 0.35).clamp(90.0, 360.0)
                    + text_width(ui, tr("Stop"), egui::TextStyle::Button)
                    + pad
                    + 2.0 * gap
            }
            EngineTexts::Button { label, note } | EngineTexts::Done { label, note } => {
                text_width(ui, label, egui::TextStyle::Button)
                    + pad
                    + note
                        .as_deref()
                        .map_or(0.0, |n| text_width(ui, n, egui::TextStyle::Body) + gap)
                    + gap
            }
        } + 26.0
            + gap;
        ui.horizontal(|ui| {
            for choice in [ColorChoice::White, ColorChoice::Black, ColorChoice::Both] {
                if ui
                    .selectable_label(self.color == choice, choice.label())
                    .clicked()
                {
                    self.set_color(choice);
                }
            }
            ui.separator();
            let caption_apart = width >= 640.0;
            if caption_apart {
                ui.label(tr("Book"));
            }
            let book_w = if width < 700.0 {
                120.0
            } else if width < 900.0 {
                150.0
            } else {
                240.0
            };
            if let Some(b) = book_picker(
                ui,
                "prep_book",
                &self.book,
                &self.path,
                &self.dbs,
                book_w,
                !caption_apart,
            ) {
                self.book = b;
                self.book_failed = None;
                self.book_asked = None;
                settings.prep.book = Some(self.book.setting(&self.path));
                actions.push(PrepAction::SettingsChanged);
            }
            if let Some((_, why)) = &self.book_failed {
                ui.label(RichText::new("⚠").color(ui.visuals().warn_fg_color))
                    .on_hover_text(trf("Book: could not open — {reason}", &[("reason", why)]));
            }
            ui.separator();
            let moves = self.plies.div_ceil(2);
            let board_w = if low {
                text_width(ui, "♙", egui::TextStyle::Button) + pad + 2.0 * gap
            } else {
                0.0
            };
            // The period takes what the depth leaves at its shortest.
            let depth_least = text_width(
                ui,
                &trf("{n} m.", &[("n", &moves)]),
                egui::TextStyle::Button,
            ) + pad
                + 20.0
                + 2.0 * gap;
            let period_room = ui.available_width() - engine_w - board_w - depth_least - 2.0 * gap;
            self.period_picker(ui, settings, period_room, actions);
            ui.separator();
            // The depth's caption by the room left: «Opening: 10 moves», «10 moves», «10 m.».
            let room = ui.available_width() - engine_w - board_w;
            let depth_label = [
                trf("Opening: {n} moves", &[("n", &moves)]),
                trf("{n} moves", &[("n", &moves)]),
                trf("{n} m.", &[("n", &moves)]),
            ]
            .into_iter()
            .find(|l| text_width(ui, l, egui::TextStyle::Button) + pad + 20.0 + gap <= room)
            .unwrap_or_else(|| trf("{n} m.", &[("n", &moves)]));
            let mut new_plies = None;
            let depth_w = text_width(ui, &depth_label, egui::TextStyle::Button) + 8.0;
            egui::ComboBox::from_id_salt("prep_plies")
                .width(depth_w)
                .selected_text(depth_label)
                .show_ui(ui, |ui| {
                    for m in [6usize, 10, 15] {
                        if ui
                            .selectable_label(
                                self.plies == m * 2,
                                trf("Opening: {n} moves", &[("n", &m)]),
                            )
                            .clicked()
                        {
                            new_plies = Some(m * 2);
                        }
                    }
                })
                .response
                .on_hover_text(tr("How deep the tree and the engine look"));
            if let Some(p) = new_plies {
                self.set_plies(p, settings, ui.ctx());
                actions.push(PrepAction::SettingsChanged);
            }
            if low {
                ui.separator();
                if ui
                    .selectable_label(self.board_popup, "♙")
                    .on_hover_text(tr("The board of the chosen position"))
                    .clicked()
                {
                    self.board_popup = !self.board_popup;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.engine_control(ui, settings, engine, width, actions);
            });
        });
    }

    /// «Last 2 years · 80 ▾»: which of the player's games, with how many each takes of
    /// the colour(s) shown; shorter («2 yrs · 80», «80») when `room` is short.
    fn period_picker(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        room: f32,
        actions: &mut Vec<PrepAction>,
    ) {
        let side = self.color.side();
        let report = self.report.as_ref();
        let dated = report.is_some_and(|r| r.pool.iter().any(|g| g.date != 0));
        // Without dates a period by months takes all the games: it is shown as all.
        let shown = match self.period {
            Period::Months(_) if report.is_some() && !dated => Period::All,
            p => p,
        };
        let number = |p: Period| -> String {
            let Some(r) = report else {
                return "…".to_owned();
            };
            match (p, side) {
                // «100 + 100»: the last N of each colour, not N in all.
                (Period::Last(_), Side::Any) => format!(
                    "{} + {}",
                    grouped(r.count(p, Side::White).games),
                    grouped(r.count(p, Side::Black).games)
                ),
                _ => grouped(r.count(p, side).games),
            }
        };
        let pad = 2.0 * ui.spacing().button_padding.x + 20.0;
        let selected = [
            format!("{} · {}", period_label(shown), number(shown)),
            format!("{} · {}", period_short(shown), number(shown)),
        ]
        .into_iter()
        .find(|t| text_width(ui, t, egui::TextStyle::Button) + pad <= room)
        .unwrap_or_else(|| number(shown));
        let mut picked = None;
        // As wide as its text: the room is the row's.
        let width = text_width(ui, &selected, egui::TextStyle::Button) + 8.0;
        let r = egui::ComboBox::from_id_salt("prep_period")
            .width(width)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for p in PERIODS {
                    // Without dates: by number only.
                    if matches!(p, Period::Months(_)) && !dated {
                        continue;
                    }
                    let text = format!("{} · {}", period_label(p), number(p));
                    if ui.selectable_label(shown == p, text).clicked() {
                        picked = Some(p);
                    }
                }
                let note = |ui: &mut Ui, text: String| {
                    ui.label(RichText::new(text).weak().small());
                };
                if !dated {
                    note(ui, tr("The games have no dates: by number only").to_owned());
                } else if let Some(r) = report {
                    let newest = prep::period_newest(&r.pool);
                    note(
                        ui,
                        trf(
                            "Counted back from his newest game — {date}",
                            &[("date", &shown_date(newest))],
                        ),
                    );
                    let out = r.count(Period::Months(12), side).undated_out;
                    if out > 0 {
                        note(
                            ui,
                            trf(
                                "Without a date: {n} — taken only with all games",
                                &[("n", &grouped(out))],
                            ),
                        );
                    }
                }
            })
            .response;
        r.on_hover_text(tr(
            "Which of his games: by date — counted back from his newest game; by number — the newest of each colour",
        ));
        if let Some(p) = picked {
            self.set_period(p, settings, ui.ctx());
            actions.push(PrepAction::SettingsChanged);
        }
    }

    /// What the engine part of the controls says, by the room there is.
    fn engine_texts(&self, state: EngineState, width: f32) -> EngineTexts {
        let (missing, secs) = self.estimate();
        let time = review::format_duration(secs);
        match state {
            EngineState::Running => EngineTexts::Running,
            EngineState::NotRun => EngineTexts::Button {
                label: if width < 700.0 {
                    format!("▶ ≈ {time}")
                } else if width < 900.0 {
                    trf("▶ Errors ≈ {time}", &[("time", &time)])
                } else {
                    trf("▶ Find opening errors ≈ {time}", &[("time", &time)])
                },
                note: None,
            },
            EngineState::Partial { done, total } => EngineTexts::Button {
                label: if width < 700.0 {
                    format!("▶ ≈ {time}")
                } else {
                    trf("Continue ≈ {time}", &[("time", &time)])
                },
                note: (width >= 900.0).then(|| {
                    trf(
                        "Checked {done} of {total} positions",
                        &[("done", &grouped(done)), ("total", &grouped(total))],
                    )
                }),
            },
            EngineState::Complete { .. } => {
                let _ = missing;
                EngineTexts::Done {
                    label: tr("Errors found").to_owned(),
                    note: None,
                }
            }
        }
    }

    /// The engine button, the progress while it runs, the state after; «▾» (the
    /// options) at the right end.
    fn engine_control(
        &mut self,
        ui: &mut Ui,
        settings: &Settings,
        texts: EngineTexts,
        width: f32,
        actions: &mut Vec<PrepAction>,
    ) {
        if ui.button("▾").on_hover_text(tr("Options…")).clicked() {
            self.dialog_open = true;
        }
        if let Some(job) = &self.job {
            let (done, total) = (job.done, job.total);
            if ui.button(tr("Stop")).on_hover_text("Shift+A").clicked() {
                self.stop_engine(actions);
                return;
            }
            let secs = self.secs_for(total.saturating_sub(done), job.processes);
            let text = if width < 760.0 {
                format!("{}/{}", grouped(done), grouped(total))
            } else {
                trf(
                    "Finding errors: {done} of {total} positions · ≈ {time}",
                    &[
                        ("done", &grouped(done)),
                        ("total", &grouped(total)),
                        ("time", &review::format_duration(secs)),
                    ],
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
        let hint = self.engine_status().unwrap_or_else(|| {
            tr("The engine finds the opening errors in his games (Shift+A)").to_owned()
        });
        let _ = report;
        match texts {
            EngineTexts::Done { label, .. } => {
                ui.label(RichText::new(label).weak()).on_hover_text(hint);
            }
            EngineTexts::Button { label, note } => {
                // Nothing to search in no games.
                let some = self.shown_games() > 0;
                if ui
                    .add_enabled_ui(some, |ui| crate::widgets::primary(ui, &label))
                    .inner
                    .on_hover_text(hint)
                    .clicked()
                {
                    self.engine_asked(settings, ui.ctx(), actions);
                }
                if let Some(n) = note {
                    ui.label(RichText::new(n).weak());
                }
            }
            EngineTexts::Running => {}
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new("⚠").color(ui.visuals().error_fg_color))
                .on_hover_text(e);
        }
    }

    /// «Errors found · Stockfish 19 · 2 947 positions», once evaluated.
    fn engine_status(&self) -> Option<String> {
        let report = self.report.as_ref()?;
        if !report.evaluated() {
            return None;
        }
        let n = grouped(report.distinct_positions_for(self.color.side()));
        Some(
            match report.engine.as_ref().filter(|e| !e.name.is_empty()) {
                Some(e) => trf(
                    "Errors found · {engine} · {n} positions",
                    &[("engine", &e.name), ("n", &n)],
                ),
                None => trf("Errors found · {n} positions", &[("n", &n)]),
            },
        )
    }

    /// The summary: a row per colour in columns — colour, games, score, opponents,
    /// the last game, the book, the errors; the columns give way from the right (the
    /// last game, the book, the opponents) instead of wrapping.
    fn summary(&self, ui: &mut Ui, stacked: bool) {
        let Some(report) = &self.report else { return };
        let colours = if stacked {
            self.color.colours()
        } else {
            vec![true, false]
        };
        let evaluated = report.evaluated() && !stacked;
        let book = self.book_ok();
        // Texts per colour: [games, score, opponents, last, book].
        let mut rows: Vec<(bool, [String; 5], ErrorCounts, usize)> = Vec::new();
        for &white in &colours {
            let cr = report.color(white);
            let root = &cr.tree.nodes[0];
            let tally = &root.tally;
            let title = if white {
                tr("As White")
            } else {
                tr("As Black")
            };
            let games = format!("{title}  {}", tally.games);
            let score = if tally.games > 0 {
                format!("{} ({})", score_text(tally), tally_hint(tally))
            } else {
                String::new()
            };
            let opponents = root
                .average_elo()
                .map(|elo| trf("opponents ≈ {elo}", &[("elo", &elo)]))
                .unwrap_or_default();
            let last = crate::library::display_date(&root.last_date);
            let last = if last.is_empty() {
                String::new()
            } else {
                trf("last {date}", &[("date", &last)])
            };
            let book_text = if book {
                let mut plies: Vec<u32> = cr
                    .games
                    .iter()
                    .filter_map(|&g| report.games[g as usize].book.map(|b| b.ply))
                    .collect();
                plies.sort_unstable();
                match plies.get(plies.len() / 2) {
                    Some(median) => {
                        let (own, other) = cr.book_exits(&report.games);
                        let p = 100 * own / (own + other).max(1);
                        trf(
                            "leaves the book at move {n}, first in {p} % of games",
                            &[("n", &median.div_ceil(2).max(1)), ("p", &p)],
                        )
                    }
                    None => String::new(),
                }
            } else if self.book_job.is_some() {
                tr("book…").to_owned()
            } else {
                String::new()
            };
            let with_errors: HashSet<u32> = cr
                .errors
                .iter()
                .filter(|e| self.visible(e.judgement))
                .map(|e| e.game)
                .collect();
            let p = 100 * with_errors.len() / cr.games.len().max(1);
            rows.push((
                white,
                [games, score, opponents, last, book_text],
                self.shown_counts(&cr.counts),
                p,
            ));
        }
        // Which columns fit: the errors stay; last, book, opponents give way in turn.
        let gap = 14.0;
        let col_w = |k: usize| -> f32 {
            rows.iter()
                .map(|r| text_width(ui, &r.1[k], egui::TextStyle::Body))
                .fold(0.0, f32::max)
        };
        let errors_w = if evaluated {
            rows.iter()
                .map(|(_, _, c, p)| {
                    errors_text_width(ui, c)
                        + text_width(
                            ui,
                            &trf("in {p} % of games", &[("p", p)]),
                            egui::TextStyle::Body,
                        )
                        + 12.0
                })
                .fold(0.0, f32::max)
        } else {
            0.0
        };
        let width = ui.available_width();
        let mut shown = [true; 5];
        let used = |shown: &[bool; 5]| -> f32 {
            (0..5)
                .filter(|&k| shown[k])
                .map(|k| col_w(k) + gap)
                .sum::<f32>()
                + errors_w
        };
        for k in [3, 4, 2, 1] {
            if used(&shown) <= width {
                break;
            }
            shown[k] = false;
        }
        egui::Grid::new("prep_summary")
            .num_columns(6)
            .spacing(vec2(gap, 2.0))
            .show(ui, |ui| {
                for (white, texts, counts, p) in &rows {
                    let weak = !self.color.colours().contains(white);
                    for k in 0..5 {
                        if !shown[k] {
                            continue;
                        }
                        let rt = RichText::new(&texts[k]);
                        let rt = if k == 0 { rt.strong() } else { rt };
                        ui.label(if weak { rt.weak() } else { rt });
                    }
                    if evaluated {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if counts.total() > 0 {
                                counts_text(ui, counts);
                                ui.label(
                                    RichText::new(trf("in {p} % of games", &[("p", p)])).weak(),
                                );
                            } else {
                                ui.label(RichText::new(tr("no errors")).weak());
                            }
                        });
                    }
                    ui.end_row();
                }
            });
        let n: usize = self
            .color
            .colours()
            .iter()
            .map(|&w| report.color(w).games.len())
            .sum();
        if let Period::Months(_) = report.period {
            let out = report.count(report.period, self.color.side()).undated_out;
            if out > 0 {
                ui.label(
                    RichText::new(trf(
                        "Without a date, not counted: {n} (only with all games)",
                        &[("n", &grouped(out))],
                    ))
                    .weak(),
                );
            }
        }
        if n > 0 && n < 10 {
            ui.label(
                RichText::new(trf(
                    "Few games ({n}): percentages and scores are unreliable",
                    &[("n", &n)],
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
                match s
                    .ply
                    .checked_sub(1)
                    .and_then(|p| s.sans.get(p).map(|san| short_label(p, san)))
                {
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
        let (pos, last_move, arrows): (&Chess, Option<(Square, Square)>, Vec<Arrow>) =
            match &self.shown_pos {
                Some(s) => {
                    let pos = &s.positions[s.ply];
                    let last = s
                        .ply
                        .checked_sub(1)
                        .and_then(|p| s.ucis.get(p))
                        .and_then(|u| u.parse::<UciMove>().ok())
                        .and_then(|u| u.to_move(&s.positions[s.ply - 1]).ok())
                        .and_then(|m| Some((m.from()?, crate::board::ui_target(&m))));
                    let mut arrows = if s.ply + 1 == s.positions.len() {
                        s.arrows.clone()
                    } else {
                        Vec::new()
                    };
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
            orientation: if self.board_white() {
                Color::White
            } else {
                Color::Black
            },
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
            ui.label(
                RichText::new(tr(
                    "Choose a move in the tree, an opening or an error: its position is shown here",
                ))
                .weak(),
            );
            return;
        };
        let mut goto = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if s.sans.is_empty() {
                ui.label(RichText::new(tr("Start position")).weak());
            }
            for (i, san) in s.sans.iter().enumerate() {
                // Numbers at White's moves only: «1.e4 c5 2.Nf3 d6».
                let label = if i.is_multiple_of(2) {
                    short_label(i, san)
                } else {
                    crate::san::show(san).into_owned()
                };
                let current = i + 1 == s.ply;
                let text = if current {
                    RichText::new(label).strong()
                } else {
                    RichText::new(label)
                };
                if ui
                    .add(egui::Label::new(text).sense(Sense::click()))
                    .clicked()
                {
                    goto = Some(i + 1);
                }
            }
        });
        if let Some(o) = s.opening.and_then(|o| report.openings.get(o)) {
            ui.label(
                RichText::new(format!("{} · {}", o.name, o.eco))
                    .weak()
                    .small(),
            );
        }
        if let Some(note) = &s.note {
            ui.label(RichText::new(note).small());
        }
        let (sans, ucis, pos, white, ply) = (
            s.sans.clone(),
            s.ucis.clone(),
            s.positions[s.ply].clone(),
            s.white,
            s.ply,
        );
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
                    let last = moves
                        .last()
                        .and_then(|m| Some((m.from()?, crate::board::ui_target(m))));
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
                if ui
                    .button(tr("Find this position in a database (Ctrl+Shift+F)"))
                    .clicked()
                {
                    let mut p = Chess::default();
                    let moves: Vec<Move> = ucis[..ply]
                        .iter()
                        .filter_map(|u| {
                            let m = u.parse::<UciMove>().ok()?.to_move(&p).ok()?;
                            p.play_unchecked(m);
                            Some(m)
                        })
                        .collect();
                    actions.push(PrepAction::FindPosition(
                        self.path.clone(),
                        Chess::default(),
                        moves,
                        !self.board_white(),
                    ));
                    ui.close();
                }
                if ui.button(tr("Copy FEN")).clicked() {
                    let fen =
                        shakmaty::fen::Fen::from_position(&pos, shakmaty::EnPassantMode::Legal)
                            .to_string();
                    actions.push(PrepAction::CopyText(fen, tr("FEN copied")));
                    ui.close();
                }
                if ui.button(tr("Copy moves (PGN)")).clicked() {
                    actions.push(PrepAction::CopyText(
                        self.line_pgn(&sans[..ply], white),
                        tr("PGN copied"),
                    ));
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
        let (w, b) = if white {
            (name, "?".to_owned())
        } else {
            ("?".to_owned(), name)
        };
        let mut moves = String::new();
        for (i, san) in sans.iter().enumerate() {
            if i.is_multiple_of(2) {
                moves.push_str(&format!("{}. ", i / 2 + 1));
            }
            moves.push_str(san);
            moves.push(' ');
        }
        format!(
            "[Event \"{}\"]\n[White \"{w}\"]\n[Black \"{b}\"]\n[Result \"*\"]\n\n{moves}*\n",
            self.title()
        )
    }

    /// In a low window: the board over the right bottom of the screen.
    fn board_overlay(
        &mut self,
        ctx: &egui::Context,
        settings: &Settings,
        actions: &mut Vec<PrepAction>,
    ) {
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
                            actions.push(PrepAction::OpenPgn(
                                self.line_pgn(&s.sans[..s.ply], s.white),
                            ));
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
    fn right_column(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<PrepAction>,
    ) {
        let height = ui.available_height();
        let width = ui.available_width();
        // In a low window the tabs keep the most: two rows of games at least, a quarter.
        let (rows, share) = if self.low_window {
            (2.0, 0.25)
        } else {
            (4.0, 0.35)
        };
        let least = rows * ROW_H + 60.0;
        let most = (height - 160.0).max(least);
        let games_h = self
            .games_height
            .unwrap_or(height * share)
            .clamp(least, most)
            .floor();
        let tabs_h = (height - games_h - 10.0).max(0.0);
        ui.allocate_ui_with_layout(
            vec2(width, tabs_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_height(tabs_h);
                ui.set_max_height(tabs_h);
                self.tabs(ui, settings, actions);
            },
        );
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
        ui.allocate_ui_with_layout(
            vec2(width, games_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_height(games_h);
                ui.set_max_height(games_h);
                self.games_list(ui, actions);
            },
        );
    }

    fn tabs(&mut self, ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<PrepAction>) {
        {
            ui.horizontal(|ui| {
                for (tab, label, key) in [
                    (Tab::Repertoire, tr("Repertoire"), "Ctrl+1"),
                    (Tab::Openings, tr("Openings"), "Ctrl+2"),
                    (Tab::Errors, tr("Errors"), "Ctrl+3"),
                ] {
                    ui.selectable_value(&mut self.tab, tab, label)
                        .on_hover_text(key);
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
                                    ui.selectable_value(
                                        &mut n,
                                        k,
                                        trf("Branches from {n} games", &[("n", &k)]),
                                    );
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
                let mut children = n.children.clone();
                let sort = self.sorts.tree;
                let val = |c: u32| -> SortVal {
                    let m = &tree.nodes[c as usize];
                    match sort.key {
                        TreeSort::Games => num(tree.position_tally(m.key).games),
                        TreeSort::Share => num(m.tally.games),
                        TreeSort::Score => score_val(&tree.position_tally(m.key)),
                        TreeSort::Last => num(chess_import::pgn_catalog::date_number(
                            tree.position_last_date(m.key),
                        )),
                    }
                };
                // Ties: most played first, as the tree has them.
                children.sort_by(|&a, &b| sort.apply(cmp_val(&val(a), &val(b))));
                let (main, rest): (Vec<u32>, Vec<u32>) = children
                    .into_iter()
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
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let has_book = self.book_ok() || (self.book_job.is_some() && self.book != BookChoice::None);
        let mut used = 180.0 + 76.0 + 48.0 + 50.0 + 4.0 * gap;
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
        let book_title = if self.book == BookChoice::Same {
            tr("In database")
        } else {
            tr("Book")
        };
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
        table = table
            .column(Column::exact(76.0))
            .column(Column::exact(48.0));
        table = table.column(Column::exact(if show_bar { 110.0 } else { 50.0 }));
        if show_last {
            table = table.column(Column::exact(84.0));
        }
        if show_book {
            table = table.column(Column::exact(80.0));
        }
        let selected_row = rows.iter().position(|r| match (r, &self.selection) {
            (
                TreeRow::Node { white, node, .. },
                Selection::Node {
                    white: sw,
                    node: sn,
                },
            ) => white == sw && node == sn,
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
        let threshold = report
            .book
            .as_ref()
            .map_or(prep::DEFAULT_BOOK_THRESHOLD, |b| b.threshold);
        let mut tree_click = None;
        table
            .header(ROW_H, |mut h| {
                head_col(&mut h, tr("Move"), tr("His moves plain, the opponents' weak; ⇄ — the position is reached by another move order too"));
                if show_opening {
                    head_col(&mut h, tr("Opening"), "");
                }
                let sort = self.sorts.tree;
                sort_col(&mut h, tr("Games"), "", TreeSort::Games, sort, &mut tree_click);
                sort_col(&mut h, "%", tr("Share of the games of the move above"), TreeSort::Share, sort, &mut tree_click);
                sort_col(&mut h, tr("Score"), tr("The opponent's points in these games"), TreeSort::Score, sort, &mut tree_click);
                if show_last {
                    sort_col(&mut h, tr("Last"), "", TreeSort::Last, sort, &mut tree_click);
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
                                // Short when the column is narrow at this depth.
                                let full = trf("other moves ({n})", &[("n", &count)]);
                                let text = if text_width(ui, &full, egui::TextStyle::Body) > ui.available_width() {
                                    trf("others ({n})", &[("n", &count)])
                                } else {
                                    full
                                };
                                ui.label(RichText::new(text).weak());
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
                            // The statistics are the position's, whatever the move order.
                            let tally = cr.tree.position_tally(n.key);
                            let few = tally.games < FEW_GAMES;
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
                                if !show_opening
                                    && let Some(o) = n.opening.filter(|_| n.named).and_then(|o| report.openings.get(o))
                                {
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
                                    && self.visible(m.judgement)
                                {
                                    let best = crate::san::show(&m.best_san).into_owned();
                                    pill(ui, m.judgement).on_hover_text(trf(
                                        "{judgement}: better {best} ({before} → {after}) · in {n} games",
                                        &[
                                            ("judgement", &review::judgement_name(m.judgement)),
                                            ("best", &best),
                                            ("before", &m.before.format()),
                                            ("after", &m.after.format()),
                                            ("n", &m.games),
                                        ],
                                    ));
                                    if m.games > 1 {
                                        ui.label(RichText::new(format!("×{}", m.games)).weak().small());
                                    }
                                }
                            });
                            if show_opening {
                                row.col(|ui| {
                                    // The name where it changes (`named`); the ECO keeps
                                    // its place at the right, the name takes the rest.
                                    if n.named && let Some(o) = n.opening.and_then(|o| report.openings.get(o)) {
                                        let eco = RichText::new(&o.eco).weak().small();
                                        let eco_w = text_width(ui, &o.eco, egui::TextStyle::Small) + ui.spacing().item_spacing.x;
                                        let name_w = (ui.available_width() - eco_w).max(0.0);
                                        ui.allocate_ui(vec2(name_w, ROW_H), |ui| {
                                            ui.add(egui::Label::new(RichText::new(&o.name).weak()).truncate())
                                                .on_hover_text(format!("{} · {}", o.name, o.eco));
                                        });
                                        ui.label(eco);
                                    }
                                });
                            }
                            row.col(|ui| {
                                ui.label(tally.games.to_string());
                            });
                            row.col(|ui| {
                                // The move's share where the games were (this order).
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
                                let t = RichText::new(score_text(&tally));
                                let l = ui.label(if few { t.weak() } else { t });
                                l.on_hover_text(tally_hint(&tally));
                                if show_bar {
                                    score_bar(ui, &tally, white, few);
                                }
                            });
                            if show_last {
                                row.col(|ui| {
                                    let d = crate::library::display_date(cr.tree.position_last_date(n.key));
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
        if let Some(k) = tree_click {
            self.sorts.tree.click(k, false);
        }
        // A child of the chosen node under the pointer: its move as an arrow.
        if let (
            Some((white, node)),
            Selection::Node {
                white: sw,
                node: sn,
            },
        ) = (hovered, &self.selection)
            && white == *sw
        {
            let cr = report.color(white);
            let n = &cr.tree.nodes[node as usize];
            if n.parent == Some(*sn)
                && let Some(s) = &self.shown_pos
                && s.ply + 1 == s.positions.len()
                && let Some(m) = n
                    .uci
                    .parse::<UciMove>()
                    .ok()
                    .and_then(|u| u.to_move(&s.positions[s.ply]).ok())
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
        // No game in the period at all: the way back to all of them.
        let side = self.color.side();
        if report.period != Period::All && report.count(report.period, side).games == 0 {
            let all = report.count(Period::All, side).games;
            ui.add_space(12.0);
            ui.label(match self.color {
                ColorChoice::White => tr("No games as White in this period"),
                ColorChoice::Black => tr("No games as Black in this period"),
                ColorChoice::Both => tr("No games in this period"),
            });
            if all > 0
                && ui
                    .button(format!("{} · {}", period_label(Period::All), grouped(all)))
                    .clicked()
            {
                self.want_period = Some(Period::All);
            }
            return;
        }
        let (shown, other) = match self.color {
            ColorChoice::White => (true, false),
            ColorChoice::Black => (false, true),
            ColorChoice::Both => return,
        };
        if !report.color(shown).games.is_empty() {
            return;
        }
        let other_n = report.color(other).games.len();
        let (colour, other_name) = if shown {
            (tr("White"), tr("Black"))
        } else {
            (tr("Black"), tr("White"))
        };
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(trf(
                "No games as {colour}. As {other} — {n}",
                &[("colour", &colour), ("other", &other_name), ("n", &other_n)],
            ));
            if other_n > 0
                && ui
                    .link(trf("Show {other}", &[("other", &other_name)]))
                    .clicked()
            {
                self.set_color(ColorChoice::from_side(if other {
                    Side::White
                } else {
                    Side::Black
                }));
            }
        });
    }

    // ----- the openings --------------------------------------------------------------

    /// The value of a family (`v` None) or a variation for the openings' order.
    fn opening_val(&self, white: bool, f: usize, v: Option<usize>, key: OpeningSort) -> SortVal {
        let Some(report) = &self.report else {
            return num(0);
        };
        let cr = report.color(white);
        let fam = &cr.families[f];
        let vars: Vec<&prep::Variation> = match v {
            Some(v) => vec![&fam.variations[v]],
            None => fam.variations.iter().collect(),
        };
        let tally = match v {
            Some(v) => fam.variations[v].tally,
            None => fam.tally,
        };
        let opening = |var: &prep::Variation| var.opening.and_then(|o| report.openings.get(o));
        match key {
            OpeningSort::Name => SortVal::Text(match v {
                Some(_) => vars[0]
                    .opening
                    .map_or_else(String::new, |o| report.openings[o].name.clone()),
                None => fam.name.clone(),
            }),
            OpeningSort::Eco => SortVal::Text(
                vars.iter()
                    .filter_map(|x| opening(x))
                    .map(|o| o.eco.clone())
                    .min()
                    .unwrap_or_default(),
            ),
            OpeningSort::Games | OpeningSort::Share => num(tally.games),
            OpeningSort::Score => score_val(&tally),
            OpeningSort::Last => num(vars
                .iter()
                .flat_map(|x| x.games.iter())
                .map(|&g| report.games[g as usize].date_number())
                .max()
                .unwrap_or(0)),
            OpeningSort::Errors => num(vars
                .iter()
                .flat_map(|x| x.error_list.iter())
                .map(|&e| cr.errors[e as usize].judgement)
                .filter(|j| self.visible(*j))
                .map(judgement_weight)
                .sum::<u32>()),
        }
    }

    /// The rows of the openings table: (white, family, variation) — families in the
    /// chosen order, the variations of an open one under it in the same order.
    fn opening_rows(&self) -> Vec<(bool, usize, Option<usize>)> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let sort = self.sorts.openings;
        let mut rows = Vec::new();
        for white in self.color.colours() {
            let cr = report.color(white);
            let mut fams: Vec<usize> = (0..cr.families.len()).collect();
            fams.sort_by(|&a, &b| {
                sort.apply(cmp_val(
                    &self.opening_val(white, a, None, sort.key),
                    &self.opening_val(white, b, None, sort.key),
                ))
            });
            for f in fams {
                rows.push((white, f, None));
                if self.open_families.contains(&(white, f)) {
                    let mut vars: Vec<usize> = (0..cr.families[f].variations.len()).collect();
                    vars.sort_by(|&a, &b| {
                        sort.apply(cmp_val(
                            &self.opening_val(white, f, Some(a), sort.key),
                            &self.opening_val(white, f, Some(b), sort.key),
                        ))
                    });
                    rows.extend(vars.into_iter().map(|v| (white, f, Some(v))));
                }
            }
        }
        rows
    }

    fn openings_table(&mut self, ui: &mut Ui) {
        let rows = self.opening_rows();
        let Some(report) = self.report.as_ref() else {
            return;
        };
        if rows.is_empty() {
            self.no_games_note(ui);
            return;
        }
        let both = self.color == ColorChoice::Both;
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let mut used = 180.0 + 76.0 + 48.0 + 50.0 + 4.0 * gap + if both { 24.0 + gap } else { 0.0 };
        let mut fits = |w: f32, wanted: bool| {
            let ok = wanted && used + w + gap <= width;
            if ok {
                used += w + gap;
            }
            ok
        };
        // The errors stay as long as possible; the first moves give way first.
        let show_errors = fits(90.0, report.evaluated());
        let show_bar = fits(64.0, true);
        let show_eco = fits(64.0, true);
        let show_last = fits(84.0, true);
        let show_moves = fits(140.0, true);
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("prep_openings")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if both {
            table = table.column(Column::exact(24.0));
        }
        table = table.column(Column::remainder().at_least(180.0).clip(true));
        if show_eco {
            table = table.column(Column::exact(64.0));
        }
        if show_moves {
            table = table.column(Column::remainder().at_least(140.0).clip(true));
        }
        table = table
            .column(Column::exact(76.0))
            .column(Column::exact(48.0));
        table = table.column(Column::exact(if show_bar { 110.0 } else { 50.0 }));
        if show_last {
            table = table.column(Column::exact(84.0));
        }
        if show_errors {
            table = table.column(Column::exact(90.0));
        }
        let selected_row = rows.iter().position(|&(w, f, v)| {
            self.selection
                == Selection::Opening {
                    white: w,
                    family: f,
                    variation: v,
                }
        });
        if self.scroll_to_selected
            && let Some(i) = selected_row
        {
            table = table.scroll_to_row(i, None);
        }
        self.scroll_to_selected = false;
        let mut clicked = None;
        let mut opening_click = None;
        let mut toggled = None;
        table
            .header(ROW_H, |mut h| {
                if both {
                    head_col(&mut h, "", "");
                }
                let sort = self.sorts.openings;
                let c = &mut opening_click;
                sort_col(&mut h, tr("Opening"), "", OpeningSort::Name, sort, c);
                if show_eco {
                    sort_col(&mut h, "ECO", "", OpeningSort::Eco, sort, c);
                }
                if show_moves {
                    head_col(&mut h, tr("First moves"), "");
                }
                sort_col(&mut h, tr("Games"), "", OpeningSort::Games, sort, c);
                sort_col(
                    &mut h,
                    "%",
                    tr("Share of the colour's games; a variation's — of its family's"),
                    OpeningSort::Share,
                    sort,
                    c,
                );
                sort_col(
                    &mut h,
                    tr("Score"),
                    tr("The opponent's points in these games"),
                    OpeningSort::Score,
                    sort,
                    c,
                );
                if show_last {
                    sort_col(&mut h, tr("Last"), "", OpeningSort::Last, sort, c);
                }
                if show_errors {
                    sort_col(&mut h, tr("Errors"), "", OpeningSort::Errors, sort, c);
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
                        None => (
                            &fam.tally,
                            &fam.errors,
                            fam.variations
                                .iter()
                                .flat_map(|v| v.games.iter().copied())
                                .collect(),
                        ),
                    };
                    let few = tally.games < FEW_GAMES;
                    if both {
                        row.col(|ui| {
                            if v.is_none() {
                                ui.label(RichText::new(if white { "♙" } else { "♟" }).weak())
                                    .on_hover_text(if white {
                                        tr("As White")
                                    } else {
                                        tr("As Black")
                                    });
                            }
                        });
                    }
                    row.col(|ui| match v {
                        None => {
                            let open = self.open_families.contains(&(white, f));
                            let glyph = if open { "▾" } else { "▸" };
                            if ui
                                .add(
                                    egui::Label::new(RichText::new(glyph).weak())
                                        .sense(Sense::click()),
                                )
                                .clicked()
                            {
                                toggled = Some((white, f));
                            }
                            let name = if fam.name.is_empty() {
                                tr("no name").to_owned()
                            } else {
                                fam.name.clone()
                            };
                            ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                        }
                        Some(v) => {
                            ui.add_space(18.0);
                            let name = fam.variations[v]
                                .opening
                                .and_then(|o| report.openings.get(o))
                                .map(|o| {
                                    o.variation().map_or_else(|| o.name.clone(), str::to_owned)
                                });
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
                                Some(v) => fam.variations[v]
                                    .opening
                                    .and_then(|o| report.openings.get(o))
                                    .map(|o| o.eco.clone()),
                                None => {
                                    let mut ecos: Vec<&str> = fam
                                        .variations
                                        .iter()
                                        .filter_map(|v| {
                                            v.opening.and_then(|o| report.openings.get(o))
                                        })
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
                                Some(v) => fam.variations[v]
                                    .opening
                                    .and_then(|o| report.openings.get(o))
                                    .map(|o| o.line.clone()),
                                None => family_line(report, fam),
                            };
                            if let Some(l) = line {
                                let tokens = table_line(&l);
                                let full = crate::san::show_line(&tokens.join(" "));
                                let shown = fit_line(ui, &tokens, ui.available_width());
                                ui.label(RichText::new(shown).weak()).on_hover_text(full);
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
                        ui.label(if few { t.weak() } else { t })
                            .on_hover_text(tally_hint(tally));
                        if show_bar {
                            score_bar(ui, tally, white, few);
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
                            ui.label(
                                RichText::new(if last.is_empty() {
                                    "—".to_owned()
                                } else {
                                    last
                                })
                                .weak(),
                            );
                        });
                    }
                    if show_errors {
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            counts_text(ui, &self.shown_counts(errors));
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
        if let Some(k) = opening_click {
            self.sorts
                .openings
                .click(k, matches!(k, OpeningSort::Name | OpeningSort::Eco));
        }
        if let Some(key) = toggled {
            if !self.open_families.remove(&key) {
                self.open_families.insert(key);
            }
        } else if let Some((white, family, variation)) = clicked {
            self.focus_games = false;
            self.select(Selection::Opening {
                white,
                family,
                variation,
            });
        }
    }

    // ----- the errors ----------------------------------------------------------------

    fn errors_tab(&mut self, ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<PrepAction>) {
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let moves = self.plies.div_ceil(2);
        let state = self.engine_state();
        if state == EngineState::NotRun {
            // The invitation (§5).
            let (positions, secs) = self.estimate();
            let games: usize = self
                .color
                .colours()
                .iter()
                .map(|&w| report.color(w).games.len())
                .sum();
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
                        ("positions", &grouped(positions)),
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
        let mut ask_engine = false;
        let mut inaccuracies = settings.prep.inaccuracies;
        // The state line: how far the engine got; the switch of the inaccuracies.
        ui.horizontal_wrapped(|ui| {
            match state {
                EngineState::Running => {
                    if let Some(job) = &self.job {
                        let secs = self.secs_for(job.total.saturating_sub(job.done), job.processes);
                        ui.label(
                            RichText::new(trf(
                                "Checked {done} of {total} positions",
                                &[("done", &grouped(job.done)), ("total", &grouped(job.total))],
                            ))
                            .weak(),
                        );
                        ui.label(
                            RichText::new(format!("· ≈ {}", review::format_duration(secs))).weak(),
                        );
                    }
                }
                EngineState::Partial { done, total } => {
                    let (_, secs) = self.estimate();
                    ui.label(
                        RichText::new(trf(
                            "Checked {done} of {total} positions",
                            &[("done", &grouped(done)), ("total", &grouped(total))],
                        ))
                        .weak(),
                    )
                    .on_hover_text(tr("The counts are of the checked positions"));
                    ui.label(RichText::new("·").weak());
                    if ui
                        .link(trf(
                            "Continue ≈ {time}",
                            &[("time", &review::format_duration(secs))],
                        ))
                        .clicked()
                    {
                        ask_engine = true;
                    }
                }
                EngineState::Complete { .. } => {
                    if let Some(status) = self.engine_status() {
                        ui.label(RichText::new(status).weak());
                    }
                }
                EngineState::NotRun => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut inaccuracies, tr("Inaccuracies"))
                    .on_hover_text(tr(
                        "Show the inaccuracies (?!) with the mistakes and blunders",
                    ));
            });
        });
        if inaccuracies != settings.prep.inaccuracies {
            settings.prep.inaccuracies = inaccuracies;
            self.show_inaccuracies = inaccuracies;
            self.rebuild_marks();
            actions.push(PrepAction::SettingsChanged);
        }
        if ask_engine {
            self.engine_asked(settings, ui.ctx(), actions);
            return;
        }
        let Some(report) = self.report.as_ref() else {
            return;
        };
        // The lines per colour: in the stack only (beside the board the summary has them).
        if self.stacked {
            for white in self.color.colours() {
                let cr = report.color(white);
                let with: HashSet<u32> = cr
                    .errors
                    .iter()
                    .filter(|e| self.visible(e.judgement))
                    .map(|e| e.game)
                    .collect();
                let p = 100 * with.len() / cr.games.len().max(1);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new(if white {
                            tr("As White")
                        } else {
                            tr("As Black")
                        })
                        .strong(),
                    );
                    counts_text(ui, &self.shown_counts(&cr.counts));
                    ui.label(RichText::new(trf("in {p} % of games", &[("p", &p)])).weak());
                });
            }
        }
        let serious: u32 = self
            .color
            .colours()
            .iter()
            .map(|&w| {
                let c = &report.color(w).counts;
                c.mistakes + c.blunders
            })
            .sum();
        let inacc: u32 = self
            .color
            .colours()
            .iter()
            .map(|&w| report.color(w).counts.inaccuracies)
            .sum();
        if matches!(state, EngineState::Complete { .. }) && serious == 0 {
            let mut deeper = false;
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(trf(
                    "No errors in the first {n} moves ({i} inaccuracies).",
                    &[("n", &moves), ("i", &inacc)],
                ));
                if self.plies < 30 && ui.link(trf("Deeper: {n} moves", &[("n", &15)])).clicked() {
                    deeper = true;
                }
            });
            if deeper {
                // As if chosen in the depth list.
                self.set_plies(30, settings, ui.ctx());
                actions.push(PrepAction::SettingsChanged);
                return;
            }
        }
        ui.add_space(4.0);
        self.errors_table(ui, actions);
    }

    /// The repeated errors (the same position and move in several games) and the
    /// errors by opening (the variations, the worst first), the inaccuracies left out
    /// when they are hidden.
    fn repeated_shown(&self) -> Vec<(bool, usize)> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut list: Vec<(bool, usize)> = self
            .color
            .colours()
            .into_iter()
            .flat_map(|w| {
                report
                    .color(w)
                    .repeated
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| self.visible(r.judgement))
                    .map(move |(i, _)| (w, i))
            })
            .collect();
        let sort = self.sorts.repeated;
        let val = |&(w, i): &(bool, usize)| -> SortVal {
            let r = &report.color(w).repeated[i];
            match sort.key {
                RepeatedSort::Move => num(r.ply),
                RepeatedSort::Times => num(r.games.len() as u32),
                RepeatedSort::Score => score_val(&r.tally),
                RepeatedSort::Opening => SortVal::Text(
                    r.opening
                        .map_or_else(String::new, |o| report.openings[o].name.clone()),
                ),
            }
        };
        // Ties: the worst first.
        list.sort_by(|a, b| {
            let (ra, rb) = (
                &report.color(a.0).repeated[a.1],
                &report.color(b.0).repeated[b.1],
            );
            sort.apply(cmp_val(&val(a), &val(b)))
                .then(judgement_weight(rb.judgement).cmp(&judgement_weight(ra.judgement)))
        });
        list
    }

    /// The games of a repeated error, the newest first.
    fn repeated_games(&self, white: bool, index: usize) -> Vec<u32> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut games = report.color(white).repeated[index].games.clone();
        games.sort_by_key(|&g| std::cmp::Reverse(report.games[g as usize].date_number()));
        games
    }

    fn err_rows(&self) -> Vec<ErrRow> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut rows = vec![ErrRow::RepeatedTitle];
        for (white, index) in self.repeated_shown() {
            rows.push(ErrRow::Repeated { white, index });
            if self.open_repeated.contains(&(white, index)) {
                for game in self.repeated_games(white, index) {
                    rows.push(ErrRow::RepeatedGame { white, index, game });
                }
            }
        }
        rows.push(ErrRow::OpeningsTitle);
        // (white, family, variation, weight)
        let mut vars: Vec<(bool, usize, usize, u32)> = Vec::new();
        for white in self.color.colours() {
            let cr = report.color(white);
            for (f, fam) in cr.families.iter().enumerate() {
                for (v, var) in fam.variations.iter().enumerate() {
                    let weight: u32 = var
                        .error_list
                        .iter()
                        .map(|&e| cr.errors[e as usize].judgement)
                        .filter(|j| self.visible(*j))
                        .map(judgement_weight)
                        .sum();
                    if weight > 0 {
                        vars.push((white, f, v, weight));
                    }
                }
            }
        }
        let sort = self.sorts.variations;
        let val = |v: &(bool, usize, usize, u32)| -> u32 {
            match sort.key {
                VariationSort::Games => {
                    report.color(v.0).families[v.1].variations[v.2].games.len() as u32
                }
                VariationSort::Errors => v.3,
            }
        };
        vars.sort_by(|a, b| sort.apply(val(a).cmp(&val(b))).then(b.3.cmp(&a.3)));
        for (white, family, variation, _) in vars {
            rows.push(ErrRow::Variation {
                white,
                family,
                variation,
            });
            if self.open_variations.contains(&(white, family, variation)) {
                let cr = report.color(white);
                for &e in &cr.families[family].variations[variation].error_list {
                    if self.visible(cr.errors[e as usize].judgement) {
                        rows.push(ErrRow::Error {
                            white,
                            index: e as usize,
                        });
                    }
                }
            }
        }
        rows
    }

    /// The error of a repeated error in one of its games.
    fn error_in(&self, white: bool, index: usize, game: u32) -> Option<usize> {
        let cr = self.report.as_ref()?.color(white);
        let r = cr.repeated.get(index)?;
        cr.errors
            .iter()
            .position(|e| e.game == game && e.key == r.key && e.uci == r.uci)
    }

    /// What a row of the errors tab chooses.
    fn err_select(&mut self, row: ErrRow) {
        self.err_cursor = Some(row);
        self.focus_games = false;
        let sel = match row {
            ErrRow::Repeated { white, index } => Selection::Repeated { white, index },
            ErrRow::RepeatedGame { white, index, game } => {
                match self.error_in(white, index, game) {
                    Some(e) => Selection::Error { white, index: e },
                    None => return,
                }
            }
            ErrRow::Variation {
                white,
                family,
                variation,
            } => Selection::Opening {
                white,
                family,
                variation: Some(variation),
            },
            ErrRow::Error { white, index } => Selection::Error { white, index },
            ErrRow::RepeatedTitle | ErrRow::OpeningsTitle => return,
        };
        self.select(sel);
    }

    /// The game a row opens, at the move after the error: (record, ply). A repeated
    /// error opens its newest game.
    fn err_open(&self, row: ErrRow) -> Option<(usize, usize)> {
        let report = self.report.as_ref()?;
        let (white, e) = match row {
            ErrRow::Repeated { white, index } => {
                let game = *self.repeated_games(white, index).first()?;
                (white, self.error_in(white, index, game)?)
            }
            ErrRow::RepeatedGame { white, index, game } => {
                (white, self.error_in(white, index, game)?)
            }
            ErrRow::Error { white, index } => (white, index),
            _ => return None,
        };
        let e = report.color(white).errors.get(e)?;
        Some((
            report.games[e.game as usize].index as usize,
            e.ply as usize + 1,
        ))
    }

    fn err_toggle(&mut self, row: ErrRow) {
        match row {
            ErrRow::Repeated { white, index } => {
                if !self.open_repeated.remove(&(white, index)) {
                    self.open_repeated.insert((white, index));
                }
            }
            ErrRow::Variation {
                white,
                family,
                variation,
            } => {
                let k = (white, family, variation);
                if !self.open_variations.remove(&k) {
                    self.open_variations.insert(k);
                }
            }
            _ => {}
        }
    }

    /// The errors tab as one table: the repeated errors, then by opening; a click
    /// chooses, a double click (Enter) opens the game at the error.
    fn errors_table(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        let rows = self.err_rows();
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let both = self.color == ColorChoice::Both;
        let width = ui.available_width();
        let show_last = width > 640.0;
        let cursor = self
            .err_cursor
            .and_then(|c| rows.iter().position(|r| *r == c));
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("prep_errors")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::exact(110.0))
            .column(Column::exact(80.0))
            .column(Column::exact(70.0))
            .column(Column::exact(120.0).clip(true));
        if show_last {
            table = table.column(Column::exact((width * 0.24).clamp(120.0, 260.0)).clip(true));
        }
        if self.scroll_to_selected
            && let Some(i) = cursor
        {
            table = table.scroll_to_row(i, None);
        }
        self.scroll_to_selected = false;
        let mut clicked = None;
        let mut toggled = None;
        let mut opened = None;
        let colour_icon = |ui: &mut Ui, white: bool| {
            if both {
                ui.label(RichText::new(if white { "♙" } else { "♟" }).weak())
                    .on_hover_text(if white {
                        tr("As White")
                    } else {
                        tr("As Black")
                    });
            }
        };
        let repeated_n = rows
            .iter()
            .filter(|r| matches!(r, ErrRow::Repeated { .. }))
            .count();
        let sorts = self.sorts.clone();
        let mut sort_click: Option<SortKeyOf> = None;
        let openings_n = rows
            .iter()
            .filter(|r| matches!(r, ErrRow::Variation { .. }))
            .count();
        table.body(|body| {
            body.rows(ROW_H, rows.len(), |mut row| {
                let r = rows[row.index()];
                row.set_selected(Some(row.index()) == cursor);
                let weak = |t: &str| RichText::new(t).weak();
                match r {
                    ErrRow::RepeatedTitle | ErrRow::OpeningsTitle => {
                        let repeated = r == ErrRow::RepeatedTitle;
                        let (title, empty) = if repeated {
                            (
                                tr("Repeated errors — the same position, the same move"),
                                repeated_n == 0,
                            )
                        } else {
                            (tr("By opening"), openings_n == 0)
                        };
                        row.col(|ui| {
                            ui.label(RichText::new(title).strong());
                            if empty {
                                ui.label(weak(tr("none")));
                            }
                        });
                        // The captions of the part's columns; those it can be ordered by
                        // are sort buttons.
                        let captions: [(&str, Option<SortKeyOf>); 5] = if repeated {
                            [
                                (tr("Played"), Some(SortKeyOf::Repeated(RepeatedSort::Move))),
                                (tr("Better"), None),
                                ("×", Some(SortKeyOf::Repeated(RepeatedSort::Times))),
                                (tr("Score"), Some(SortKeyOf::Repeated(RepeatedSort::Score))),
                                (
                                    tr("Opening"),
                                    Some(SortKeyOf::Repeated(RepeatedSort::Opening)),
                                ),
                            ]
                        } else {
                            [
                                (tr("Played"), None),
                                (tr("Better"), None),
                                (
                                    tr("Games"),
                                    Some(SortKeyOf::Variation(VariationSort::Games)),
                                ),
                                (
                                    tr("Errors"),
                                    Some(SortKeyOf::Variation(VariationSort::Errors)),
                                ),
                                (tr("Date"), None),
                            ]
                        };
                        for (k, (c, key)) in captions.iter().enumerate() {
                            if k < 4 || show_last {
                                row.col(|ui| match key {
                                    Some(key) => {
                                        let active = match key {
                                            SortKeyOf::Repeated(k) => (sorts.repeated.key == *k)
                                                .then_some(sorts.repeated.ascending),
                                            SortKeyOf::Variation(k) => (sorts.variations.key == *k)
                                                .then_some(sorts.variations.ascending),
                                        };
                                        if crate::widgets::sort_header(ui, c, active).clicked() {
                                            sort_click = Some(*key);
                                        }
                                    }
                                    None => {
                                        ui.label(weak(c));
                                    }
                                });
                            }
                        }
                    }
                    ErrRow::Repeated { white, index } => {
                        let cr = report.color(white);
                        let rep = &cr.repeated[index];
                        let g = &report.games[rep.games[0] as usize];
                        let open = self.open_repeated.contains(&(white, index));
                        row.col(|ui| {
                            if ui
                                .add(
                                    egui::Label::new(weak(if open { "▾" } else { "▸" }))
                                        .sense(Sense::click()),
                                )
                                .clicked()
                            {
                                toggled = Some(r);
                            }
                            colour_icon(ui, white);
                            let before = (rep.ply as usize)
                                .checked_sub(1)
                                .map(|p| short_label(p, &g.sans[p]))
                                .map_or_else(
                                    || tr("start").to_owned(),
                                    |m| trf("after {move}", &[("move", &m)]),
                                );
                            let side = if rep.ply.is_multiple_of(2) {
                                tr("White to move")
                            } else {
                                tr("Black to move")
                            };
                            ui.add(egui::Label::new(format!("{before} · {side}")).truncate());
                        });
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(short_label(rep.ply as usize, &rep.san));
                            pill(ui, rep.judgement)
                                .on_hover_text(review::judgement_name(rep.judgement));
                        });
                        row.col(|ui| {
                            ui.label(short_label(rep.ply as usize, &rep.best_san));
                        });
                        row.col(|ui| {
                            ui.label(rep.times().to_string());
                        });
                        row.col(|ui| {
                            ui.label(weak(&tally_hint(&rep.tally)));
                        });
                        if show_last {
                            row.col(|ui| {
                                let name = rep
                                    .opening
                                    .and_then(|o| report.openings.get(o))
                                    .map(|o| format!("{} · {}", o.name, o.eco))
                                    .unwrap_or_default();
                                ui.add(egui::Label::new(weak(&name)).truncate())
                                    .on_hover_text(name);
                            });
                        }
                    }
                    ErrRow::RepeatedGame { white, index, game } => {
                        let g = &report.games[game as usize];
                        let lr = &self.rows[game as usize];
                        let _ = (white, index);
                        row.col(|ui| {
                            ui.add_space(28.0);
                            ui.add(
                                egui::Label::new(format!("{} — {}", lr.white, lr.black)).truncate(),
                            );
                        });
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|ui| {
                            ui.label(result_text(lr.result));
                        });
                        if show_last {
                            row.col(|ui| {
                                ui.label(weak(&crate::library::display_date(&g.date)));
                            });
                        }
                    }
                    ErrRow::Variation {
                        white,
                        family,
                        variation,
                    } => {
                        let cr = report.color(white);
                        let var = &cr.families[family].variations[variation];
                        let open = self.open_variations.contains(&(white, family, variation));
                        let mut counts = ErrorCounts::default();
                        for &e in &var.error_list {
                            counts.add(cr.errors[e as usize].judgement);
                        }
                        row.col(|ui| {
                            if ui
                                .add(
                                    egui::Label::new(weak(if open { "▾" } else { "▸" }))
                                        .sense(Sense::click()),
                                )
                                .clicked()
                            {
                                toggled = Some(r);
                            }
                            colour_icon(ui, white);
                            let name = var
                                .opening
                                .and_then(|o| report.openings.get(o))
                                .map_or_else(
                                    || tr("no name").to_owned(),
                                    |o| format!("{} · {}", o.name, o.eco),
                                );
                            ui.add(egui::Label::new(name.as_str()).truncate())
                                .on_hover_text(name);
                        });
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|ui| {
                            ui.label(weak(&var.games.len().to_string()));
                        });
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            counts_text(ui, &self.shown_counts(&counts));
                        });
                        if show_last {
                            row.col(|_| {});
                        }
                    }
                    ErrRow::Error { white, index } => {
                        let e = &report.color(white).errors[index];
                        let g = &report.games[e.game as usize];
                        let lr = &self.rows[e.game as usize];
                        row.col(|ui| {
                            ui.add_space(28.0);
                            ui.add(
                                egui::Label::new(format!("{} — {}", lr.white, lr.black)).truncate(),
                            );
                        });
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(short_label(e.ply as usize, &e.san));
                            pill(ui, e.judgement).on_hover_text(format!(
                                "{} · {} → {}",
                                review::judgement_name(e.judgement),
                                e.before.format(),
                                e.after.format()
                            ));
                        });
                        row.col(|ui| {
                            ui.label(short_label(e.ply as usize, &e.best_san));
                        });
                        row.col(|_| {});
                        row.col(|ui| {
                            ui.label(result_text(lr.result));
                        });
                        if show_last {
                            row.col(|ui| {
                                ui.label(weak(&crate::library::display_date(&g.date)));
                            });
                        }
                    }
                }
                if !r.is_title() {
                    let resp = row.response();
                    if resp.double_clicked() {
                        match r {
                            ErrRow::Repeated { .. } | ErrRow::Variation { .. } => toggled = Some(r),
                            _ => opened = Some(r),
                        }
                        clicked = Some(r);
                    } else if resp.clicked() {
                        clicked = Some(r);
                    }
                }
            });
        });
        match sort_click {
            Some(SortKeyOf::Repeated(k)) => {
                // Indices into the lists do not change: the rows only reorder.
                self.sorts
                    .repeated
                    .click(k, matches!(k, RepeatedSort::Move | RepeatedSort::Opening))
            }
            Some(SortKeyOf::Variation(k)) => self.sorts.variations.click(k, false),
            None => {}
        }
        if let Some(r) = toggled {
            self.err_toggle(r);
        }
        if let Some(r) = clicked {
            self.err_select(r);
        }
        if let Some((index, ply)) = opened.and_then(|r| self.err_open(r)) {
            actions.push(PrepAction::OpenGame {
                path: self.path.clone(),
                index,
                ply,
            });
        }
    }

    /// The keyboard in the errors tab: ↑/↓ the rows, →/← open and close, Enter the game.
    #[allow(clippy::too_many_arguments)]
    fn errors_keys(
        &mut self,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        enter: bool,
        space: bool,
        home: bool,
        end: bool,
        actions: &mut Vec<PrepAction>,
    ) {
        let rows: Vec<ErrRow> = self
            .err_rows()
            .into_iter()
            .filter(|r| !r.is_title())
            .collect();
        if rows.is_empty() {
            return;
        }
        let at = self
            .err_cursor
            .and_then(|c| rows.iter().position(|r| *r == c));
        let Some(at) = at else {
            if up || down || home || end {
                self.err_select(rows[0]);
                self.scroll_to_selected = true;
            }
            return;
        };
        let r = rows[at];
        let next = if down {
            Some((at + 1).min(rows.len() - 1))
        } else if up {
            Some(at.saturating_sub(1))
        } else if home {
            Some(0)
        } else if end {
            Some(rows.len() - 1)
        } else {
            None
        };
        if let Some(i) = next {
            self.err_select(rows[i]);
            self.scroll_to_selected = true;
            return;
        }
        let open = match r {
            ErrRow::Repeated { white, index } => Some(self.open_repeated.contains(&(white, index))),
            ErrRow::Variation {
                white,
                family,
                variation,
            } => Some(self.open_variations.contains(&(white, family, variation))),
            _ => None,
        };
        if (right && open == Some(false))
            || (left && open == Some(true))
            || (space && open.is_some())
        {
            self.err_toggle(r);
        } else if left {
            // A game or an error row: to the row it is under.
            if let Some(parent) = rows[..at]
                .iter()
                .rev()
                .find(|p| matches!(p, ErrRow::Repeated { .. } | ErrRow::Variation { .. }))
            {
                self.err_select(*parent);
                self.scroll_to_selected = true;
            }
        } else if enter {
            match self.err_open(r) {
                Some((index, ply)) => actions.push(PrepAction::OpenGame {
                    path: self.path.clone(),
                    index,
                    ply,
                }),
                None => {
                    self.focus_games = true;
                    self.games_cursor = Some(0);
                }
            }
        }
    }

    // ----- the games list ------------------------------------------------------------

    fn games_list(&mut self, ui: &mut Ui, actions: &mut Vec<PrepAction>) {
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let list = self.games_of();
        let title = match &self.selection {
            Selection::None => trf("Games · {n}", &[("n", &list.len())]),
            Selection::Node { white, node } => {
                let n = &report.color(*white).tree.nodes[*node as usize];
                match n.ply {
                    0 => trf("Games · {n}", &[("n", &list.len())]),
                    p => trf(
                        "Games · after {move} · {n}",
                        &[
                            ("move", &short_label(usize::from(p) - 1, &n.san)),
                            ("n", &list.len()),
                        ],
                    ),
                }
            }
            Selection::Opening {
                white,
                family,
                variation,
            } => {
                let fam = &report.color(*white).families[*family];
                let name = match variation {
                    Some(v) => fam.variations[*v]
                        .opening
                        .and_then(|o| report.openings.get(o))
                        .map(|o| o.name.clone())
                        .unwrap_or_else(|| fam.name.clone()),
                    None => fam.name.clone(),
                };
                trf(
                    "Games · {opening} · {n}",
                    &[("opening", &name), ("n", &list.len())],
                )
            }
            Selection::Repeated { white, index } => {
                let r = &report.color(*white).repeated[*index];
                trf(
                    "Games · {move} · {n}",
                    &[
                        (
                            "move",
                            &format!(
                                "{} {}",
                                short_label(r.ply as usize, &r.san),
                                review::judgement_symbol(r.judgement)
                            ),
                        ),
                        ("n", &list.len()),
                    ],
                )
            }
            Selection::Error { white, index } => {
                let e = &report.color(*white).errors[*index];
                trf(
                    "Games · {move} · {n}",
                    &[
                        (
                            "move",
                            &format!(
                                "{} {}",
                                short_label(e.ply as usize, &e.san),
                                review::judgement_symbol(e.judgement)
                            ),
                        ),
                        ("n", &list.len()),
                    ],
                )
            }
        };
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            if self.selection != Selection::None
                && crate::widgets::small_close_button(ui)
                    .on_hover_text(tr("All games"))
                    .clicked()
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
        let width = ui.available_width();
        // The names first (at least 110 each): the event, «Then», the ratings (they go
        // into the name cell), the errors and the date give way in turn.
        let mut show_event = true;
        let mut show_then = true;
        let mut show_elo = true;
        let mut show_errors = report.evaluated();
        let mut show_date = true;
        let gap = ui.spacing().item_spacing.x;
        let player_w_of = |event: bool, then: bool, elo: bool, errors: bool, date: bool| -> f32 {
            let columns = 3
                + 2 * usize::from(elo)
                + usize::from(date)
                + usize::from(then)
                + usize::from(event)
                + usize::from(errors);
            let fixed = 44.0
                + if elo { 88.0 } else { 0.0 }
                + if date { 84.0 } else { 0.0 }
                + if errors { 80.0 } else { 0.0 }
                + match (then, event) {
                    (true, true) => 150.0,
                    (true, false) => 100.0,
                    _ => 0.0,
                }
                + if event { 100.0 } else { 0.0 }
                + gap * columns as f32;
            (width - fixed) / 2.0
        };
        for drop in 0..5 {
            if player_w_of(show_event, show_then, show_elo, show_errors, show_date) >= 110.0 {
                break;
            }
            match drop {
                0 => show_event = false,
                1 => show_then = false,
                2 => show_elo = false,
                3 => show_errors = false,
                _ => show_date = false,
            }
        }
        let player_w =
            player_w_of(show_event, show_then, show_elo, show_errors, show_date).clamp(60.0, 170.0);
        let plies: HashMap<u32, usize> = list.iter().map(|&g| (g, self.game_ply(g))).collect();
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
        let mut game_click = None;
        let sort = self.sorts.games;
        let player_names: HashSet<&str> = self.names.iter().map(String::as_str).collect();
        table
            .header(ROW_H, |mut h| {
                let c = &mut game_click;
                sort_col(&mut h, tr("White"), "", GameSort::White, sort, c);
                if show_elo {
                    sort_col(&mut h, "Elo", "", GameSort::WhiteElo, sort, c);
                }
                sort_col(&mut h, tr("Black"), "", GameSort::Black, sort, c);
                if show_elo {
                    sort_col(&mut h, "Elo", "", GameSort::BlackElo, sort, c);
                }
                sort_col(&mut h, tr("Res."), "", GameSort::Result, sort, c);
                if show_date {
                    sort_col(&mut h, tr("Date"), "", GameSort::Date, sort, c);
                }
                if show_then {
                    sort_col(
                        &mut h,
                        tr("Then"),
                        tr("How the game went on from this position"),
                        GameSort::Then,
                        sort,
                        c,
                    );
                }
                if show_event {
                    sort_col(&mut h, tr("Event"), "", GameSort::Event, sort, c);
                }
                if show_errors {
                    sort_col(
                        &mut h,
                        tr("Errors"),
                        tr("His marked moves in the opening of this game"),
                        GameSort::Errors,
                        sort,
                        c,
                    );
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
                        ui.add(
                            egui::Label::new(if player_names.contains(n) {
                                t.strong()
                            } else {
                                t
                            })
                            .truncate(),
                        );
                    };
                    let elo = |e: u16| if e > 0 { e.to_string() } else { String::new() };
                    let ply = plies.get(&gi).copied().unwrap_or(0);
                    // Without their column the ratings go after the names, weak.
                    let name = |ui: &mut Ui, n: &str, e: u16| {
                        name(ui, n);
                        if !show_elo && e > 0 {
                            ui.label(RichText::new(e.to_string()).weak().small());
                        }
                    };
                    row.col(|ui| name(ui, &r.white, r.white_elo));
                    if show_elo {
                        row.col(|ui| {
                            ui.label(RichText::new(elo(r.white_elo)).weak());
                        });
                    }
                    row.col(|ui| name(ui, &r.black, r.black_elo));
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
                            let then: Vec<String> =
                                g.sans.iter().skip(ply).take(3).cloned().collect();
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
                            ui.add(
                                egui::Label::new(crate::san::show_line(text.trim_end())).truncate(),
                            );
                        });
                    }
                    if show_event {
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&r.event).italics().weak())
                                    .truncate(),
                            );
                        });
                    }
                    if show_errors {
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let cr = report.color(g.white);
                            for e in cr
                                .errors
                                .iter()
                                .filter(|e| e.game == gi && self.visible(e.judgement))
                            {
                                let number = e.ply as usize / 2 + 1;
                                ui.label(RichText::new(format!("{number}.")).small());
                                pill(ui, e.judgement).on_hover_text(format!(
                                    "{} {} — {}",
                                    short_label(e.ply as usize, &e.san),
                                    review::judgement_name(e.judgement),
                                    trf(
                                        "better {best}",
                                        &[("best", &crate::san::show(&e.best_san))]
                                    )
                                ));
                            }
                        });
                    }
                    let resp = row
                        .response()
                        .on_hover_text(tr("Double click: open this game at this position"));
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
        if let Some(k) = game_click {
            let names = matches!(
                k,
                GameSort::White
                    | GameSort::Black
                    | GameSort::Result
                    | GameSort::Then
                    | GameSort::Event
            );
            self.sorts.games.click(k, names);
            // The keyboard's row is a place in the list: it goes back to the top.
            self.games_cursor = None;
        }
        // The game's move from the shown position, as an arrow.
        if let Some(gi) = hovered
            && let Some(s) = &self.shown_pos
            && s.ply + 1 == s.positions.len()
        {
            let g = &report.games[gi as usize];
            let ply = plies.get(&gi).copied().unwrap_or(0);
            if let Some(m) = g
                .ucis
                .get(ply)
                .and_then(|u| u.parse::<UciMove>().ok())
                .and_then(|u| u.to_move(&s.positions[s.ply]).ok())
            {
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
                ply: self.open_ply(gi),
            });
        }
    }

    // ----- the keyboard --------------------------------------------------------------

    fn keyboard(
        &mut self,
        ctx: &egui::Context,
        settings: &Settings,
        actions: &mut Vec<PrepAction>,
    ) {
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
            self.set_color(ColorChoice::White);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&alt(Key::Num2))) {
            self.set_color(ColorChoice::Black);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&alt(Key::Num3))) {
            self.set_color(ColorChoice::Both);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::W))) {
            actions.push(PrepAction::Back);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F)) {
            let base = self
                .shown_pos
                .as_ref()
                .map_or(self.color != ColorChoice::Black, |s| s.white);
            self.flipped = Some(!self.board_white() == base);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::A)) {
            if self.job.is_some() {
                self.stop_engine(actions);
            } else if self.report.is_some() {
                self.engine_asked(settings, ctx, actions);
            }
        }
        if ctx.input_mut(|i| {
            i.consume_shortcut(&KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::F,
            ))
        }) && let Some(s) = &self.shown_pos
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
            actions.push(PrepAction::FindPosition(
                self.path.clone(),
                Chess::default(),
                moves,
                !self.board_white(),
            ));
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
            if enter && let Some(r) = &self.report {
                actions.push(PrepAction::OpenGame {
                    path: self.path.clone(),
                    index: r.games[list[next] as usize].index as usize,
                    ply: self.open_ply(list[next]),
                });
            }
            return;
        }
        match self.tab {
            Tab::Repertoire => self.tree_keys(up, down, left, right, enter, space, home, end),
            Tab::Openings => self.openings_keys(up, down, left, right, enter, space, home, end),
            Tab::Errors => {
                self.errors_keys(up, down, left, right, enter, space, home, end, actions)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tree_keys(
        &mut self,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        enter: bool,
        space: bool,
        home: bool,
        end: bool,
    ) {
        let rows = self.tree_rows();
        if rows.is_empty() {
            return;
        }
        let at = rows.iter().position(|r| match (r, &self.selection) {
            (
                TreeRow::Node { white, node, .. },
                Selection::Node {
                    white: sw,
                    node: sn,
                },
            ) => white == sw && node == sn,
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
            let last = rows
                .iter()
                .rposition(|r| matches!(r, TreeRow::Node { .. }))
                .unwrap_or(0);
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
            } else if let Some(p) = self
                .report
                .as_ref()
                .and_then(|r| r.color(white).tree.nodes[node as usize].parent)
            {
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
    fn openings_keys(
        &mut self,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        enter: bool,
        space: bool,
        home: bool,
        end: bool,
    ) {
        let rows = self.opening_rows();
        if rows.is_empty() {
            return;
        }
        let at = rows.iter().position(|&(w, f, v)| {
            self.selection
                == Selection::Opening {
                    white: w,
                    family: f,
                    variation: v,
                }
        });
        let go = |screen: &mut Self, i: usize| {
            if let Some(&(white, family, variation)) = rows.get(i) {
                screen.select(Selection::Opening {
                    white,
                    family,
                    variation,
                });
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
                go(
                    self,
                    rows.iter()
                        .position(|&(w, ff, vv)| w == white && ff == f && vv.is_none())
                        .unwrap_or(at),
                );
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
    if let Some(a) = best.and_then(|b| {
        to_arrow(
            b,
            crate::board::shape_preview_color(chess_core::ShapeColor::Green),
        )
    }) {
        out.push(a);
    }
    out
}

/// «6…Nb6 ? — better 6…e6 · 0.0 → −1.2».
fn error_note(e: &prep::OpeningError) -> String {
    let played = format!(
        "{} {}",
        short_label(e.ply as usize, &e.san),
        review::judgement_symbol(e.judgement)
    );
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
    fn a_header_click_sorts_then_turns_the_order_round() {
        let mut s = PrepSorts::default().games;
        assert_eq!(
            (s.key, s.ascending),
            (GameSort::Date, false),
            "newest first"
        );
        s.click(GameSort::Date, false);
        assert!(s.ascending, "the same column again: the other way");
        s.click(GameSort::White, true);
        assert_eq!(
            (s.key, s.ascending),
            (GameSort::White, true),
            "a name column starts A–Z"
        );
        assert_eq!(s.apply(std::cmp::Ordering::Less), std::cmp::Ordering::Less);
        s.click(GameSort::White, true);
        assert_eq!(
            s.apply(std::cmp::Ordering::Less),
            std::cmp::Ordering::Greater
        );
        assert_eq!(cmp_val(&num(2), &num(10)), std::cmp::Ordering::Less);
        assert_eq!(
            cmp_val(&SortVal::Text("b".into()), &SortVal::Text("a".into())),
            std::cmp::Ordering::Greater
        );
        // Kept in the settings as they are.
        let json = serde_json::to_string(&PrepSorts::default()).unwrap();
        assert_eq!(
            serde_json::from_str::<PrepSorts>(&json).unwrap(),
            PrepSorts::default()
        );
    }

    #[test]
    fn labels_and_lines() {
        assert_eq!(short_label(0, "e4"), "1.e4");
        assert_eq!(short_label(1, "c5"), "1…c5");
        assert_eq!(short_label(4, "Bb5"), "3.Bb5");
        let sans: Vec<String> = ["e4", "c5", "Nf3"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(line_text(&sans), "1.e4 c5 2.Nf3");
        assert_eq!(
            table_line("1. e4 c5 2. Nf3 Nc6 3. Bb5"),
            vec!["1.e4", "c5", "2.Nf3", "Nc6", "3.Bb5"]
        );
        assert_eq!(table_line("1... e5"), vec!["1…e5"]);
        let ucis: Vec<String> = ["e2e4", "c7c5", "g1f3"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
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
        assert_eq!(
            period(&[game("2026.10.18"), game("2026.01.03"), game("")]),
            "01.2026 – 10.2026"
        );
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
        assert_eq!(
            BookChoice::from_setting(None, own, &dbs),
            BookChoice::Database(PathBuf::from("/a/mega.cbh"))
        );
        assert_eq!(
            BookChoice::from_setting(Some(Path::new("")), own, &dbs),
            BookChoice::None
        );
        assert_eq!(
            BookChoice::from_setting(Some(own), own, &dbs),
            BookChoice::Same
        );
        assert_eq!(
            BookChoice::from_setting(Some(Path::new("/a/small.pgn")), own, &dbs),
            BookChoice::Database(PathBuf::from("/a/small.pgn"))
        );
        // A database no longer on the shelf: the default again.
        assert_eq!(
            BookChoice::from_setting(Some(Path::new("/gone.pgn")), own, &dbs),
            BookChoice::Database(PathBuf::from("/a/mega.cbh"))
        );
        assert_eq!(BookChoice::Same.setting(own), own);
        assert_eq!(BookChoice::None.setting(own), PathBuf::new());
    }
}
