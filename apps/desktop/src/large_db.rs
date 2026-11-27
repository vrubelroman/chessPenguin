//! Game list of a large ChessBase database (millions of games): a filter panel on
//! the right and a table of the matching records. Filtering and sorting run in a
//! background thread; a newer request cancels the older one. Layout and behaviour:
//! `docs/design/large-databases.md`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, channel};

use chess_import::Database;
use chess_import::chessbase::base::{
    EloRule, Filter, NameFilter, Outcome, RecordKind, Side, SortKey, Suggestion, parse_eco,
};
use chess_import::position::{Match, PositionOutcome, PositionQuery};
use egui::{Color32, RichText, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use shakmaty::uci::UciMove;

use crate::board_search::{self, Background, BoardEvent, SearchBoard};
use crate::i18n::{tr, trf};
use crate::library::LibraryAction;
use crate::prep_ui::PrepEntry;
use crate::settings::Settings;
use crate::viewed::{self, Viewed};

/// The × of a text field and its gap (`widgets::small_close_button` is 22 pt wide).
const CLEAR_SLOT: f32 = 26.0;

/// Text edits are applied this long after the last key press.
const DEBOUNCE: f64 = 0.25;
/// A pass that takes longer than this dims the table and shows its progress.
const SLOW: f64 = 0.15;
const SUGGESTIONS: usize = 8;

/// The name fields that offer suggestions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NameField {
    Player,
    Opponent,
    Event,
}

/// What the user typed and picked in the panel.
#[derive(Clone, Default)]
struct Form {
    player: NameFilter,
    side: Side,
    opponent: NameFilter,
    event: NameFilter,
    annotator: String,
    year_from: String,
    year_to: String,
    elo_min: String,
    elo_rule: EloRule,
    outcomes: Vec<Outcome>,
    eco: String,
    moves_from: String,
    moves_to: String,
    with_comments: bool,
    with_variations: bool,
    hide_texts: bool,
    /// The result buttons are relative to the player (win / loss).
    relative_outcomes: bool,
    /// The quick search of the header (not a condition of the panel).
    search: String,
}

/// Number in a field: `Ok(None)` when empty, `Err` when not a number in `range`.
fn number(text: &str, range: std::ops::RangeInclusive<u16>) -> Result<Option<u16>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u16>() {
        Ok(n) if range.contains(&n) => Ok(Some(n)),
        _ => Err(()),
    }
}

impl Form {
    /// The filter for the valid fields (invalid ones are left out and shown in red).
    fn filter(&self) -> Filter {
        let years = (
            number(&self.year_from, 1..=2100),
            number(&self.year_to, 1..=2100),
        );
        let (year_from, year_to) = match years {
            (Ok(a), Ok(b)) if a.zip(b).is_none_or(|(a, b)| a <= b) => (a, b),
            (Ok(a), Err(())) => (a, None),
            (Err(()), Ok(b)) => (None, b),
            _ => (None, None),
        };
        let moves = (
            number(&self.moves_from, 0..=999),
            number(&self.moves_to, 0..=999),
        );
        let (moves_from, moves_to) = match moves {
            (Ok(a), Ok(b)) if a.zip(b).is_none_or(|(a, b)| a <= b) => (a, b),
            _ => (None, None),
        };
        let player_set = self.player.is_set();
        Filter {
            player: self.player.clone(),
            side: if player_set { self.side } else { Side::Any },
            opponent: self.opponent.clone(),
            event: self.event.clone(),
            annotator: self.annotator.clone(),
            year_from,
            year_to,
            elo_min: number(&self.elo_min, 1..=3500).ok().flatten(),
            elo_rule: self.elo_rule,
            eco: parse_eco(&self.eco).unwrap_or_default(),
            outcomes: self.outcomes.clone(),
            player_relative: player_set,
            moves_from,
            moves_to,
            with_comments: self.with_comments,
            with_variations: self.with_variations,
            games_only: self.hide_texts,
            text: self.search.trim().to_string(),
        }
    }

    /// Number of conditions set (for "Reset filters (N)").
    fn active(&self) -> usize {
        let text = |s: &str| usize::from(!s.trim().is_empty());
        usize::from(self.player.is_set())
            + usize::from(self.opponent.is_set())
            + usize::from(self.event.is_set())
            + text(&self.annotator)
            + usize::from(!self.year_from.trim().is_empty() || !self.year_to.trim().is_empty())
            + text(&self.elo_min)
            + usize::from(!self.outcomes.is_empty())
            + text(&self.eco)
            + usize::from(!self.moves_from.trim().is_empty() || !self.moves_to.trim().is_empty())
            + usize::from(self.with_comments)
            + usize::from(self.with_variations)
            + usize::from(self.hide_texts)
    }
}

#[derive(Clone)]
struct Request {
    filter: Filter,
    sort: SortKey,
    ascending: bool,
    /// The order of the view without a position (a position not applied for lack of
    /// memory sorts by it, not strongest first).
    base_sort: (SortKey, bool),
    /// The board's position while the board is shown or the position filters the list
    /// (its continuations are counted even in the starting position).
    position: Option<(u64, PositionQuery)>,
    /// The position filters the list (it differs from the starting position).
    use_position: bool,
    only_order: bool,
}

impl PartialEq for Request {
    fn eq(&self, other: &Request) -> bool {
        let pos = |r: &Request| r.position.as_ref().map(|(k, q)| (*k, q.line.clone()));
        self.filter == other.filter
            && self.sort == other.sort
            && self.ascending == other.ascending
            && self.base_sort == other.base_sort
            && pos(self) == pos(other)
            && self.use_position == other.use_position
            && self.only_order == other.only_order
    }
}

/// The game list for a request and, with a position, its search outcome.
struct Answer {
    view: Vec<u32>,
    outcome: Option<Arc<PositionOutcome>>,
    /// Games that reached the position in the board's move order, of all found.
    in_order: Option<(usize, usize)>,
    /// The position was not searched: the memory its table needs (bytes) is not there.
    blocked: Option<u64>,
}

struct Job {
    request: Request,
    work: Background<Answer>,
}

/// What the header line says about a database (the library knows it).
pub struct Header {
    /// "ChessBase, read-only", "PGN, encoding UTF-8"…
    pub format: String,
    pub format_hint: String,
    /// A database downloaded from Lichess and Chess.com: its "⟳ New games".
    pub online: Option<OnlineButton>,
    /// A file the browser holds only for this visit: "Keep a copy in the browser".
    pub keep_copy: bool,
}

pub struct OnlineButton {
    /// A download into it runs.
    pub busy: bool,
    /// The sites answered at start.
    pub enabled: bool,
    pub hint: String,
}

/// What the right side of the library shows for a database.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Hidden,
    Filters,
    Board,
}

/// Widths of the narrow set of table columns (docs/design/narrow-windows.md §1): the
/// least table is built of them, so the table and its least width do not part.
const MOVE_COL: f32 = 52.0;
const ELO_COL: f32 = 44.0;
const RESULT_COL: f32 = 56.0;
const YEAR_COL: f32 = 48.0;
/// A name still recognised ("Конюшенко С…").
const NAME_MIN: f32 = 110.0;

/// The width of the "№" column: as wide as the largest number (a check mark before it).
fn number_col(db: &Database) -> f32 {
    let digits = grouped(db.len()).chars().count() as f32;
    (digits * 8.0 + 24.0).clamp(52.0, 84.0)
}

/// The width of `text` in `style` on one line.
fn text_w(ui: &Ui, text: &str, style: egui::TextStyle) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), style.resolve(ui.style()), Color32::WHITE)
        .size()
        .x
}

/// Search outcomes of the session, by filter and position with the board's move order
/// (it decides "in this move order").
type OutcomeCache = HashMap<(u64, u64), Arc<PositionOutcome>>;

/// Matching games kept in the cache at most (16 bytes each).
const CACHE_MATCHES: usize = 20_000_000;

/// Identity of a searched position with the board's move order.
fn position_id((key, query): &(u64, PositionQuery)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    if let Some(line) = &query.line {
        for m in line {
            chess_import::position::uci(m).to_string().hash(&mut h);
        }
    } else {
        "no line".hash(&mut h);
    }
    h.finish()
}

fn filter_hash(f: &Filter) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.hash(&mut h);
    h.finish()
}

struct SuggestJob {
    field: NameField,
    query: String,
    rx: Receiver<(Vec<Suggestion>, usize)>,
}

/// The open suggestion list of a name field.
struct Suggestions {
    field: NameField,
    query: String,
    list: Vec<Suggestion>,
    total: usize,
    highlighted: Option<usize>,
}

/// The search for great moves under way, as the conditions line shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PassStatus {
    /// In this database (else in `name`).
    pub here: bool,
    pub name: String,
    pub done: usize,
    pub total: usize,
    /// The record being analysed.
    pub current: Option<u32>,
    pub left_secs: f64,
    pub stopping: bool,
}

/// A copy of chosen records being written, as the conditions line shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchStatus {
    pub name: String,
    pub done: usize,
    pub total: usize,
    pub cancelling: bool,
}

/// State of one large database in the library (kept for the session).
pub struct LargeView {
    form: Form,
    sort: (SortKey, bool),
    /// The user picked the sort order (otherwise a position sorts strongest first).
    sort_chosen: bool,
    /// Matching records, in display order, and the request they answer.
    view: Vec<u32>,
    shown: Option<Request>,
    outcome: Option<Arc<PositionOutcome>>,
    in_order: Option<(usize, usize)>,
    cache: OutcomeCache,
    job: Option<Job>,
    /// Building the search index of the base (our booster when `.cbb` cannot be used,
    /// the opening positions), once the board is used; `booster_failed` keeps a
    /// failure from being retried.
    /// The job and the work it has to do (in its progress units).
    booster: Option<(Background<bool>, usize)>,
    booster_failed: bool,
    board: SearchBoard,
    only_order: bool,
    /// Next move of the game under the pointer in the table (an arrow on the board).
    table_hover: Option<UciMove>,
    /// Time of the last edit of a text field (for the debounce).
    typed_at: f64,
    suggest_job: Option<SuggestJob>,
    suggestions: Option<Suggestions>,
    /// The pointer is on the open suggestion list.
    pointer_on_list: bool,
    /// Record index of the row under the keyboard cursor: Enter, F2 and «Board» take it.
    pub selected: Option<usize>,
    /// The records chosen (task 42): one or many, kept across filters and sorting.
    marked: crate::batch_copy::Selection,
    /// Where a Shift range starts.
    anchor: Option<usize>,
    /// Changes of `marked` and of `view`, and the chosen records the list hides then.
    marked_gen: u64,
    view_gen: u64,
    hidden: Option<(u64, u64, usize)>,
    /// The copy of chosen records being written (set by the library each frame).
    pub batch: Option<BatchStatus>,
    /// «Cancel» of that copy was clicked.
    pub cancel_batch: bool,
    /// Why the last copy wrote nothing: said in red where its progress was, until `×`.
    pub batch_error: Option<String>,
    /// The chosen records as the file had them before it was read again (a hash of
    /// each one's bytes), to know they are still the same games; `None` for a choice too
    /// large to check (it is let go when the file changes).
    marked_before: Option<Vec<(u32, u64)>>,
    /// Said once (a toast), taken by the library.
    pub notice: Option<String>,
    /// The marks of the records found by the search for great moves (task 27), set by
    /// the library; what the table shows of them, checked against each record's main
    /// line, by record, for the index's generation.
    pub marks: Option<Arc<std::sync::Mutex<crate::brilliant::Index>>>,
    shown_marks: std::cell::RefCell<(u64, HashMap<u32, Option<crate::brilliant::Entry>>)>,
    /// The table sorted by the «!!» column.
    marks_sort: bool,
    /// The database the marks are checked against (set by `table`).
    marks_db: Option<Arc<Database>>,
    /// The texts of the conditions of the list (for the search's question).
    last_conditions: Vec<String>,
    /// Whose moves the «!!» column counts: the filter's player's (found in the background
    /// for that filter), else both sides'.
    marks_sides: (Option<Filter>, crate::brilliant::Sides),
    sides_job: Option<(Filter, Receiver<crate::brilliant::Sides>)>,
    /// The search under way (here or in another database), set by the library.
    pub pass: Option<PassStatus>,
    /// Why the search cannot start now (the engine is busy), set by the app.
    pub engine_busy: Option<String>,
    /// Why the last search stopped with an error, in red where its progress was.
    pub pass_error: Option<String>,
    /// «Stop» of the search was clicked.
    pub stop_pass: bool,
    /// «!! Find great moves» was clicked (taken by `table`, which has the path).
    find_clicked: bool,
    scroll_to_selected: bool,
    scroll_to_top: bool,
    focus_player: bool,
    pane: Pane,
    /// Last text of each name field, to notice typing.
    typed: [String; 3],
    /// Most games have no result: the continuations count chapters (a book).
    books: bool,
    /// The position of the shown list could not be searched for lack of memory
    /// (the memory its table needs, bytes).
    blocked: Option<u64>,
    /// Ctrl+F asked for the quick search.
    focus_search: bool,
    /// The database takes games (delete, add a copy): set by `table`.
    writable: bool,
    /// The file was read again: the next list is the same list, not a new filter (the
    /// place in the table and the selection stay).
    rereading: bool,
    /// The window is too narrow for the filters beside the table: they open over it
    /// (`drawer_open`), and too narrow for the board beside it: one at a time (§2).
    narrow_filters: bool,
    narrow_board: bool,
    drawer_open: bool,
    /// Something asked for the filters this frame (it was clicked, not "elsewhere").
    drawer_asked: bool,
    /// Where the filters open over the table: under its header and conditions.
    drawer_top: f32,
    /// The quick search has the keyboard (arrows and Enter go to the table).
    search_focused: bool,
    /// «Prepare against…» was asked (the panel's button, a row's menu): the library
    /// takes it (docs/design/opponent-prep.md §2.1).
    pub prepare: Option<PrepEntry>,
    /// The web page of the game whose context menu is open (its record, the link).
    menu_link: Option<(usize, Option<crate::game_link::GameLink>)>,
}

impl LargeView {
    pub fn new() -> LargeView {
        // Dev hook (screenshots, manual checks): a player filter from the start.
        let mut form = Form::default();
        if let Ok(p) = std::env::var("CP_FILTER_PLAYER") {
            form.player.text = p;
        }
        // `CP_FILTER="player=carlsen;years=2015-2026;elo=2600"` (screenshots).
        if let Ok(spec) = std::env::var("CP_FILTER") {
            for part in spec.split(';') {
                let Some((k, v)) = part.split_once('=') else {
                    continue;
                };
                match k.trim() {
                    "player" => form.player.text = v.to_string(),
                    "years" => {
                        let (a, b) = v.split_once('-').unwrap_or((v, ""));
                        form.year_from = a.to_string();
                        form.year_to = b.to_string();
                    }
                    "elo" => form.elo_min = v.to_string(),
                    _ => {}
                }
            }
        }
        // `CP_SEARCH="carlsen 2024"`: the quick search from the start (screenshots).
        if let Ok(t) = std::env::var("CP_SEARCH") {
            form.search = t;
        }
        let (board, mut pane) = match SearchBoard::from_env() {
            Some(b) => (b, Pane::Board),
            None => (SearchBoard::new(), Pane::Filters),
        };
        // `CP_PANE=hidden`: no panel; `=drawer`: the filters open over the table in a
        // narrow window (beside it in a wide one).
        let pane_env = std::env::var("CP_PANE").ok();
        if pane_env.as_deref() == Some("hidden") {
            pane = Pane::Hidden;
        }
        let drawer = pane_env.as_deref() == Some("drawer");
        // `CP_SORT=date`: newest games first (screenshots).
        // `CP_SELECT=3,7,15`: these records (their numbers in the table) chosen, the
        // cursor on the last (screenshots of task 42).
        let mut marked = crate::batch_copy::Selection::default();
        let mut cursor = None;
        if let Ok(list) = std::env::var("CP_SELECT") {
            for n in list
                .split(',')
                .filter_map(|n| n.trim().parse::<usize>().ok())
            {
                if n > 0 {
                    marked.insert(n - 1);
                    cursor = Some(n - 1);
                }
            }
        }
        let sort = if std::env::var("CP_SORT").as_deref() == Ok("date") {
            (SortKey::Date, false)
        } else {
            (SortKey::Number, true)
        };
        LargeView {
            form,
            sort,
            sort_chosen: sort.0 != SortKey::Number,
            view: Vec::new(),
            shown: None,
            outcome: None,
            in_order: None,
            cache: HashMap::new(),
            job: None,
            booster: None,
            booster_failed: false,
            board,
            only_order: false,
            table_hover: None,
            typed_at: f64::NEG_INFINITY,
            suggest_job: None,
            suggestions: None,
            pointer_on_list: false,
            selected: cursor,
            marked,
            anchor: cursor,
            marked_gen: 0,
            view_gen: 0,
            hidden: None,
            batch: None,
            cancel_batch: false,
            batch_error: None,
            marked_before: None,
            notice: None,
            marks: None,
            shown_marks: std::cell::RefCell::new((u64::MAX, HashMap::new())),
            marks_sort: false,
            marks_db: None,
            last_conditions: Vec::new(),
            marks_sides: (None, Default::default()),
            sides_job: None,
            pass: None,
            engine_busy: None,
            pass_error: None,
            stop_pass: false,
            find_clicked: false,
            scroll_to_selected: false,
            scroll_to_top: false,
            focus_player: false,
            pane,
            typed: Default::default(),
            books: false,
            blocked: None,
            focus_search: false,
            writable: false,
            rereading: false,
            search_focused: false,
            narrow_filters: drawer,
            narrow_board: false,
            drawer_open: drawer,
            drawer_asked: false,
            drawer_top: 0.0,
            prepare: None,
            menu_link: None,
        }
    }

    /// The view of a database just opened: a book's continuations count chapters, and
    /// games downloaded from Lichess and Chess.com come newest first (§9.6).
    pub fn for_database(db: &Database, online: bool) -> LargeView {
        let mut view = LargeView::new();
        view.books = db.books();
        if online && view.sort == (SortKey::Number, true) {
            view.sort = (SortKey::Date, false);
        }
        view
    }

    /// The database was read again (it grew): the list is found anew, the conditions,
    /// the position, the sort and the selection stay (§9.7).
    pub fn reread(&mut self, db: &Database) {
        self.job = None;
        self.shown = None;
        self.rereading = true;
        self.outcome = None;
        self.in_order = None;
        self.cache.clear();
        self.blocked = None;
        self.books = db.books();
        self.shown_marks.borrow_mut().1.clear();
        // Several chosen: kept only if each is still the same record (a game added at
        // the end keeps them; one deleted or put back moves the numbers). Otherwise let
        // go, and said: the choice never moves to other games unnoticed.
        if self.marked.len() >= 2 {
            let same = std::mem::take(&mut self.marked_before).is_some_and(|before| {
                before.len() == self.marked.len()
                    && before
                        .iter()
                        .all(|&(r, h)| (r as usize) < db.len() && record_hash(db, r as usize) == h)
            });
            if !same {
                self.clear_marks();
                self.notice = Some(tr("Selection cleared: the database changed").to_owned());
            }
        } else if self.marked.last().is_some_and(|r| r >= db.len()) {
            self.mark(|m| m.clear());
        }
        self.selected = self.selected.filter(|&c| c < db.len());
    }

    /// Before the file is read again: what the chosen records are now (up to
    /// `CHECKED_CHOICE` of them).
    pub fn before_reread(&mut self, db: &Database) {
        self.marked_before =
            (self.marked.len() >= 2 && self.marked.len() <= CHECKED_CHOICE).then(|| {
                self.marked
                    .records()
                    .into_iter()
                    .map(|r| (r, record_hash(db, r as usize)))
                    .collect()
            });
    }

    /// Selects a record and scrolls to it (returning from a game): a choice of several
    /// records stays.
    pub fn reveal(&mut self, index: usize) {
        self.selected = Some(index);
        if self.marked.len() <= 1 {
            self.mark(|m| m.only(index));
        }
        self.scroll_to_selected = true;
    }

    /// Changes the chosen records.
    fn mark(&mut self, change: impl FnOnce(&mut crate::batch_copy::Selection)) {
        change(&mut self.marked);
        self.marked_gen += 1;
    }

    /// No record chosen any more.
    pub fn clear_marks(&mut self) {
        self.mark(|m| m.clear());
        self.anchor = None;
    }

    /// The chosen records hidden by the list's filter, counted once per change.
    fn hidden_marked(&mut self) -> usize {
        let key = (self.marked_gen, self.view_gen);
        match self.hidden {
            Some((m, v, n)) if (m, v) == key => n,
            _ => {
                let n = self.marked.len() - self.marked.count_in(&self.view);
                self.hidden = Some((key.0, key.1, n));
                n
            }
        }
    }

    /// The choice comes down to the cursor's row (Esc, «Clear», a row deleted).
    fn mark_cursor_only(&mut self) {
        let cursor = self.selected;
        self.mark(|m| match cursor {
            Some(i) => m.only(i),
            None => m.clear(),
        });
        self.anchor = cursor;
    }

    /// The records of the view from position `a` to position `b` (either order).
    fn range(&self, a: usize, b: usize) -> &[u32] {
        let (lo, hi) = (a.min(b), a.max(b));
        &self.view[lo..=hi.min(self.view.len().saturating_sub(1))]
    }

    fn position_of(&self, record: usize) -> Option<usize> {
        self.view.iter().position(|&i| i as usize == record)
    }

    /// A click on the row of `index` with `mods` (§42.5).
    fn click(&mut self, index: usize, mods: egui::Modifiers) {
        // An anchor hidden by the filter: the cursor, when it is seen, starts the range.
        let anchor = self
            .anchor
            .filter(|a| self.position_of(*a).is_some())
            .or(self.selected.filter(|c| self.position_of(*c).is_some()));
        let at = self.position_of(index);
        match (
            mods.command,
            mods.shift,
            anchor.and_then(|a| self.position_of(a)),
            at,
        ) {
            (add, true, Some(a), Some(b)) => {
                let range = self.range(a, b).to_vec();
                self.mark(|m| {
                    if !add {
                        m.clear();
                    }
                    m.extend(&range);
                });
            }
            (true, false, _, _) => {
                self.mark(|m| m.toggle(index));
                self.anchor = Some(index);
            }
            _ => {
                self.mark(|m| m.only(index));
                self.anchor = Some(index);
            }
        }
        self.selected = Some(index);
    }

    /// The filter's player's sides of each record, for the «!!» column: found again in
    /// the background when the filter changes; both sides without a player.
    fn update_marks_sides(&mut self, db: &Arc<Database>, ctx: &egui::Context) {
        let filter = self.form.filter();
        if let Some((f, rx)) = &self.sides_job
            && let Ok(sides) = rx.try_recv()
        {
            self.marks_sides = (Some(f.clone()), sides);
            self.sides_job = None;
            self.shown_marks.borrow_mut().1.clear();
            if self.marks_sort {
                self.sort_by_marks();
            }
        }
        let wanted = self.marks_sides.0.as_ref() == Some(&filter)
            || self.sides_job.as_ref().is_some_and(|(f, _)| *f == filter);
        if wanted {
            return;
        }
        if !filter.player.is_set() {
            self.marks_sides = (Some(filter), Default::default());
            self.sides_job = None;
            self.shown_marks.borrow_mut().1.clear();
            return;
        }
        let (tx, rx) = channel();
        let (db, f, ctx) = (db.clone(), filter.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("marks-sides".into())
            .spawn(move || {
                let sides = crate::brilliant::player_sides(
                    &db,
                    &f,
                    &std::sync::atomic::AtomicBool::new(false),
                );
                let _ = tx.send(sides);
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.sides_job = Some((filter, rx));
        }
    }

    /// The database the marks are checked against.
    pub fn set_marks_db(&mut self, db: &Arc<Database>) {
        if !self.marks_db.as_ref().is_some_and(|d| Arc::ptr_eq(d, db)) {
            self.marks_db = Some(db.clone());
        }
    }

    /// The table sorted by its «!!» column (the summary's «Sort»).
    pub fn sort_marks_on(&mut self) {
        self.marks_sort = true;
        self.sort_by_marks();
    }

    /// The list by its marks (!! first), each group in the order of its numbers.
    fn sort_by_marks(&mut self) {
        let Some(index) = self.marks.clone() else {
            return;
        };
        let Ok(index) = index.lock() else {
            return;
        };
        let sides = &self.marks_sides.1;
        // Only the entries of the games the database has there now (a deleted game's
        // marks are not its successor's).
        let current: std::collections::HashSet<u32> = match &self.marks_db {
            Some(db) => index
                .records()
                .filter(|&r| crate::brilliant::current_entry(&index, db, r).is_some())
                .collect(),
            None => std::collections::HashSet::new(),
        };
        self.view
            .sort_by_key(|&r| (index.rank(r, sides, &|r| current.contains(&r)), r));
        self.view_gen += 1;
    }

    /// The marks of record `index` to show: those of the index if the record still has
    /// the main line they were found in (checked once per record and index change).
    fn shown_mark(
        marks: &Option<Arc<std::sync::Mutex<crate::brilliant::Index>>>,
        shown: &std::cell::RefCell<(u64, HashMap<u32, Option<crate::brilliant::Entry>>)>,
        sides: &crate::brilliant::Sides,
        db: &Database,
        record: u32,
    ) -> Option<crate::brilliant::Entry> {
        let index = marks.as_ref()?.lock().ok()?;
        let mut shown = shown.borrow_mut();
        if index.generation != shown.0 {
            shown.1.clear();
            shown.0 = index.generation;
        }
        let entry = index.get(record)?.clone();
        drop(index);
        shown
            .1
            .entry(record)
            .or_insert_with(|| {
                let game = db.load_game(record as usize)?.game;
                let line = game_review::GameAnalysis::new(&game, None).cache_key();
                (entry.line == line).then_some(entry)
            })
            .clone()
            .map(|e| {
                let (w, b) = sides.of(record);
                e.for_sides(w, b)
            })
    }

    /// «!! Find great moves» (the button, Shift+A): the library asks first.
    fn ask_search(&mut self, path: &Path, actions: &mut Vec<LibraryAction>) {
        let about = if self.last_conditions.is_empty() {
            tr("all games").to_owned()
        } else {
            trf(
                "matching: {list}",
                &[("list", &self.last_conditions.join(" · "))],
            )
        };
        actions.push(LibraryAction::FindGreatMoves {
            path: path.to_path_buf(),
            records: self.view.clone(),
            filter: self.form.filter(),
            about,
        });
    }

    /// «!! Find great moves»: the button, or Shift+A outside a field (a disabled button
    /// says why, also over an empty list).
    fn handle_find(&mut self, ui: &mut Ui, path: &Path, actions: &mut Vec<LibraryAction>) {
        if !crate::platform::CAPS.great_moves {
            return;
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        let shift_a = !typing
            && !crate::app::overlay_open_besides(ui.ctx(), &[egui::Id::new("filters_drawer")])
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::A));
        if std::mem::take(&mut self.find_clicked) || shift_a {
            match (self.pass.is_some(), self.search_blocked()) {
                (false, None) => self.ask_search(path, actions),
                (false, Some(why)) => actions.push(LibraryAction::Notice(why)),
                (true, _) => {}
            }
        }
    }

    /// Why the search cannot start on the list shown, if it cannot.
    fn search_blocked(&self) -> Option<String> {
        if let Some(why) = &self.engine_busy {
            return Some(why.clone());
        }
        if let Some(p) = self.pass.as_ref().filter(|p| !p.here) {
            return Some(trf(
                "Searching in «{db}»: {i} of {n}; stop it there",
                &[("db", &p.name), ("i", &p.done), ("n", &p.total)],
            ));
        }
        if self.job.is_some() || self.shown.is_none() {
            return Some(tr("Wait: the games are still being filtered").to_owned());
        }
        match crate::brilliant::can_start(self.view.len()) {
            Ok(()) => None,
            Err(crate::brilliant::Blocked::NoGames) => {
                Some(tr("No games match these conditions").to_owned())
            }
            Err(crate::brilliant::Blocked::TooMany) => Some(format!(
                "{}\n{}",
                tr("Filter the games down to 100 or fewer"),
                trf("Now: {n}", &[("n", &grouped(self.view.len()))])
            )),
        }
    }

    /// Every row of the list chosen (Ctrl+A): the list's records only.
    fn select_all(&mut self) {
        let view = std::mem::take(&mut self.view);
        self.mark(|m| m.extend(&view));
        self.view = view;
        if self.selected.is_none() {
            self.selected = self.view.first().map(|&i| i as usize);
        }
    }

    /// «New database from selected…»: the library opens the dialog (none while another
    /// copy is written).
    fn ask_copy(&mut self, path: &Path, actions: &mut Vec<LibraryAction>) {
        if self.batch.is_some() || !crate::platform::CAPS.batch_copy {
            return;
        }
        let (records, sort, ascending) = self.copy_request();
        actions.push(LibraryAction::CopySelection {
            path: path.to_path_buf(),
            records,
            sort,
            ascending,
            hidden: self.hidden_marked(),
        });
    }

    /// The chosen records for a copy, in the table's order once sorted by the copy.
    fn copy_request(&self) -> (Vec<u32>, SortKey, bool) {
        let (sort, ascending) = if self.blocked.is_some() {
            self.sort
        } else {
            self.effective_sort()
        };
        (self.marked.records(), sort, ascending)
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// The least width of the table with the narrow set of columns (§1): the "Move"
    /// column instead of "№" when the list is a position's games.
    pub fn table_min_width(&self, db: &Database, use_position: bool) -> f32 {
        let first = if use_position {
            MOVE_COL
        } else {
            number_col(db)
        };
        first + 2.0 * ELO_COL + RESULT_COL + YEAR_COL + 2.0 * NAME_MIN + 6.0 * 8.0 + 10.0 + 16.0
    }

    /// The layout the window allows (§2.1). Narrowing closes the filters rather than
    /// laying them over the table; widening docks the filters opened over it.
    pub fn set_layout(&mut self, narrow_filters: bool, narrow_board: bool) {
        // Called first in each frame: what asked for the filters is this frame's.
        self.drawer_asked = false;
        if narrow_filters && !self.narrow_filters {
            self.drawer_open = false;
        }
        if !narrow_filters && self.narrow_filters && self.drawer_open {
            // The filters opened over the list dock; opened over the board, they close
            // and the board stays (beside the list now, or alone).
            self.drawer_open = false;
            if self.pane != Pane::Board {
                self.pane = Pane::Filters;
            }
        }
        self.narrow_filters = narrow_filters;
        self.narrow_board = narrow_board;
    }

    /// The filters open over the table: from where (under the header).
    pub fn drawer(&self) -> Option<f32> {
        (self.narrow_filters && self.drawer_open).then_some(self.drawer_top)
    }

    /// A click elsewhere closes the filters over the list, unless that click asked for
    /// them (a condition's chip).
    pub fn close_drawer_on_click(&mut self) {
        if !self.drawer_asked {
            self.drawer_open = false;
        }
    }

    /// The board takes the place of the table (a narrow window).
    pub fn board_alone(&self) -> bool {
        self.narrow_board && self.pane == Pane::Board
    }

    /// Escape in the library with no field typing: the filters over the table close,
    /// then the board alone gives the place back to the table.
    pub fn escape(&mut self) -> bool {
        if self.board_alone() && self.board.is_busy() {
            // A piece picked up on the board is put back first, as in a game.
            self.board.drop_pick();
            true
        } else if self.drawer().is_some() {
            self.drawer_open = false;
            true
        } else if self.marked.len() >= 2 {
            self.mark_cursor_only();
            true
        } else if self.board_alone() {
            self.pane = Pane::Hidden;
            true
        } else {
            false
        }
    }

    /// Opens the filters: beside the table, or over it in a narrow window (over the
    /// board alone too, which stays).
    fn open_filters(&mut self) {
        if self.narrow_filters {
            self.drawer_open = true;
            self.drawer_asked = true;
            if !self.board_alone() {
                self.pane = Pane::Filters;
            }
        } else {
            self.pane = Pane::Filters;
        }
    }

    /// Shows the board with `line` played from `start` (a position from a game).
    pub fn show_position(
        &mut self,
        start: shakmaty::Chess,
        line: Vec<shakmaty::Move>,
        flipped: bool,
    ) {
        self.board.set(start, line, flipped);
        self.only_order = false;
        self.pane = Pane::Board;
    }

    /// Sort order in effect: a position without a chosen order sorts strongest first.
    fn effective_sort(&self) -> (SortKey, bool) {
        if self.board.is_set() && !self.sort_chosen {
            (SortKey::AverageElo, false)
        } else {
            self.sort
        }
    }

    fn request(&self) -> Request {
        let (sort, ascending) = self.effective_sort();
        let use_position = self.board.is_set();
        Request {
            filter: self.form.filter(),
            sort,
            ascending,
            base_sort: self.sort,
            position: (use_position || self.pane == Pane::Board)
                .then(|| (self.board.key(), self.board.query())),
            use_position,
            only_order: use_position && self.only_order,
        }
    }

    /// The search outcome for the board's current position, if the shown list has it.
    fn board_outcome(&self) -> Option<&PositionOutcome> {
        let shown = self.shown.as_ref()?;
        let (key, query) = shown.position.as_ref()?;
        let current = self.board.query();
        (*key == self.board.key()
            && query.line == current.line
            && shown.filter == self.form.filter())
        .then_some(self.outcome.as_deref())
        .flatten()
    }

    /// Collects finished work and starts what the form asks for.
    fn update(&mut self, db: &Arc<Database>, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Some(job) = &self.job
            && let Some(result) = job.work.poll()
        {
            let job = self.job.take().expect("job");
            if let Some(answer) = result {
                let rereading = std::mem::take(&mut self.rereading);
                let new_filter = !rereading
                    && self.shown.as_ref().is_none_or(|s| {
                        s.filter != job.request.filter
                            || s.use_position != job.request.use_position
                            || (job.request.use_position
                                && s.position.as_ref().map(|p| p.0)
                                    != job.request.position.as_ref().map(|p| p.0))
                    });
                if let (Some(out), Some(pos)) = (&answer.outcome, &job.request.position) {
                    let kept: usize = self.cache.values().map(|o| o.matches.len()).sum();
                    if kept + out.matches.len() > CACHE_MATCHES {
                        self.cache.clear();
                    }
                    self.cache.insert(
                        (filter_hash(&job.request.filter), position_id(pos)),
                        out.clone(),
                    );
                }
                self.view = answer.view;
                self.view_gen += 1;
                if self.marks_sort {
                    self.sort_by_marks();
                }
                self.outcome = answer.outcome;
                self.in_order = answer.in_order;
                self.blocked = answer.blocked;
                self.shown = Some(job.request);
                // A selected game that left the list is no longer selected, so Enter
                // cannot open a game the user does not see.
                // A choice of several records stays: it is built from several searches,
                // and its hidden part is counted.
                if let Some(sel) = self.selected
                    && new_filter
                    && !self.view.contains(&(sel as u32))
                {
                    self.selected = None;
                    if self.marked.len() <= 1 {
                        self.mark(|m| m.clear());
                    }
                }
                if new_filter {
                    // The user looks at the result of the new filter from its top.
                    self.scroll_to_top = true;
                } else if self.selected.is_some() {
                    self.scroll_to_selected = true;
                }
            }
        }
        let wanted = self.request();
        let pending = self.job.as_ref().map(|j| &j.request);
        let current = pending.or(self.shown.as_ref());
        if current != Some(&wanted) {
            let wait = self.typed_at + DEBOUNCE - now;
            if wait > 0.0 && self.shown.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            } else {
                self.start(db, wanted, now, ctx);
            }
        }
        if let Some(j) = &self.suggest_job
            && let Ok((list, total)) = j.rx.try_recv()
        {
            let j = self.suggest_job.take().expect("suggest job");
            self.suggestions = Some(Suggestions {
                field: j.field,
                query: j.query,
                list,
                total,
                highlighted: None,
            });
        }
        self.update_booster(db, now, ctx);
        if self.job.is_some() || self.suggest_job.is_some() || self.booster.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// Builds the search index in the background when the board is in use and the
    /// base lacks something (`Base::prepare`); searches meanwhile run without it.
    fn update_booster(&mut self, db: &Arc<Database>, now: f64, ctx: &egui::Context) {
        if let Some((b, _)) = &self.booster
            && let Some(result) = b.poll()
        {
            self.booster = None;
            self.booster_failed = result != Some(true);
        }
        let wanted = self.pane == Pane::Board || self.board.is_set();
        if !wanted
            || self.booster.is_some()
            || self.booster_failed
            || !db.as_chessbase().is_some_and(|b| b.prepare_wanted())
        {
            return;
        }
        let work = db.as_chessbase().map_or(0, |b| b.prepare_work());
        let db = db.clone();
        let job = Background::spawn(ctx, now, move |progress, cancel| {
            match db.as_chessbase()?.prepare(progress, cancel) {
                Ok(true) => Some(true),
                Ok(false) => None,
                Err(e) => {
                    eprintln!("search index: {e}");
                    Some(false)
                }
            }
        });
        self.booster = job.map(|j| (j, work));
    }

    fn start(&mut self, db: &Arc<Database>, request: Request, now: f64, ctx: &egui::Context) {
        // Dropping the old job cancels it.
        self.job = None;
        let cached = request
            .position
            .as_ref()
            .and_then(|pos| {
                self.cache
                    .get(&(filter_hash(&request.filter), position_id(pos)))
            })
            .cloned();
        // A PGN database without the memory for its position table: the list by the
        // filters only, and the board says why (checked once per request).
        let blocked = request
            .position
            .as_ref()
            .filter(|_| cached.is_none())
            .and_then(|_| db.positions_blocked());
        let (db, req) = (db.clone(), request.clone());
        let work = Background::spawn(ctx, now, move |progress, cancel| {
            if blocked.is_some() {
                let (sort, ascending) = req.base_sort;
                return Some(Answer {
                    view: db.query(&req.filter, sort, ascending, progress, cancel)?,
                    outcome: None,
                    in_order: None,
                    blocked,
                });
            }
            let mut outcome = cached;
            if let Some((_, query)) = &req.position
                && outcome.is_none()
            {
                let candidates = if req.filter == Filter::default() {
                    None
                } else {
                    let ignore = AtomicUsize::new(0);
                    Some(db.query(&req.filter, SortKey::Number, true, &ignore, cancel)?)
                };
                let found = db.search_position(candidates.as_deref(), query, progress, cancel)?;
                outcome = Some(Arc::new(found));
            }
            let mut in_order = None;
            let view = if req.use_position {
                let out = outcome.as_ref()?;
                let ordered = req.position.as_ref().is_some_and(|(_, q)| q.line.is_some());
                if ordered {
                    let same = out.matches.iter().filter(|m| m.same_order).count();
                    in_order = Some((same, out.matches.len()));
                }
                let mut v: Vec<u32> = out
                    .matches
                    .iter()
                    .filter(|m| !req.only_order || m.same_order)
                    .map(|m| m.index)
                    .collect();
                db.sort_records(&mut v, req.sort, req.ascending);
                v
            } else {
                db.query(&req.filter, req.sort, req.ascending, progress, cancel)?
            };
            Some(Answer {
                view,
                outcome,
                in_order,
                blocked: None,
            })
        });
        if let Some(work) = work {
            self.job = Some(Job { request, work });
        }
    }

    fn suggest(&mut self, db: &Arc<Database>, field: NameField, query: String) {
        if query.trim().chars().count() < 2 {
            self.suggestions = None;
            self.suggest_job = None;
            return;
        }
        let (tx, rx) = channel();
        let (db, q) = (db.clone(), query.clone());
        let spawned = std::thread::Builder::new()
            .name("db-suggest".into())
            .spawn(move || {
                let r = match field {
                    NameField::Event => db.suggest_events(&q, SUGGESTIONS),
                    _ => db.suggest_players(&q, SUGGESTIONS),
                };
                let _ = tx.send(r);
            });
        if spawned.is_ok() {
            self.suggest_job = Some(SuggestJob { field, query, rx });
        }
    }

    /// Progress of a slow pass (0–1), `None` when nothing slow is running.
    fn slow_progress(&self, now: f64, total: usize) -> Option<f32> {
        let j = self.job.as_ref()?;
        (now - j.work.started > SLOW)
            .then(|| (j.work.progress() as f32 / total.max(1) as f32).min(1.0))
    }

    /// The search board (an `egui::Panel` on the right of the library).
    pub fn board_panel(
        &mut self,
        ui: &mut Ui,
        db: &Arc<Database>,
        settings: &Settings,
        actions: &mut Vec<LibraryAction>,
    ) {
        self.update(db, ui.ctx());
        let total = db.len();
        // A PGN's first search builds its position table: the progress counts its games.
        self.board.preparing = !db.positions_ready();
        self.board.unavailable = self.blocked.is_some();
        let counting = self
            .job
            .as_ref()
            .map(|j| (j.work.progress() as f32 / total.max(1) as f32).min(1.0));
        if let Some((b, work)) = &self.booster {
            let p = (b.progress() as f32 / (*work).max(1) as f32).min(1.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(trf(
                        "Speeding up search in this database (once)… {p} %",
                        &[("p", &format!("{:.0}", p * 100.0))],
                    ))
                    .weak(),
                )
                .on_hover_text(tr(
                    "An index for fast position search is built once and kept in the cache. Searching works meanwhile, only slower.",
                ));
            });
        }
        if let Some(need) = self.blocked {
            let free = chess_import::memory::available().unwrap_or(0);
            let missing = (need + (256 << 20)).saturating_sub(free) / 1_000_000;
            ui.label(
                RichText::new(trf(
                    "Not enough memory for position search in this database: about {n} MB more is needed. Close other programs and try again.",
                    &[("n", &missing.max(1))],
                ))
                .color(ui.visuals().warn_fg_color),
            );
            if ui.button(tr("Retry")).clicked() {
                // The list is found anew; the memory is looked at again.
                self.shown = None;
                self.blocked = None;
            }
        }
        let outcome = self.board_outcome().cloned();
        let mut events = Vec::new();
        self.board.keys(ui.ctx(), outcome.as_ref(), &mut events);
        events.extend(self.board.panel(
            ui,
            settings,
            outcome.as_ref(),
            counting,
            self.table_hover,
            self.books,
        ));
        for e in events {
            match e {
                // A new position starts from the top of its list.
                BoardEvent::Changed => self.scroll_to_top = true,
                BoardEvent::Sound(s) => actions.push(LibraryAction::Sound(s)),
                BoardEvent::CopyText(t, m) => actions.push(LibraryAction::CopyText(t, m)),
                BoardEvent::OpenAsGame(pgn) => actions.push(LibraryAction::OpenPgn(pgn)),
            }
        }
        if !self.board.is_set() {
            self.only_order = false;
        }
    }

    /// The quick search of the header (§9.2): words found in a player, the tournament,
    /// the annotator, the ECO code or the date; with the panel and the position.
    fn search_field(&mut self, ui: &mut Ui, now: f64, width: f32) {
        // The × has its place also when there is nothing to clear: the field stays put.
        if self.form.search.is_empty() {
            ui.add_space(20.0);
        } else if crate::widgets::small_close_button(ui)
            .on_hover_text(tr("Clear"))
            .clicked()
        {
            self.form.search.clear();
            self.typed_at = f64::NEG_INFINITY;
        }
        let resp = ui
            .add(
                egui::TextEdit::singleline(&mut self.form.search)
                    .hint_text(if width < 200.0 {
                        tr("🔍 Search")
                    } else {
                        tr("🔍 Search: player, event, ECO, year")
                    })
                    .desired_width(width),
            )
            .on_hover_text(if width < 200.0 {
                format!(
                    "{}\n{}",
                    tr("🔍 Search: player, event, ECO, year"),
                    tr("Ctrl+F; again — the filters")
                )
            } else {
                tr("Ctrl+F; again — the filters").to_owned()
            });
        if resp.changed() {
            self.typed_at = now;
        }
        // Escape in a non-empty field clears it (egui has just taken the focus away);
        // in an empty one it leaves the field for the table.
        if resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Escape))
            && !self.form.search.is_empty()
        {
            self.form.search.clear();
            self.typed_at = f64::NEG_INFINITY;
            resp.request_focus();
        }
        if std::mem::take(&mut self.focus_search) {
            resp.request_focus();
            // The text selected: typing replaces it.
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                let range = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(self.form.search.chars().count()),
                );
                state.cursor.set_char_range(Some(range));
                state.store(ui.ctx(), resp.id);
            }
        }
        self.search_focused = resp.has_focus();
    }

    /// The pane switch `[Filters | Board]`; a click on the shown pane hides it.
    fn pane_switch(&mut self, ui: &mut Ui) {
        let active = self.form.active();
        let filters_shown = if self.narrow_filters {
            self.drawer_open
        } else {
            self.pane == Pane::Filters
        };
        let filters = if active > 0 && !filters_shown {
            trf("Filters ({n})", &[("n", &active)])
        } else {
            tr("Filters").to_string()
        };
        let board = if self.board.is_set() && self.pane != Pane::Board {
            format!("{} •", tr("Board"))
        } else {
            tr("Board").to_string()
        };
        // Right to left: the board segment first.
        ui.spacing_mut().item_spacing.x = 0.0;
        for (pane, label, hint, on) in [
            (
                Pane::Board,
                board,
                tr("Find games by position: play moves on the board (Ctrl+B)"),
                self.pane == Pane::Board,
            ),
            (
                Pane::Filters,
                filters,
                tr("Filters: players, tournament, years, rating… (Ctrl+F twice)"),
                filters_shown,
            ),
        ] {
            // The search by position is not in every version.
            if pane == Pane::Board && !crate::platform::CAPS.reference {
                continue;
            }
            let hint = if on { tr("Hide the panel") } else { hint };
            if ui.selectable_label(on, label).on_hover_text(hint).clicked() {
                match (pane, on) {
                    (Pane::Filters, false) => self.open_filters(),
                    (Pane::Filters, true) if self.narrow_filters => self.drawer_open = false,
                    (_, true) => self.pane = Pane::Hidden,
                    (_, false) => {
                        self.pane = pane;
                        self.drawer_open = false;
                    }
                }
            }
        }
    }

    /// The conditions in force, each removable; a click on one opens its pane.
    fn conditions(&self, settings: &Settings) -> Vec<Condition> {
        let f = &self.form;
        let mut out = Vec::new();
        if self.board.is_set() {
            let text = if self.only_order {
                trf(
                    "Moves: {line} (this order)",
                    &[("line", &self.board.chip_text())],
                )
            } else {
                trf("Position: {line}", &[("line", &self.board.chip_text())])
            };
            let blocked = self.blocked.is_some();
            out.push(Condition {
                text: if blocked { format!("{text} ⚠") } else { text },
                pane: Pane::Board,
                clear: Clear::Position,
                hint: blocked
                    .then(|| tr("Not applied: not enough memory for position search").to_string()),
                mini: Some((
                    self.board.position(),
                    self.board.flipped,
                    settings.board_theme,
                    settings.piece_set,
                )),
            });
        }
        if !f.search.trim().is_empty() {
            out.push(Condition {
                text: format!("{}: {}", tr("Search"), f.search.trim()),
                pane: self.pane,
                clear: Clear::Search,
                hint: None,
                mini: None,
            });
        }
        let side = match f.side {
            Side::White => format!(" · {}", tr("as White")),
            Side::Black => format!(" · {}", tr("as Black")),
            Side::Any => String::new(),
        };
        let mut add = |set: bool, text: String, clear: Clear| {
            if set {
                out.push(Condition {
                    text,
                    pane: Pane::Filters,
                    clear,
                    hint: None,
                    mini: None,
                });
            }
        };
        add(
            f.player.is_set(),
            format!("{}: {}{side}", tr("Player"), f.player.text.trim()),
            Clear::Player,
        );
        add(
            f.opponent.is_set(),
            format!("{}: {}", tr("Opponent"), f.opponent.text.trim()),
            Clear::Opponent,
        );
        add(
            f.event.is_set(),
            format!("{}: {}", tr("Tournament"), f.event.text.trim()),
            Clear::Event,
        );
        add(
            !f.annotator.trim().is_empty(),
            format!("{}: {}", tr("Annotator"), f.annotator.trim()),
            Clear::Annotator,
        );
        add(
            !f.year_from.trim().is_empty() || !f.year_to.trim().is_empty(),
            format!(
                "{}: {}–{}",
                tr("Years"),
                f.year_from.trim(),
                f.year_to.trim()
            ),
            Clear::Years,
        );
        add(
            !f.elo_min.trim().is_empty(),
            format!("Elo ≥ {}", f.elo_min.trim()),
            Clear::Elo,
        );
        add(
            !f.outcomes.is_empty(),
            tr("Result").to_string(),
            Clear::Result,
        );
        add(
            !f.eco.trim().is_empty(),
            format!("ECO {}", f.eco.trim()),
            Clear::Eco,
        );
        add(
            !f.moves_from.trim().is_empty() || !f.moves_to.trim().is_empty(),
            format!(
                "{}: {}–{}",
                tr("Length"),
                f.moves_from.trim(),
                f.moves_to.trim()
            ),
            Clear::Length,
        );
        add(
            f.with_comments,
            tr("with comments").to_string(),
            Clear::Comments,
        );
        add(
            f.with_variations,
            tr("with variations").to_string(),
            Clear::Variations,
        );
        add(f.hide_texts, tr("no texts").to_string(), Clear::Texts);
        out
    }

    fn clear(&mut self, what: Clear) {
        let f = &mut self.form;
        match what {
            Clear::Position => {
                self.board = SearchBoard::new();
                self.only_order = false;
            }
            Clear::Search => f.search.clear(),
            Clear::Player => {
                f.player = NameFilter::default();
                f.side = Side::Any;
                self.typed[0].clear();
            }
            Clear::Opponent => {
                f.opponent = NameFilter::default();
                self.typed[1].clear();
            }
            Clear::Event => {
                f.event = NameFilter::default();
                self.typed[2].clear();
            }
            Clear::Annotator => f.annotator.clear(),
            Clear::Years => {
                f.year_from.clear();
                f.year_to.clear();
            }
            Clear::Elo => f.elo_min.clear(),
            Clear::Result => f.outcomes.clear(),
            Clear::Eco => f.eco.clear(),
            Clear::Length => {
                f.moves_from.clear();
                f.moves_to.clear();
            }
            Clear::Comments => f.with_comments = false,
            Clear::Variations => f.with_variations = false,
            Clear::Texts => f.hide_texts = false,
        }
        self.typed_at = f64::NEG_INFINITY;
    }

    /// The line of conditions: position and filters, each with ×; on the right, how
    /// many games reached the position in the board's move order.
    fn chips(&mut self, ui: &mut Ui, settings: &Settings) {
        let conditions = self.conditions(settings);
        self.last_conditions = conditions.iter().map(|c| c.text.clone()).collect();
        let mut clear = None;
        let mut goto = None;
        let mut toggle_order = false;
        let chosen = self.marked.len();
        let hidden = if chosen >= 2 { self.hidden_marked() } else { 0 };
        let batch = self.batch.clone();
        let batch_error = self.batch_error.clone();
        let mut close_error = false;
        let mut clear_marked = false;
        let mut cancel_batch = false;
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right to left (docs/design/task-27-brilliant-search-2026-11-27.md §4.1):
            // the slot of the far right, the move order, the choice or its copy, the
            // conditions. The far right slot keeps one width whatever it shows, so what
            // stands left of it does not move when it changes.
            self.right_slot(ui);
            if let Some((same, all)) = self.in_order
                && self.board.is_set()
                && (same < all || self.only_order)
            {
                {
                    let link = if self.only_order {
                        tr("all move orders")
                    } else {
                        tr("only this order")
                    };
                    if ui
                        .link(link)
                        .on_hover_text(tr("Show only the games whose moves went exactly as on your board. Otherwise all games that reached this position by any path are shown."))
                        .clicked()
                    {
                        toggle_order = true;
                    }
                    ui.label(
                        RichText::new(trf(
                            "in this move order: {n} of {m} ·",
                            &[("n", &grouped(same)), ("m", &grouped(all))],
                        ))
                        .weak()
                        .size(12.0),
                    );
                }
            }
            // The choice, or the copy being written, in the same place (§42.3).
            if batch.is_some() || batch_error.is_some() || chosen >= 2 {
                ui.add_space(SLOT_GAP);
            }
            if let (None, Some(e)) = (&batch, &batch_error) {
                if crate::widgets::small_close_button(ui).clicked() {
                    close_error = true;
                }
                let width = (ui.available_width() - 140.0).max(80.0);
                ui.scope(|ui| {
                    ui.set_max_width(width);
                    ui.add(
                        egui::Label::new(RichText::new(e).color(ui.visuals().error_fg_color))
                            .truncate(),
                    )
                    .on_hover_text(e);
                });
            } else if let Some(b) = &batch {
                if b.cancelling {
                    ui.spinner();
                    ui.label(RichText::new(tr("Cancelling…")).weak());
                } else {
                    if ui.link(tr("Cancel")).clicked() {
                        cancel_batch = true;
                    }
                    let fraction = b.done as f32 / b.total.max(1) as f32;
                    ui.add(egui::ProgressBar::new(fraction).desired_width(80.0).desired_height(8.0));
                    ui.label(
                        RichText::new(trf(
                            "Writing to «{name}»: {i} of {n}",
                            &[("name", &b.name), ("i", &grouped(b.done)), ("n", &grouped(b.total))],
                        ))
                        .weak(),
                    );
                }
            } else if chosen >= 2 {
                if ui
                    .link(tr("Deselect"))
                    .on_hover_text(tr("Clear the selection (Esc)"))
                    .clicked()
                {
                    clear_marked = true;
                }
                let full = if hidden > 0 {
                    trf(
                        "Selected: {n} ({k} hidden by the filter)",
                        &[("n", &grouped(chosen)), ("k", &grouped(hidden))],
                    )
                } else {
                    trf("Selected: {n}", &[("n", &grouped(chosen))])
                };
                // Short in a narrow line: the conditions keep their 140 px.
                let short = ui.available_width()
                    < 140.0 + ui.fonts_mut(|f| {
                        f.layout_no_wrap(
                            full.clone(),
                            egui::TextStyle::Body.resolve(ui.style()),
                            Color32::WHITE,
                        )
                        .size()
                        .x
                    }) + 16.0;
                let text = if short { format!("{} ·", grouped(chosen)) } else { format!("{full} ·") };
                let label = ui.label(text);
                if hidden > 0 {
                    label.on_hover_text(format!(
                        "{full}\n{}",
                        tr("Games hidden by the filter will be copied too. Esc clears the selection")
                    ));
                } else if short {
                    label.on_hover_text(full);
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if conditions.is_empty() {
                    ui.label(RichText::new(tr("No conditions: all games are shown")).weak());
                }
                for (n, c) in conditions.iter().enumerate() {
                    // No room for another chip: the rest in a tooltip.
                    if ui.available_width() < 140.0 && n + 1 < conditions.len() {
                        let rest: Vec<&str> =
                            conditions[n..].iter().map(|c| c.text.as_str()).collect();
                        if ui
                            .link(trf("+{n} more", &[("n", &rest.len())]))
                            .on_hover_text(rest.join("\n"))
                            .clicked()
                        {
                            goto = Some(Pane::Filters);
                        }
                        break;
                    }
                    let (open, remove) = chip(ui, c);
                    if open && c.clear == Clear::Search {
                        self.focus_search = true;
                    } else if open {
                        goto = Some(c.pane);
                    }
                    if remove {
                        clear = Some(c.clear);
                    }
                }
            });
            });
        });
        if toggle_order {
            self.only_order = !self.only_order;
        }
        if clear_marked {
            self.mark_cursor_only();
        }
        if close_error {
            self.batch_error = None;
        }
        if cancel_batch {
            self.cancel_batch = true;
        }
        if let Some(c) = clear {
            self.clear(c);
        }
        match goto {
            Some(Pane::Filters) => self.open_filters(),
            Some(p) => self.pane = p,
            None => {}
        }
    }

    /// The far right slot of the conditions line: «!! Find great moves», or the search
    /// under way with «Stop», or why it stopped (task 27 §4.1–4.2). Its width is that of
    /// its longest form for the line's width, whatever it shows: what stands left of it
    /// never moves.
    fn right_slot(&mut self, ui: &mut Ui) {
        if !crate::platform::CAPS.great_moves {
            return;
        }
        let line = ui.available_width();
        let form = if line >= 1100.0 {
            2
        } else if line >= 800.0 {
            1
        } else {
            0
        };
        let body = egui::TextStyle::Body.resolve(ui.style());
        let width = |text: &str| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(text.to_owned(), body.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        };
        let progress_text = |i: usize, n: usize, left: &str| match form {
            2 => format!(
                "!! {}",
                trf(
                    "Analysed {i} of {n} · ≈ {time} left",
                    &[("i", &i), ("n", &n), ("time", &left)]
                )
            ),
            1 => format!(
                "!! {} · ≈ {left}",
                trf("{i} of {n}", &[("i", &i), ("n", &n)])
            ),
            _ => format!("!! {}", trf("{i} of {n}", &[("i", &i), ("n", &n)])),
        };
        let bar = if form == 2 { 90.0 } else { 60.0 };
        let pad = 2.0 * ui.spacing().button_padding.x;
        let gap = ui.spacing().item_spacing.x;
        let button_text = if form == 0 {
            tr("!! Find moves")
        } else {
            tr("!! Find great moves")
        };
        let longest = width(&progress_text(
            100,
            100,
            &crate::review::format_duration(59.0 * 60.0),
        )) + gap
            + bar
            + gap
            + width(tr("Stop"))
            + pad;
        let slot = longest.max(width(button_text) + pad);
        let pass = self.pass.clone();
        let blocked = self.search_blocked();
        let error = self.pass_error.clone();
        let mut find = false;
        let mut stop = false;
        let mut close_error = false;
        ui.allocate_ui_with_layout(
            vec2(slot, 22.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.set_min_width(slot);
                match (&pass, &error) {
                    (Some(p), _) => {
                        if p.stopping {
                            ui.spinner();
                            ui.label(RichText::new(tr("Stopping…")).weak());
                        } else {
                            if ui
                                .small_button(tr("Stop"))
                                .on_hover_text(tr("Stop the search; the reviewed games are kept"))
                                .clicked()
                            {
                                stop = true;
                            }
                            ui.add(
                                egui::ProgressBar::new(p.done as f32 / p.total.max(1) as f32)
                                    .desired_width(bar)
                                    .desired_height(8.0),
                            );
                            let left = crate::review::format_duration(p.left_secs);
                            let text = if p.here {
                                progress_text(p.done, p.total, &left)
                            } else {
                                format!(
                                    "!! {}",
                                    trf(
                                        "In «{db}»: {i} of {n}",
                                        &[("db", &p.name), ("i", &p.done), ("n", &p.total)]
                                    )
                                )
                            };
                            ui.add(egui::Label::new(RichText::new(text).weak()).truncate())
                                .on_hover_text(trf(
                                    "Analysed {i} of {n} · ≈ {time} left",
                                    &[("i", &p.done), ("n", &p.total), ("time", &left)],
                                ));
                        }
                    }
                    (None, Some(e)) => {
                        if crate::widgets::small_close_button(ui).clicked() {
                            close_error = true;
                        }
                        ui.add(
                            egui::Label::new(RichText::new(e).color(ui.visuals().error_fg_color))
                                .truncate(),
                        )
                        .on_hover_text(e);
                    }
                    (None, None) => {
                        let teal = crate::notation::assessment_text_color(
                            chess_core::Nag(3),
                            ui.visuals().dark_mode,
                        )
                        .unwrap_or(Color32::WHITE);
                        let mut job = egui::text::LayoutJob::default();
                        let rest = button_text.strip_prefix("!! ").unwrap_or(button_text);
                        let bold = egui::FontId::new(13.0, egui::FontFamily::Name("bold".into()));
                        let bold = if ui.fonts_mut(|f| f.families().contains(&bold.family)) {
                            bold
                        } else {
                            body.clone()
                        };
                        job.append("!! ", 0.0, egui::TextFormat::simple(bold, teal));
                        // The button's own text colour (it lightens under the pointer).
                        job.append(
                            rest,
                            0.0,
                            egui::TextFormat::simple(body.clone(), Color32::PLACEHOLDER),
                        );
                        let enabled = blocked.is_none();
                        let r = ui.add_enabled(enabled, egui::Button::new(job).small());
                        let r = match &blocked {
                            Some(why) => r.on_disabled_hover_text(why),
                            None => r.on_hover_text(trf(
                                "Review {games} with the engine and mark the !! and ! moves found (Shift+A)",
                                &[("games", &crate::i18n::games(self.view.len()))],
                            )),
                        };
                        if r.clicked() {
                            find = true;
                        }
                    }
                }
            },
        );
        ui.add_space(SLOT_GAP);
        if find {
            self.find_clicked = true;
        }
        if stop {
            self.stop_pass = true;
        }
        if close_error {
            self.pass_error = None;
        }
    }

    /// The filter panel (an `egui::Panel` on the right of the library).
    pub fn panel(&mut self, ui: &mut Ui, db: &Arc<Database>) {
        self.update(db, ui.ctx());
        let active = self.form.active();
        egui::Panel::bottom("filters_footer")
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.add_space(6.0);
                let label = if active > 0 {
                    trf("Reset filters ({n})", &[("n", &active)])
                } else {
                    tr("Reset filters").to_string()
                };
                if ui
                    .add_enabled(active > 0, egui::Button::new(label))
                    .on_hover_text("Ctrl+Shift+Backspace")
                    .clicked()
                {
                    self.reset();
                }
                ui.add_space(4.0);
            });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                self.players_group(ui, db);
                ui.add_space(4.0);
                self.event_group(ui, db);
                ui.add_space(4.0);
                self.game_group(ui);
                ui.add_space(4.0);
                self.annotations_group(ui, db.texts() > 0);
            });
    }

    /// Clears the conditions of the panel; the quick search and the position stay.
    fn reset(&mut self) {
        self.form = Form {
            search: std::mem::take(&mut self.form.search),
            ..Form::default()
        };
        self.suggestions = None;
        self.typed = Default::default();
    }

    fn group_header(ui: &mut Ui, title: &str, count: usize) {
        let text = if count > 0 {
            format!("{title} ({count})")
        } else {
            title.to_string()
        };
        ui.label(RichText::new(text).strong());
    }

    fn players_group(&mut self, ui: &mut Ui, db: &Arc<Database>) {
        let count =
            usize::from(self.form.player.is_set()) + usize::from(self.form.opponent.is_set());
        Self::group_header(ui, tr("Players"), count);
        self.name_field(ui, db, NameField::Player, tr("Player"));
        let player_set = self.form.player.is_set();
        ui.add_enabled_ui(player_set, |ui| {
            // Wrapped: longer words (French, Spanish) do not widen the panel.
            ui.horizontal_wrapped(|ui| {
                for (side, label) in [
                    (Side::Any, tr("either colour")),
                    (Side::White, tr("as White")),
                    (Side::Black, tr("as Black")),
                ] {
                    ui.selectable_value(&mut self.form.side, side, label);
                }
            })
            .response
            .on_disabled_hover_text(tr("Enter a player first"));
        });
        // Preparing against the player: with the name picked from the list, else the
        // dialog offers the names that match the text (opponent-prep.md §2.1).
        if player_set {
            let label = match self.form.player.exact {
                Some(_) => trf(
                    "♟ Prepare against {name}…",
                    &[("name", &self.form.player.text)],
                ),
                None => tr("♟ Prepare against an opponent…").to_owned(),
            };
            if ui
                .add(egui::Button::new(label).truncate())
                .on_hover_text(tr(
                    "The player's openings by colour, with names, and the opening errors the engine finds (Ctrl+Shift+P)",
                ))
                .clicked()
            {
                self.prepare = self.prep_entry();
            }
        }
        self.name_field(ui, db, NameField::Opponent, tr("Opponent"));
    }

    /// «Prepare against…» for the player of the filter: the name picked from the list
    /// (or the text typed) and the colour of the filter.
    pub fn prep_entry(&self) -> Option<PrepEntry> {
        self.form.player.is_set().then(|| PrepEntry {
            query: self.form.player.text.trim().to_owned(),
            exact: self.form.player.exact,
            side: self.form.side,
        })
    }

    fn event_group(&mut self, ui: &mut Ui, db: &Arc<Database>) {
        let count = usize::from(self.form.event.is_set())
            + usize::from(!self.form.annotator.trim().is_empty());
        Self::group_header(ui, tr("Tournament"), count);
        self.name_field(ui, db, NameField::Event, tr("Tournament or place"));
        ui.label(tr("Annotator"));
        let r = text_field(ui, &mut self.form.annotator, "", true);
        if r.changed() {
            self.typed_at = ui.input(|i| i.time);
        }
    }

    fn game_group(&mut self, ui: &mut Ui) {
        let f = &self.form;
        let count = usize::from(!f.year_from.trim().is_empty() || !f.year_to.trim().is_empty())
            + usize::from(!f.elo_min.trim().is_empty())
            + usize::from(!f.outcomes.is_empty())
            + usize::from(!f.eco.trim().is_empty())
            + usize::from(!f.moves_from.trim().is_empty() || !f.moves_to.trim().is_empty());
        Self::group_header(ui, tr("Game"), count);
        let now = ui.input(|i| i.time);
        let mut typed = false;

        // One grid (§2.6): captions in a column as wide as the longest of them, the
        // fields from one edge to one edge; "from – to" and Elo with its rule in halves.
        let font = egui::TextStyle::Body.resolve(ui.style());
        let label_w = [tr("Years"), tr("Elo from"), tr("Moves"), "ECO"]
            .iter()
            .map(|t| {
                ui.painter()
                    .layout_no_wrap((*t).to_string(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
            .ceil();
        let fields = (ui.available_width() - label_w - 8.0 - CLEAR_SLOT).floor();
        let half = ((fields - DASH - 8.0) / 2.0).floor().max(40.0);

        let years_ok = {
            let a = number(&self.form.year_from, 1..=2100);
            let b = number(&self.form.year_to, 1..=2100);
            match (a, b) {
                (Ok(Some(a)), Ok(Some(b))) => (a <= b, a <= b),
                (a, b) => (a.is_ok(), b.is_ok()),
            }
        };
        form_row(ui, label_w, tr("Years"), |ui| {
            typed |= number_field(ui, &mut self.form.year_from, "", years_ok.0, half).changed();
            dash(ui, "–");
            typed |= number_field(ui, &mut self.form.year_to, "", years_ok.1, half).changed();
        });

        form_row(ui, label_w, tr("Elo from"), |ui| {
            let ok = number(&self.form.elo_min, 1..=3500).is_ok();
            typed |= number_field(ui, &mut self.form.elo_min, "", ok, half).changed();
            dash(ui, "");
            let player_set = self.form.player.is_set();
            if !player_set && self.form.elo_rule == EloRule::Opponent {
                self.form.elo_rule = EloRule::Both;
            }
            let label = |r: EloRule| match r {
                EloRule::Both => tr("both"),
                EloRule::Either => tr("either"),
                EloRule::Average => tr("average"),
                EloRule::Opponent => tr("opponent"),
            };
            egui::ComboBox::from_id_salt("elo_rule")
                .selected_text(label(self.form.elo_rule))
                .width(half)
                .show_ui(ui, |ui| {
                    let mut rules = vec![EloRule::Both, EloRule::Either, EloRule::Average];
                    if player_set {
                        rules.push(EloRule::Opponent);
                    }
                    for r in rules {
                        ui.selectable_value(&mut self.form.elo_rule, r, label(r));
                    }
                });
        });

        let moves_ok = {
            let a = number(&self.form.moves_from, 0..=999);
            let b = number(&self.form.moves_to, 0..=999);
            match (a, b) {
                (Ok(Some(a)), Ok(Some(b))) => (a <= b, a <= b),
                (a, b) => (a.is_ok(), b.is_ok()),
            }
        };
        form_row(ui, label_w, tr("Moves"), |ui| {
            typed |= number_field(ui, &mut self.form.moves_from, "", moves_ok.0, half).changed();
            dash(ui, "–");
            typed |= number_field(ui, &mut self.form.moves_to, "", moves_ok.1, half).changed();
        });

        form_row(ui, label_w, "ECO", |ui| {
            let ok = self.form.eco.trim().is_empty() || parse_eco(&self.form.eco).is_some();
            let r = text_field(ui, &mut self.form.eco, tr("B90, B9 or C60-C99"), ok);
            let r = if ok {
                r
            } else {
                r.on_hover_text(tr(
                    "A code (B90), a prefix (B9) or a range (C60-C99), separated by commas",
                ))
            };
            typed |= r.changed();
        });

        ui.label(tr("Result"));
        let relative = self.form.player.is_set();
        if relative != self.form.relative_outcomes {
            // "1-0" and "Win" mean different things: do not carry one over as the other.
            self.form.relative_outcomes = relative;
            self.form.outcomes.clear();
        }
        ui.horizontal_wrapped(|ui| {
            let buttons: [(Outcome, &str); 4] = if relative {
                [
                    (Outcome::WhiteWins, tr("Win")),
                    (Outcome::Draw, tr("Draw")),
                    (Outcome::BlackWins, tr("Loss")),
                    (Outcome::Unknown, "*"),
                ]
            } else {
                [
                    (Outcome::WhiteWins, "1-0"),
                    (Outcome::Draw, "½-½"),
                    (Outcome::BlackWins, "0-1"),
                    (Outcome::Unknown, "*"),
                ]
            };
            for (o, label) in buttons {
                let on = self.form.outcomes.contains(&o);
                let text = if on {
                    RichText::new(label).strong()
                } else {
                    RichText::new(label)
                };
                if ui.selectable_label(on, text).clicked() {
                    if on {
                        self.form.outcomes.retain(|x| *x != o);
                    } else {
                        self.form.outcomes.push(o);
                    }
                }
            }
        });
        if typed {
            self.typed_at = now;
        }
    }

    fn annotations_group(&mut self, ui: &mut Ui, texts: bool) {
        let f = &self.form;
        let count = usize::from(f.with_comments)
            + usize::from(f.with_variations)
            + usize::from(f.hide_texts);
        Self::group_header(ui, tr("Annotations"), count);
        ui.checkbox(&mut self.form.with_comments, tr("with comments"));
        ui.checkbox(&mut self.form.with_variations, tr("with variations"));
        // Only a database with guiding texts has the choice (§9.4).
        if texts {
            let mut show_texts = !self.form.hide_texts;
            ui.checkbox(&mut show_texts, tr("show texts"))
                .on_hover_text(tr("Guiding texts of the database; left out by any condition on players, dates, ratings, results or moves"));
            self.form.hide_texts = !show_texts;
        }
    }

    /// A name field with suggestions; picking one filters by exactly that name.
    fn name_field(&mut self, ui: &mut Ui, db: &Arc<Database>, field: NameField, caption: &str) {
        let slot = field as usize;
        let value = match field {
            NameField::Player => &mut self.form.player,
            NameField::Opponent => &mut self.form.opponent,
            NameField::Event => &mut self.form.event,
        };
        // "=" on the caption's line: the field itself does not move when a name is picked.
        ui.horizontal(|ui| {
            ui.label(caption);
            if value.exact.is_some() {
                ui.label(RichText::new("=").strong()).on_hover_text(tr(
                    "Exactly this name; edit the text to search by words again",
                ));
            }
        });
        let mut picked_this_frame = None;
        let resp = ui
            .horizontal(|ui| {
                let r = text_field(ui, &mut value.text, "", true);
                if field == NameField::Player && std::mem::take(&mut self.focus_player) {
                    r.request_focus();
                    // Select the text so typing replaces it.
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                        let range = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(value.text.chars().count()),
                        );
                        state.cursor.set_char_range(Some(range));
                        state.store(ui.ctx(), r.id);
                    }
                }
                r
            })
            .inner;
        let now = ui.input(|i| i.time);
        if resp.changed() {
            value.exact = None;
            self.typed_at = now;
        }
        let text = value.text.clone();
        let typing = self.typed[slot] != text;
        if typing {
            self.typed[slot] = text.clone();
            if resp.has_focus() {
                self.suggest(db, field, text.clone());
            }
        }
        // A click on the list takes the focus from the field before the click is
        // over: the list stays open while the pointer is on it.
        let mine = self.suggestions.as_ref().is_some_and(|s| s.field == field);
        let keep = resp.has_focus() || (mine && self.pointer_on_list);
        let open = keep
            && self
                .suggestions
                .as_ref()
                .is_some_and(|s| s.field == field && s.query == text && !s.list.is_empty());
        if !open {
            if mine && !keep {
                self.suggestions = None;
                self.pointer_on_list = false;
            }
            return;
        }
        // Keyboard in the list: arrows move, Enter picks, Escape closes.
        let (down, up, esc) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let s = self.suggestions.as_mut().expect("open list");
        let n = s.list.len();
        if down {
            s.highlighted = Some(s.highlighted.map_or(0, |h| (h + 1).min(n - 1)));
        }
        if up {
            s.highlighted = s.highlighted.and_then(|h| h.checked_sub(1));
        }
        if esc {
            self.suggestions = None;
            return;
        }
        if let Some(h) = s.highlighted
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            picked_this_frame = s.list.get(h).cloned();
        }
        let area = egui::Area::new(egui::Id::new(("suggest", slot)))
            .order(egui::Order::Foreground)
            .fixed_pos(resp.rect.left_bottom() + vec2(0.0, 2.0));
        let width = resp.rect.width().max(220.0);
        let shown = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(width);
                for (i, sug) in s.list.iter().enumerate() {
                    let label = if sug.games > 0 {
                        format!("{} — {}", sug.name, grouped(sug.games as usize))
                    } else {
                        sug.name.clone()
                    };
                    let r = ui.add(
                        egui::Button::selectable(s.highlighted == Some(i), label)
                            .truncate()
                            .min_size(vec2(width, 0.0)),
                    );
                    if r.clicked() {
                        picked_this_frame = Some(sug.clone());
                    }
                }
                if s.total > s.list.len() {
                    ui.label(
                        RichText::new(trf(
                            "{n} more matches…",
                            &[("n", &grouped(s.total - s.list.len()))],
                        ))
                        .weak(),
                    );
                }
            });
        });
        self.pointer_on_list = shown.response.contains_pointer();
        if let Some(sug) = picked_this_frame {
            let value = match field {
                NameField::Player => &mut self.form.player,
                NameField::Opponent => &mut self.form.opponent,
                NameField::Event => &mut self.form.event,
            };
            value.text = sug.name.clone();
            value.exact = Some(sug.id);
            self.typed[slot] = sug.name;
            self.suggestions = None;
            self.pointer_on_list = false;
            resp.request_focus();
            // Picking applies at once.
            self.typed_at = f64::NEG_INFINITY;
        }
    }

    /// The header line and the table (in the central panel).
    #[allow(clippy::too_many_arguments)]
    pub fn table(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        db: &Arc<Database>,
        header: &Header,
        viewed: &mut Viewed,
        notes: usize,
        settings: &Settings,
        writable: bool,
        actions: &mut Vec<LibraryAction>,
    ) -> bool {
        let mut open_notes = false;
        self.update(db, ui.ctx());
        self.writable = writable;
        let now = ui.input(|i| i.time);
        let progress = self.slow_progress(now, db.len());
        let total = db.games() + db.texts();
        // A database with no games yet (own-databases.md §2.3): how to put one in.
        if total == 0 && writable {
            let name = crate::own_db::base_name(path);
            // The header as for any database; nothing to search, so no field and no switch,
            // and «+ Game» once: the main button in the middle.
            ui.horizontal(|ui| {
                ui.label(RichText::new(&name).strong());
                ui.label(RichText::new(format!("· {}", header.format)).weak());
                ui.label(format!("· {}", crate::i18n::games(0)));
            });
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.label(
                    RichText::new(trf("No games in «{name}» yet", &[("name", &name)])).size(16.0),
                );
                ui.add_space(10.0);
                if crate::widgets::primary(ui, tr("+ Game"))
                    .on_hover_text(new_game_hint(&name))
                    .clicked()
                {
                    actions.push(LibraryAction::NewGameIn(path.to_path_buf()));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr(
                        "«+ Game» opens an empty board; Ctrl+S adds the game here. Or open any game and choose «Add to database…» (Ctrl+Shift+S)",
                    ))
                    .weak(),
                );
                ui.add_space(6.0);
                ui.label(RichText::new(trf("File: {path}", &[("path", &crate::platform::files::shown(path))])).weak());
            });
            return open_notes;
        }

        // The header (docs/design/narrow-windows.md §2.4), by importance: the pane switch,
        // the quick search (220, down to 140), the counter, "⟳ New games", the import
        // notes, the database's name (shortened, at least 80, else left out: it is on
        // the shelf too), its format. Measured from the texts; what does not fit on one
        // line goes to a second.
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let format = format!("· {}", header.format);
        let counter = match &self.shown {
            Some(s)
                if (s.use_position && self.blocked.is_none())
                    || s.filter != Filter::default()
                    || self.form.active() > 0 =>
            {
                trf(
                    "· found {n} of {total}",
                    &[("n", &grouped(self.view.len())), ("total", &grouped(total))],
                )
            }
            _ if db.texts() > 0 => format!(
                "· {} · {}",
                crate::i18n::games(db.games()),
                crate::i18n::texts(db.texts())
            ),
            _ => format!("· {}", crate::i18n::games(db.games())),
        };
        // The board alone in a narrow window: the counter leads back to the list.
        let counter = if self.board_alone() {
            format!("{} ›", counter.trim_start_matches("· "))
        } else {
            counter
        };
        let gap = ui.spacing().item_spacing.x;
        let pad = 2.0 * ui.spacing().button_padding.x;
        let body = |t: &str| text_w(ui, t, egui::TextStyle::Body);
        let switch_w = {
            let active = self.form.active();
            body(&trf("Filters ({n})", &[("n", &active)]))
                + body(&format!("{} •", tr("Board")))
                + 2.0 * pad
        };
        let search_w = |field: f32| 20.0 + gap + field + 8.0 + 8.0 + gap;
        let info_w = body(&counter)
            + gap
            + progress.map_or(0.0, |_| 14.0 + gap + body("100 %") + gap)
            + if notes > 0 {
                body(&format!("ℹ {notes}")) + pad + gap
            } else {
                0.0
            }
            + header.online.as_ref().map_or(0.0, |o| {
                if o.busy {
                    14.0 + gap + body(tr("updating…")) + gap
                } else {
                    text_w(ui, tr("⟳ New games"), egui::TextStyle::Small) + pad + gap
                }
            });
        let name_w = text_w(ui, &name, egui::TextStyle::Body) + gap;
        let format_w = body(&format) + gap;
        let avail = ui.available_width();
        // «+ Game» (a database that takes games only), shortened to «+» after the field.
        let plus_w = |full: bool| {
            if !writable {
                0.0
            } else if full {
                body(tr("+ Game")) + pad + 8.0 + gap
            } else {
                body("+") + pad + 8.0 + gap
            }
        };
        let right = |field: f32, full: bool| switch_w + 8.0 + search_w(field) + plus_w(full);
        // One line: the widest field that leaves the counter and at least 80 of the name.
        // (field, format, the name's least room: 0 leaves the name out, «+ Game» in full)
        let one_line = [
            (220.0, true, name_w, true),
            (220.0, false, 80.0 + gap, true),
            (140.0, false, 80.0 + gap, true),
            (140.0, false, 80.0 + gap, false),
            (140.0, false, 0.0, false),
        ]
        .into_iter()
        .find(|(field, with_format, name_least, full)| {
            let format = if *with_format { format_w } else { 0.0 };
            avail - right(*field, *full) - info_w - format >= *name_least
        })
        .map(|(field, with_format, _, full)| (field, with_format, full));
        let (field, plus_full) = match one_line {
            Some((field, _, full)) => (field, full),
            None if avail - right(220.0, true) >= 80.0 + gap => (220.0, true),
            None => (140.0, false),
        };
        let with_format = one_line.is_some_and(|(_, f, _)| f);
        let mut back_to_list = false;
        let mut new_game = false;
        let alone = self.board_alone();
        // On a line of its own the counter loses its leading "· ".
        // The name is drawn when it has its own width or at least 80 (shortened).
        let name_shown = |room: f32| room >= name_w.min(80.0 + gap);
        let name_room = if one_line.is_some() {
            avail - right(field, plus_full) - info_w - if with_format { format_w } else { 0.0 }
        } else {
            avail - right(field, plus_full)
        };
        let shown = name_shown(name_room);
        // What starts the line loses its leading "· ".
        let counter_text = if one_line.is_some() && (shown || with_format) {
            counter.as_str()
        } else {
            counter.trim_start_matches("· ")
        };
        let mut info = |ui: &mut Ui| {
            let mut text = RichText::new(counter_text);
            if progress.is_some() {
                text = text.weak();
            }
            if alone {
                if ui
                    .link(text)
                    .on_hover_text(tr("Show the list of games (Esc)"))
                    .clicked()
                {
                    back_to_list = true;
                }
            } else {
                ui.label(text);
            }
            if let Some(p) = progress {
                ui.label(RichText::new(format!("{:.0} %", p * 100.0)).weak());
            }
            if notes > 0 && crate::widgets::notes_button(ui, notes).clicked() {
                open_notes = true;
            }
            if header.keep_copy
                && ui
                    .add(egui::Button::new(tr("📋 Keep a copy in the browser")).small())
                    .on_hover_text(tr("The copy opens right away, without choosing the file. It is stored in this browser on this device and is lost if you clear the site's data. The file on disk is not changed."))
                    .clicked()
            {
                actions.push(LibraryAction::KeepCopy(path.to_path_buf()));
            }
            // A database from Lichess and Chess.com: its new games.
            if let Some(online) = &header.online {
                if online.busy {
                    ui.label(RichText::new(tr("updating…")).weak());
                } else if ui
                    .add_enabled(online.enabled, egui::Button::new(tr("⟳ New games")).small())
                    .on_hover_text(&online.hint)
                    .on_disabled_hover_text(&online.hint)
                    .clicked()
                {
                    #[cfg(not(web))]
                    actions.push(LibraryAction::UpdateOnline(path.to_path_buf()));
                }
            }
        };
        ui.horizontal(|ui| {
            if shown {
                ui.allocate_ui(
                    vec2((name_room - gap).floor(), ui.spacing().interact_size.y),
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(&name).strong()).truncate())
                            .on_hover_text(&name);
                    },
                );
            }
            if with_format {
                let format = if shown {
                    format.as_str()
                } else {
                    format.trim_start_matches("· ")
                };
                ui.label(RichText::new(format).weak())
                    .on_hover_text(&header.format_hint);
            }
            if one_line.is_some() {
                info(ui);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The switch packs its segments; the rest keeps the usual spacing.
                ui.scope(|ui| self.pane_switch(ui));
                ui.add_space(8.0);
                self.search_field(ui, now, field);
                if writable {
                    ui.add_space(8.0 - ui.spacing().item_spacing.x);
                    let label = if plus_full { tr("+ Game") } else { "+" };
                    if ui
                        .button(label)
                        .on_hover_text(new_game_hint(&name))
                        .clicked()
                    {
                        new_game = true;
                    }
                }
            });
        });
        if one_line.is_none() {
            ui.horizontal_wrapped(|ui| info(ui));
        }
        if back_to_list {
            self.pane = Pane::Hidden;
        }
        if new_game {
            actions.push(LibraryAction::NewGameIn(path.to_path_buf()));
        }
        self.chips(ui, settings);
        ui.add_space(2.0);
        self.drawer_top = ui.cursor().top();

        // Ctrl+F: the quick search; once more, the filters' first field (§9.10).
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            if self.search_focused {
                self.open_filters();
                self.focus_player = true;
            } else {
                self.focus_search = true;
            }
        }
        if crate::platform::CAPS.reference
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B))
        {
            self.pane = Pane::Board;
        }
        // Ctrl+Shift+Backspace: all conditions, the quick search and the position included.
        if ui.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Backspace,
            )
        }) {
            self.reset();
            self.form.search.clear();
            self.board = SearchBoard::new();
            self.only_order = false;
        }

        // A narrow window: the board takes the place of the table (§2.3).
        if self.board_alone() {
            self.board_panel(ui, db, settings, actions);
            return open_notes;
        }

        // A position that could not be searched (no memory) does not filter the list.
        let use_position =
            self.shown.as_ref().is_some_and(|s| s.use_position) && self.blocked.is_none();
        // The position the shown rows were found for (the board may be ahead of it).
        let shown_position = self
            .shown
            .as_ref()
            .filter(|s| s.use_position && self.blocked.is_none())
            .and_then(|s| s.position.as_ref())
            .map(|(_, q)| q.position.clone());
        if self.shown.is_some() && self.view.is_empty() && self.job.is_none() {
            let searched = !self.form.search.trim().is_empty();
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                if use_position {
                    ui.label(
                        RichText::new(tr("The position does not occur in these games")).size(16.0),
                    );
                    ui.add_space(6.0);
                    if (self.form.active() > 0 || searched)
                        && ui.button(tr("Search the whole database")).clicked()
                    {
                        self.reset();
                        self.form.search.clear();
                    }
                } else {
                    ui.label(RichText::new(tr("Nothing found")).size(16.0));
                    // A condition on a field no game of the database has (§9.8).
                    let filled = db.filled();
                    let f = &self.form;
                    for (asked, has, text) in [
                        (
                            !f.eco.trim().is_empty(),
                            filled.eco,
                            tr("No game of this database has an ECO code"),
                        ),
                        (
                            !f.annotator.trim().is_empty(),
                            filled.annotator,
                            tr("No game of this database has an annotator"),
                        ),
                        (
                            !f.elo_min.trim().is_empty(),
                            filled.elo,
                            tr("No game of this database has a rating"),
                        ),
                        (
                            !f.year_from.trim().is_empty() || !f.year_to.trim().is_empty(),
                            filled.year,
                            tr("No game of this database has a year"),
                        ),
                    ] {
                        if asked && !has {
                            ui.label(RichText::new(text).weak());
                        }
                    }
                    ui.add_space(6.0);
                    let label = match (self.form.active() > 0, searched) {
                        (true, true) => tr("Reset filters and search"),
                        (false, true) => tr("Reset search"),
                        _ => tr("Reset filters"),
                    };
                    if ui.button(label).clicked() {
                        self.reset();
                        self.form.search.clear();
                    }
                }
            });
            // Shift+A on an empty list still says why nothing can be searched.
            self.handle_find(ui, path, actions);
            return open_notes;
        }

        self.handle_find(ui, path, actions);
        self.update_marks_sides(db, ui.ctx());
        // `CP_MARKS_DEMO=1`: made-up marks for the first rows (screenshots of the column).
        if std::env::var_os("CP_MARKS_DEMO").is_some()
            && self.shown.is_some()
            && self.job.is_none()
            && let Some(marks) = self.marks.clone()
        {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| demo_marks(db, &self.view, &marks));
        }
        // `CP_BRILLIANT_RUN=1`: the search asked for once the list is there.
        if std::env::var_os("CP_BRILLIANT_RUN").is_some()
            && self.shown.is_some()
            && self.job.is_none()
            && self.search_blocked().is_none()
        {
            static ONCE: std::sync::Once = std::sync::Once::new();
            let mut first = false;
            ONCE.call_once(|| first = true);
            if first {
                self.ask_search(path, actions);
            }
        }
        // Records the database does not have (`CP_SELECT` on a short one) are not chosen.
        if self.marked.last().is_some_and(|r| r >= db.len()) {
            let len = db.len();
            self.mark(|m| m.retain_below(len));
            self.selected = self.selected.filter(|&c| c < len);
        }
        self.keyboard(ui, path, actions);
        // `CP_BATCH=new:<name>`: the chosen records copied into a new database once the
        // list is there (screenshots, checks).
        if std::env::var("CP_BATCH").is_ok_and(|b| b.starts_with("new:"))
            && !self.marked.is_empty()
            && !self.view.is_empty()
        {
            static ONCE: std::sync::Once = std::sync::Once::new();
            let mut first = false;
            ONCE.call_once(|| first = true);
            if first {
                self.ask_copy(path, actions);
            }
        }

        let scroll_row = if std::mem::take(&mut self.scroll_to_selected) {
            self.selected
                .and_then(|r| self.view.iter().position(|&i| i as usize == r))
        } else if std::mem::take(&mut self.scroll_to_top) {
            Some(0)
        } else {
            None
        };
        // Narrow windows drop the least needed columns first.
        // Narrow windows drop the least needed columns first (round, number, marks,
        // length, ECO, event); the date becomes the year.
        let width = ui.available_width();
        let show_round = width >= 1100.0;
        let show_number = !use_position || width >= 1100.0;
        let show_moves = width >= 1000.0;
        let show_marks = width >= 1000.0;
        let show_eco = width >= 860.0;
        let show_event = width >= 720.0;
        let year_only = width < 1000.0;
        let mut columns: Vec<(&str, Option<SortKey>, Column)> = Vec::new();
        if use_position {
            columns.push((tr("Move"), None, Column::exact(MOVE_COL)));
        }
        if show_number {
            columns.push(("№", Some(SortKey::Number), Column::exact(number_col(db))));
        }
        // Without the event the names share the rest of the width.
        let name = if show_event {
            Column::initial(170.0).at_least(NAME_MIN).clip(true)
        } else {
            Column::remainder().at_least(NAME_MIN).clip(true)
        };
        columns.extend([
            (tr("White"), Some(SortKey::White), name),
            ("Elo", Some(SortKey::WhiteElo), Column::exact(ELO_COL)),
            (tr("Black"), Some(SortKey::Black), name),
            ("Elo", Some(SortKey::BlackElo), Column::exact(ELO_COL)),
            (tr("Res."), Some(SortKey::Result), Column::exact(RESULT_COL)),
            // The marks of the search for great moves: in every layout (task 27 §4.3).
            ("!!", None, Column::exact(MARKS_COL)),
        ]);
        if show_moves {
            columns.push((tr("Length"), Some(SortKey::Moves), Column::exact(56.0)));
        }
        if show_eco {
            columns.push(("ECO", Some(SortKey::Eco), Column::exact(44.0)));
        }
        if show_marks {
            columns.push(("💬", None, Column::exact(40.0)));
        }
        if show_event {
            columns.push((
                tr("Event"),
                Some(SortKey::Event),
                Column::remainder().at_least(80.0).clip(true),
            ));
        }
        if show_round {
            columns.push((tr("Round"), None, Column::exact(48.0)));
        }
        if year_only {
            columns.push((tr("Year"), Some(SortKey::Date), Column::exact(YEAR_COL)));
        } else {
            columns.push((tr("Date"), Some(SortKey::Date), Column::exact(88.0)));
        }

        if progress.is_some() {
            ui.set_opacity(0.6);
        }
        // Each set of columns keeps its own widths: a resized window does not leave the
        // names at the widths of another layout.
        let layout = (
            show_round,
            show_number,
            show_moves,
            show_marks,
            show_eco,
            show_event,
            year_only,
            use_position,
        );
        let frame_color = ui.visuals().selection.stroke.color;
        let table_clip = ui.clip_rect().intersect(ui.max_rect());
        let spacing = ui.spacing().item_spacing;
        let mut table = TableBuilder::new(ui)
            .id_salt(("game table", layout))
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for (_, _, c) in &columns {
            table = table.column(*c);
        }
        if let Some(r) = scroll_row {
            table = table.scroll_to_row(r, Some(egui::Align::Center));
        }
        let mut sort_click = None;
        let mut clicked = None;
        let mut opened = None;
        let mut toggle_viewed = None;
        let mut hovered = None;
        let marks_sorted = self.marks_sort;
        let mut marks_click = false;
        let pass_current = self
            .pass
            .as_ref()
            .filter(|p| p.here)
            .and_then(|p| p.current);
        let (marks_ref, shown_ref, sides_ref) =
            (&self.marks, &self.shown_marks, &self.marks_sides.1);
        let view = &self.view;
        let selected = self.selected;
        let marked = &self.marked;
        let many = marked.len() >= 2;
        let batch_running = self.batch.as_ref().map(|b| b.name.clone());
        let mut batch_menu = None;
        let mut secondary = None;
        // A position not applied (no memory) does not sort: the arrow shows the view's own order.
        let sort = if self.blocked.is_some() {
            self.sort
        } else {
            self.effective_sort()
        };
        let outcome = self.outcome.clone().filter(|_| use_position);
        let ordered = self
            .shown
            .as_ref()
            .and_then(|s| s.position.as_ref())
            .is_some_and(|(_, q)| q.line.is_some());
        let find = |index: usize| -> Option<Match> {
            let out = outcome.as_ref()?;
            let i = out
                .matches
                .binary_search_by_key(&(index as u32), |m| m.index)
                .ok()?;
            out.matches.get(i).copied()
        };
        table
            .header(26.0, |mut header| {
                for (label, key, _) in &columns {
                    header.col(|ui| {
                        if *label == "!!" {
                            let r = crate::widgets::sort_header(ui, label, marks_sorted.then_some(true))
                                .on_hover_text(tr("Brilliant and great moves found by the engine: sort by them"));
                            if r.clicked() {
                                marks_click = true;
                            }
                            return;
                        }
                        let active = key.filter(|k| sort.0 == *k && !marks_sorted).map(|_| sort.1);
                        let r = crate::widgets::sort_header(ui, label, active);
                        if let Some(k) = key
                            && r.clicked()
                        {
                            sort_click = Some(*k);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, view.len(), |mut row| {
                    let Some(&index) = view.get(row.index()) else {
                        return;
                    };
                    let index = index as usize;
                    let r = db.row(index);
                    let is_marked = marked.contains(index);
                    row.set_selected(is_marked);
                    // Where the row's cells are: the cursor's frame follows the row's fill.
                    let mut cells = egui::Rect::NOTHING;
                    let key = viewed::key(db.identity(index));
                    let seen = viewed.contains(key);
                    let elo = |e: u16| if e > 0 { e.to_string() } else { String::new() };
                    let is_text = r.kind == Some(RecordKind::Text);
                    let found = find(index);
                    let result = match r.result {
                        _ if is_text => "",
                        "1/2-1/2" => "½-½",
                        x => x,
                    };
                    for (label, key_, _) in &columns {
                        row.col(|ui| {
                            cells = cells.union(ui.max_rect());
                            let cell = match (*label, *key_) {
                                (l, None) if l == tr("Move") => {
                                    let Some(m) = found else { return };
                                    let mut text = board_search::ply_label(m.ply);
                                    if ordered && !m.same_order {
                                        text = format!("↪{text}");
                                    }
                                    if m.in_variation {
                                        text.push_str(" 🔀");
                                    }
                                    let resp = ui.add(egui::Label::new(text).selectable(false));
                                    resp.on_hover_text(tr("The move after which the position arose in the game. ↪ — reached by another move order, 🔀 — in a variation"));
                                    return;
                                }
                                (_, Some(SortKey::Number)) => {
                                    if seen {
                                        check_mark(ui);
                                    }
                                    ui.add_space(14.0);
                                    let n = RichText::new(grouped(index + 1)).weak();
                                    // A game that could not be read cleanly: its number is orange.
                                    if db.damaged(index) {
                                        n.color(Color32::from_rgb(0xe6, 0x8f, 0x00))
                                    } else {
                                        n
                                    }
                                }
                                (_, Some(SortKey::White)) if is_text => {
                                    RichText::new(format!("📄 {}", r.title)).italics()
                                }
                                (_, Some(SortKey::White)) => RichText::new(r.white.as_str()),
                                (_, Some(SortKey::WhiteElo)) => RichText::new(elo(r.white_elo)),
                                (_, Some(SortKey::Black)) => RichText::new(r.black.as_str()),
                                (_, Some(SortKey::BlackElo)) => RichText::new(elo(r.black_elo)),
                                (_, Some(SortKey::Result)) => RichText::new(result),
                                ("!!", None) => {
                                    marks_cell(ui, Self::shown_mark(marks_ref, shown_ref, sides_ref, db, index as u32), pass_current == Some(index as u32), is_marked);
                                    return;
                                }
                                (_, Some(SortKey::Moves)) => RichText::new(match r.moves {
                                    0 => String::new(),
                                    255 => "255+".into(),
                                    m => m.to_string(),
                                }),
                                (_, Some(SortKey::Eco)) if r.chess960 => {
                                    let resp = ui.add(egui::Label::new("960").selectable(false));
                                    resp.on_hover_text(tr("Chess960: the moves cannot be read; the header and result are shown"));
                                    return;
                                }
                                (_, Some(SortKey::Eco)) => {
                                    RichText::new(r.eco.clone().unwrap_or_default())
                                }
                                ("💬", None) => {
                                    let marks = format!(
                                        "{}{}",
                                        if r.has_comments { "💬" } else { "" },
                                        if r.has_variations { "🔀" } else { "" }
                                    );
                                    RichText::new(marks).weak()
                                }
                                (_, Some(SortKey::Event)) => RichText::new(r.event.as_str()).italics(),
                                (_, Some(SortKey::Date)) if year_only => {
                                    RichText::new(r.date.get(..4).filter(|y| !y.contains('?')).unwrap_or(""))
                                }
                                (_, Some(SortKey::Date)) => RichText::new(crate::library::display_date(&r.date)),
                                _ => RichText::new(if r.round == "?" { "" } else { r.round.as_str() }).weak(),
                            };
                            ui.add(egui::Label::new(cell).selectable(false).truncate());
                        });
                    }
                    // Tournament · place · round · length · annotator, those it has.
                    let moves = if r.moves > 0 { trf("moves: {n}", &[("n", &r.moves)]) } else { String::new() };
                    let round = if r.round.trim().is_empty() || r.round == "?" {
                        String::new()
                    } else {
                        format!("{} {}", tr("Round").to_lowercase(), r.round)
                    };
                    let about: Vec<&str> = [r.event.as_str(), r.site.as_str(), round.as_str(), moves.as_str(), r.annotator.as_str()]
                        .into_iter()
                        .filter(|t| !t.trim().is_empty() && *t != "?" && !t.contains("://"))
                        .collect();
                    let resp = row.response();
                    // The cursor among several chosen rows, or on a row not chosen: a frame
                    // (a shape, not a colour; §42.4).
                    if selected == Some(index) && (many || !is_marked) {
                        ui_painter_frame(&resp, cells.expand2(spacing / 2.0), frame_color, table_clip);
                    }
                    let resp = if about.is_empty() { resp } else { resp.on_hover_text(about.join(" · ")) };
                    if resp.secondary_clicked() {
                        secondary = Some(index);
                    }
                    // Several chosen and this one among them: the menu of the choice.
                    if many && is_marked {
                        resp.context_menu(|ui| {
                            if let Some(item) = batch_items(ui, marked.len(), batch_running.as_deref()) {
                                batch_menu = Some(item);
                                ui.close();
                            }
                        });
                    } else {
                    resp.context_menu(|ui| {
                        if ui.button(tr("Open")).clicked() {
                            opened = Some(index);
                            ui.close();
                        }
                        // The first !! (else !) the search found: the game at it.
                        if let Some(f) = Self::shown_mark(marks_ref, shown_ref, sides_ref, db, index as u32)
                            .and_then(|e| {
                                let first = e.found.iter().find(|f| f.brilliant).or(e.found.first());
                                first.cloned()
                            })
                            && ui
                                .button(trf("Open at {move}", &[("move", &f.label())]))
                                .clicked()
                        {
                            let at = db.load_game(index).and_then(|p| {
                                let node = *p.game.mainline().get(f.ply as usize)?;
                                Some(p.game.node(node)?.position().clone())
                            });
                            actions.push(LibraryAction::OpenGameReport {
                                path: path.to_path_buf(),
                                index,
                                at,
                            });
                            ui.close();
                        }
                        // The game's page, when its tags have one (read once per menu).
                        if self.menu_link.as_ref().is_none_or(|(i, _)| *i != index) {
                            let link = db.load_game(index).and_then(|p| {
                                crate::game_link::of(|t| p.game.tag(t).map(str::to_owned))
                            });
                            self.menu_link = Some((index, link));
                        }
                        if let Some((_, Some(link))) = &self.menu_link
                            && ui.button(link.label()).clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(link.at(0, false)));
                            ui.close();
                        }
                        // One item per player of the game (the same name once).
                        let mut players: Vec<&str> = vec![r.white.as_str(), r.black.as_str()];
                        players.dedup();
                        for name in players {
                            if name.trim().is_empty() || name == "?" {
                                continue;
                            }
                            if ui
                                .button(trf("♟ Prepare against {name}…", &[("name", &name)]))
                                .clicked()
                            {
                                self.prepare = Some(PrepEntry {
                                    query: name.to_owned(),
                                    exact: None,
                                    side: Side::Any,
                                });
                                ui.close();
                            }
                        }
                        if ui.button(tr("Game data… (F2)")).clicked() {
                            actions.push(LibraryAction::EditGameData { path: path.to_path_buf(), index });
                            ui.close();
                        }
                        if ui.button(tr("Add to database… (Ctrl+Shift+S)")).clicked() {
                            actions.push(LibraryAction::AddGameToDatabase { path: path.to_path_buf(), index });
                            ui.close();
                        }
                        if ui.button(tr("Select all (Ctrl+A)")).clicked() {
                            batch_menu = Some(BatchItem::SelectAll);
                            ui.close();
                        }
                        let label = if seen {
                            tr("Mark as not viewed")
                        } else {
                            tr("Mark as viewed")
                        };
                        if ui.button(label).clicked() {
                            toggle_viewed = Some((key, !seen));
                            ui.close();
                        }
                        if self.writable {
                            ui.separator();
                            if ui.button(tr("Delete from the database… (Delete)")).clicked() {
                                actions.push(LibraryAction::DeleteGame { path: path.to_path_buf(), index });
                                ui.close();
                            }
                        }
                    });
                    }
                    if resp.hovered() {
                        hovered = Some(index);
                    }
                    if resp.clicked() {
                        clicked = Some(index);
                    }
                    if resp.double_clicked() {
                        opened = Some(index);
                    }
                });
            });
        // The shown rows belong to the shown position: no arrow on another one.
        let shown_here = shown_position.as_ref() == Some(&self.board.position());
        self.table_hover = hovered
            .filter(|_| shown_here)
            .zip(self.outcome.as_deref().filter(|_| use_position))
            .and_then(|(i, out)| board_search::next_move_of(out, i));
        if let Some((k, v)) = toggle_viewed {
            viewed.set(k, v);
        }
        if marks_click {
            self.marks_sort = !self.marks_sort;
            if self.marks_sort {
                self.sort_by_marks();
            } else {
                // The list comes again in the sort of its columns.
                self.shown = None;
            }
        }
        if let Some(k) = sort_click {
            self.marks_sort = false;
            let current = self.effective_sort();
            self.sort = if current.0 == k {
                (k, !current.1)
            } else {
                (k, true)
            };
            self.sort_chosen = true;
            self.typed_at = f64::NEG_INFINITY;
        }
        if let Some(i) = clicked {
            let mods = ui.input(|i| i.modifiers);
            self.click(i, mods);
        }
        // A right click on a row not chosen chooses it alone.
        if let Some(i) = secondary
            && !(self.marked.len() >= 2 && self.marked.contains(i))
        {
            self.click(i, egui::Modifiers::NONE);
        }
        match batch_menu {
            Some(BatchItem::NewBase) => self.ask_copy(path, actions),
            Some(BatchItem::SelectAll) => self.select_all(),
            Some(BatchItem::Clear) => self.mark_cursor_only(),
            None => {}
        }
        if let Some(index) = opened {
            self.selected = Some(index);
            actions.push(LibraryAction::OpenGame {
                path: path.to_path_buf(),
                index,
                at: shown_position.clone(),
            });
        }
        open_notes
    }

    /// Up/Down move the cursor, Page Up/Down by a page, Home/End to the ends, Enter
    /// opens. Without a modifier the choice follows the cursor, Shift chooses the range
    /// from the anchor, Ctrl moves the cursor alone; Ctrl+Space toggles its row, Ctrl+A
    /// chooses the whole list (§42.6).
    fn keyboard(&mut self, ui: &mut Ui, path: &Path, actions: &mut Vec<LibraryAction>) {
        // In a filter field the arrows still move through the table (unless the
        // suggestions are open), but Enter belongs to the field.
        let typing = ui.ctx().egui_wants_keyboard_input();
        if typing && self.suggestions.is_some() {
            return;
        }
        // A question or a menu over the table takes the keys (Enter there is its button,
        // not «open the game»).
        if crate::app::overlay_open_besides(ui.ctx(), &[egui::Id::new("filters_drawer")]) {
            return;
        }
        let (up, down, page_up, page_down, enter, mods) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Enter),
                i.modifiers,
            )
        });
        let (home, end) = if typing {
            (false, false)
        } else {
            ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                )
            })
        };
        if !typing {
            let (all, toggle) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Space),
                )
            });
            if all {
                self.select_all();
            }
            if toggle && let Some(c) = self.selected {
                self.mark(|m| m.toggle(c));
                self.anchor = Some(c);
            }
        }
        // The cursor's record: F2 opens it with its data, Ctrl+Shift+S adds a copy of it
        // to another database (several chosen: a new database of them), Delete removes
        // it (a question first; never while several are chosen).
        if !typing && let Some(index) = self.selected {
            let (data, add, delete) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::F2),
                    i.consume_key(
                        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                        egui::Key::S,
                    ),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
                )
            });
            let many = self.marked.len() >= 2;
            let p = path.to_path_buf();
            if data {
                actions.push(LibraryAction::EditGameData {
                    path: p.clone(),
                    index,
                });
            }
            if add && many {
                self.ask_copy(path, actions);
            } else if add {
                actions.push(LibraryAction::AddGameToDatabase {
                    path: p.clone(),
                    index,
                });
            }
            if delete && many {
                actions.push(LibraryAction::Notice(
                    tr("Games are deleted one at a time: clear the selection (Esc)").to_owned(),
                ));
            } else if delete {
                // Also for a database that takes no games: the app says why.
                actions.push(LibraryAction::DeleteGame { path: p, index });
            }
        }
        // Enter belongs to a filter field, but in the quick search it opens the game.
        let enter = enter && (!typing || self.search_focused);
        let step = if page_up || page_down { 20 } else { 1 };
        if up || down || page_up || page_down || home || end {
            let pos = self.selected.and_then(|r| self.position_of(r));
            let last = self.view.len().saturating_sub(1);
            let next = match (pos, down || page_down) {
                _ if home => 0,
                _ if end => last,
                (None, _) => 0,
                (Some(p), true) => (p + step).min(last),
                (Some(p), false) => p.saturating_sub(step),
            };
            if let Some(&i) = self.view.get(next) {
                let i = i as usize;
                if mods.shift {
                    let from = self
                        .anchor
                        .and_then(|a| self.position_of(a))
                        .or(pos)
                        .unwrap_or(next);
                    if self.anchor.and_then(|a| self.position_of(a)).is_none() {
                        self.anchor = self.view.get(from).map(|&r| r as usize);
                    }
                    let range = self.range(from, next).to_vec();
                    self.mark(|m| {
                        m.clear();
                        m.extend(&range);
                    });
                } else if !mods.command {
                    self.mark(|m| m.only(i));
                    self.anchor = Some(i);
                }
                self.selected = Some(i);
                self.scroll_to_selected = true;
            }
        }
        if enter && let Some(index) = self.selected {
            actions.push(LibraryAction::OpenGame {
                path: path.to_path_buf(),
                index,
                at: self
                    .shown
                    .as_ref()
                    .filter(|s| s.use_position)
                    .and_then(|s| s.position.as_ref())
                    .map(|(_, q)| q.position.clone()),
            });
        }
    }
}

/// Up to this many chosen records are checked when their file is read again; a larger
/// choice is let go if the file changed.
const CHECKED_CHOICE: usize = 100_000;

/// A hash of the bytes of record `index`.
fn record_hash(db: &Database, index: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    db.identity(index).hash(&mut h);
    h.finish()
}

/// Space between the slots of the conditions line (`item_spacing.x` + 4).
const SLOT_GAP: f32 = 4.0;
/// The «!!» column.
const MARKS_COL: f32 = 28.0;

/// An item of the menus about the chosen records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatchItem {
    NewBase,
    SelectAll,
    Clear,
}

/// The menu of several chosen rows (§42.7): how many, the copy, select all, clear. The
/// copy waits while another one is written.
fn batch_items(ui: &mut Ui, n: usize, writing: Option<&str>) -> Option<BatchItem> {
    let mut out = None;
    ui.label(RichText::new(trf("Games selected: {n}", &[("n", &grouped(n))])).weak());
    ui.separator();
    let copy = ui.add_enabled(
        writing.is_none() && crate::platform::CAPS.batch_copy,
        egui::Button::new(tr("New database from selected…")),
    );
    let copy = match writing {
        Some(name) => copy.on_disabled_hover_text(trf(
            "Writing to «{name}» — wait until it finishes",
            &[("name", &name)],
        )),
        None => copy,
    };
    if copy.clicked() {
        out = Some(BatchItem::NewBase);
    }
    ui.separator();
    if ui.button(tr("Select all (Ctrl+A)")).clicked() {
        out = Some(BatchItem::SelectAll);
    }
    if ui.button(tr("Clear selection (Esc)")).clicked() {
        out = Some(BatchItem::Clear);
    }
    out
}

/// Made-up marks for the first rows of `view` (`CP_MARKS_DEMO`): !!, !, none, not
/// analysable, in turn; not saved.
fn demo_marks(db: &Database, view: &[u32], marks: &std::sync::Mutex<crate::brilliant::Index>) {
    use crate::brilliant::{Counts, Entry, Found, SideMarks, Skip};
    let Ok(mut index) = marks.lock() else {
        return;
    };
    for (n, &r) in view.iter().take(8).enumerate() {
        let Some(game) = db.load_game(r as usize).map(|p| p.game) else {
            continue;
        };
        let line = game_review::GameAnalysis::new(&game, None).cache_key();
        let side = |brilliant, great| SideMarks {
            counts: Counts {
                brilliant,
                great,
                ..Counts::default()
            },
            acpl: Some(30),
            accuracy: Some(85.0),
        };
        let found = |brilliant| Found {
            ply: 20,
            white: true,
            number: 11,
            san: "Rxf7".into(),
            brilliant,
        };
        let entry = match n % 4 {
            0 => Entry {
                line,
                white: side(1, 0),
                black: side(0, 0),
                found: vec![found(true)],
                stale: false,
                skip: None,
            },
            1 => Entry {
                line,
                white: side(0, 2),
                black: side(0, 0),
                found: vec![found(false)],
                stale: true,
                skip: None,
            },
            2 => Entry {
                line,
                white: side(0, 0),
                black: side(0, 0),
                found: Vec::new(),
                stale: false,
                skip: None,
            },
            _ => Entry {
                line,
                white: SideMarks::default(),
                black: SideMarks::default(),
                found: Vec::new(),
                stale: false,
                skip: Some(Skip::NullMove),
            },
        };
        index.set(r, entry);
    }
}

/// The «!!» cell of a record (task 27 §4.3): !!, !, «·» analysed without them, «—» not
/// analysable, a spinner while analysed, nothing when not analysed. Signs, not colours
/// alone; on a chosen row in the text colour of the selection.
fn marks_cell(ui: &mut Ui, entry: Option<crate::brilliant::Entry>, running: bool, chosen: bool) {
    ui.with_layout(
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| {
            if running {
                ui.add(egui::Spinner::new().size(12.0))
                    .on_hover_text(tr("Being reviewed…"));
                return;
            }
            let Some(e) = entry else {
                return;
            };
            let dark = ui.visuals().dark_mode;
            let tint = |nag: u8| {
                if chosen {
                    ui.visuals().selection.stroke.color
                } else {
                    crate::notation::assessment_text_color(chess_core::Nag(nag), dark)
                        .unwrap_or(ui.visuals().text_color())
                }
            };
            let moves = |brilliant: bool| {
                let list: Vec<String> = e
                    .found
                    .iter()
                    .filter(|f| f.brilliant == brilliant)
                    .map(|f| f.label())
                    .collect();
                let shown = list.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                if list.len() > 3 {
                    format!(
                        "{shown} {}",
                        trf("and {n} more", &[("n", &(list.len() - 3))])
                    )
                } else {
                    shown
                }
            };
            let c = e.of_sides(true, true);
            let (text, color, mut hint) = if let Some(skip) = e.skip {
                (
                    "—",
                    ui.visuals().weak_text_color(),
                    trf(
                        "Cannot be reviewed: {reason}",
                        &[("reason", &skip.reason())],
                    ),
                )
            } else if c.brilliant > 0 || c.great > 0 {
                let mut hint = Vec::new();
                if c.brilliant > 0 {
                    hint.push(trf(
                        "Brilliant moves: {n}: {moves}",
                        &[("n", &c.brilliant), ("moves", &moves(true))],
                    ));
                }
                if c.great > 0 {
                    hint.push(trf(
                        "Great moves: {n}: {moves}",
                        &[("n", &c.great), ("moves", &moves(false))],
                    ));
                }
                if c.brilliant > 0 {
                    ("!!", tint(3), hint.join("\n"))
                } else {
                    ("!", tint(1), hint.join("\n"))
                }
            } else {
                (
                    "•",
                    ui.visuals().weak_text_color(),
                    tr("Reviewed: no brilliant or great moves").to_owned(),
                )
            };
            if e.stale {
                hint.push('\n');
                hint.push_str(tr("Reviewed with the previous engine settings"));
            }
            let color = if chosen {
                ui.visuals().selection.stroke.color
            } else {
                color
            };
            // Ubuntu-B, as the other signs of moves.
            let bold = egui::FontId::new(13.0, egui::FontFamily::Name("bold".into()));
            let font = if ui.fonts_mut(|f| f.families().contains(&bold.family)) {
                bold
            } else {
                egui::FontId::proportional(13.0)
            };
            ui.add(egui::Label::new(RichText::new(text).font(font).color(color)).selectable(false))
                .on_hover_text(hint);
        },
    );
}

/// The keyboard cursor's frame: 1 px inside the row.
fn ui_painter_frame(row: &egui::Response, rect: egui::Rect, color: Color32, clip: egui::Rect) {
    let stroke = Stroke::new(1.0, color);
    row.ctx
        .layer_painter(row.layer_id)
        .with_clip_rect(clip)
        .rect_stroke(rect.intersect(clip), 0.0, stroke, egui::StrokeKind::Inside);
}

/// A condition of the list, shown as a chip.
struct Condition {
    text: String,
    pane: Pane,
    clear: Clear,
    /// Said under the pointer (a condition not applied, and why).
    hint: Option<String>,
    /// Position (and how to draw it) for the tooltip of the position chip.
    mini: Option<(
        shakmaty::Chess,
        bool,
        crate::assets::BoardThemeId,
        crate::assets::PieceSet,
    )>,
}

/// What removing a chip clears.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clear {
    Position,
    Search,
    Player,
    Opponent,
    Event,
    Annotator,
    Years,
    Elo,
    Result,
    Eco,
    Length,
    Comments,
    Variations,
    Texts,
}

/// One chip: its text opens the condition, × removes it. Returns (opened, removed).
fn chip(ui: &mut Ui, c: &Condition) -> (bool, bool) {
    let mut out = (false, false);
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let r = ui.add(
                egui::Label::new(RichText::new(c.text.as_str()).size(13.0)).sense(Sense::click()),
            );
            let r = match &c.mini {
                Some((pos, flipped, theme, pieces)) => r.on_hover_ui(|ui| {
                    let view = crate::board::BoardView {
                        pos,
                        orientation: if *flipped {
                            shakmaty::Color::Black
                        } else {
                            shakmaty::Color::White
                        },
                        last_move: None,
                        arrows: &[],
                        circles: &[],
                        badge: None,
                        interactive: false,
                        temporary_shapes: false,
                        show_coords: false,
                        theme: theme.theme(),
                        pieces: *pieces,
                        animation: 0.0,
                        premove: None,
                        auto_queen: false,
                        alert: None,
                    };
                    ui.allocate_ui(vec2(160.0, 160.0), |ui| {
                        view.show(ui, &mut crate::board::BoardState::default())
                    });
                }),
                None => r,
            };
            let r = match &c.hint {
                Some(h) => r.on_hover_text(h),
                None => r,
            };
            out.0 = r.clicked();
            out.1 = crate::widgets::small_close_button(ui)
                .on_hover_text(tr("Remove the condition"))
                .clicked();
        });
    out
}

/// The cell between "from" and "to" (and before Elo's rule).
const DASH: f32 = 16.0;

/// A row of the «Game» grid: the caption in a column `label_w` wide, then the fields.
fn form_row(ui: &mut Ui, label_w: f32, caption: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) =
            ui.allocate_exact_size(vec2(label_w, ui.spacing().interact_size.y), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            caption,
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        ui.add_space(4.0);
        add(ui);
    });
}

/// The dash between "from" and "to", centred in its cell (empty: only the gap).
fn dash(ui: &mut Ui, text: &str) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(DASH, ui.spacing().interact_size.y), Sense::hover());
    if !text.is_empty() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        );
    }
}

/// A one-line text field across the panel, with a red frame when `ok` is false.
/// A non-empty field has a × to clear it (the response then reports a change).
fn text_field(ui: &mut Ui, text: &mut String, hint: &str, ok: bool) -> egui::Response {
    ui.horizontal(|ui| {
        // The × (22 pt) and its 4 pt gap have their place whether shown or not: the row
        // never sticks out, so the panel keeps its width while one types (§2.6).
        ui.spacing_mut().item_spacing.x = 4.0;
        let mut edit = egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width((ui.available_width() - CLEAR_SLOT).floor().max(40.0));
        if !ok {
            edit = edit.text_color(ui.visuals().error_fg_color);
        }
        let mut r = ui.add(edit);
        if !ok {
            ui.painter().rect_stroke(
                r.rect,
                2.0,
                Stroke::new(1.5, ui.visuals().error_fg_color),
                egui::StrokeKind::Outside,
            );
        }
        if text.is_empty() {
            ui.add_space(CLEAR_SLOT - 4.0);
        } else if crate::widgets::small_close_button(ui)
            .on_hover_text(tr("Clear"))
            .clicked()
        {
            text.clear();
            r.mark_changed();
        }
        r
    })
    .inner
}

/// A short field for a number (digits only).
fn number_field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    ok: bool,
    width: f32,
) -> egui::Response {
    let mut edit = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(width)
        .char_limit(4);
    if !ok {
        edit = edit.text_color(ui.visuals().error_fg_color);
    }
    let r = ui.add(edit);
    text.retain(|c| c.is_ascii_digit());
    if !ok {
        ui.painter().rect_stroke(
            r.rect,
            2.0,
            Stroke::new(1.5, ui.visuals().error_fg_color),
            egui::StrokeKind::Outside,
        );
    }
    r
}

fn check_mark(ui: &mut Ui) {
    let c = ui.max_rect().left_center() + vec2(7.0, 0.0);
    let stroke = Stroke::new(2.0, Color32::from_rgb(0x62, 0x9c, 0x2e));
    ui.painter()
        .line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.5)], stroke);
    ui.painter()
        .line_segment([c + vec2(-1.0, 3.5), c + vec2(5.0, -4.0)], stroke);
}

/// "11 741 260": thousands separated by a narrow space.
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// "49", "8 512", "250 thousand", "11.7 million" for the database card.
pub fn short_count(n: usize) -> String {
    if n < 10_000 {
        grouped(n)
    } else if n < 1_000_000 {
        trf("{n} thousand", &[("n", &(n / 1000))])
    } else {
        let m = format!("{:.1}", n as f64 / 1e6);
        let m = if tr("decimal comma") == "," {
            m.replace('.', ",")
        } else {
            m
        };
        trf("{n} million", &[("n", &m)])
    }
}

/// The tooltip of «+ Game»: where its game goes.
fn new_game_hint(name: &str) -> String {
    trf(
        "A new game for «{name}»: Ctrl+S adds it to this database",
        &[("name", &name)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> egui::Modifiers {
        egui::Modifiers::COMMAND
    }

    fn shift() -> egui::Modifiers {
        egui::Modifiers::SHIFT
    }

    /// A view showing records 0..20 in the order 19, 18, … 0 (a sort).
    fn table() -> LargeView {
        let mut v = LargeView::new();
        v.view = (0..20).rev().collect();
        v
    }

    #[test]
    fn clicks_choose_one_toggle_and_ranges_in_the_tables_order() {
        let mut v = table();
        v.click(3, egui::Modifiers::NONE);
        assert_eq!((v.marked.records(), v.selected), (vec![3], Some(3)));
        v.click(7, ctrl());
        v.click(15, ctrl());
        assert_eq!(v.marked.records(), vec![3, 7, 15]);
        v.click(7, ctrl());
        assert_eq!(v.marked.records(), vec![3, 15]);
        // Shift: the range from the anchor (7, the last Ctrl click) in the table's order
        // replaces the choice; Ctrl+Shift adds it.
        v.click(10, shift());
        assert_eq!(v.marked.records(), vec![7, 8, 9, 10]);
        v.click(1, ctrl());
        v.click(3, ctrl() | shift());
        assert_eq!(v.marked.records(), vec![1, 2, 3, 7, 8, 9, 10]);
        v.click(5, egui::Modifiers::NONE);
        assert_eq!(v.marked.records(), vec![5]);
    }

    #[test]
    fn an_anchor_hidden_by_the_filter_gives_way_to_the_cursor() {
        let mut v = table();
        v.click(30, egui::Modifiers::NONE);
        v.click(4, ctrl());
        v.anchor = Some(30);
        v.click(6, shift());
        assert_eq!(v.marked.records(), vec![4, 5, 6]);
    }

    #[test]
    fn select_all_takes_the_list_and_escape_comes_back_to_the_cursor() {
        let mut v = table();
        v.marked.insert(40);
        v.click(40, ctrl());
        v.click(2, ctrl());
        v.select_all();
        assert_eq!(v.marked.len(), 20);
        assert_eq!(v.hidden_marked(), 0);
        v.marked.insert(40);
        v.marked_gen += 1;
        assert_eq!(v.hidden_marked(), 1);
        assert!(v.escape());
        assert_eq!(v.marked.records(), vec![2]);
        // One chosen: Escape is the next one's (here nobody's).
        assert!(!v.escape());
    }

    #[test]
    fn returning_from_a_game_keeps_a_choice_of_several() {
        let mut v = table();
        v.click(1, egui::Modifiers::NONE);
        v.reveal(9);
        assert_eq!(v.marked.records(), vec![9]);
        v.click(3, ctrl());
        v.reveal(3);
        assert_eq!(v.marked.records(), vec![3, 9]);
    }

    fn database(name: &str, games: &[&str]) -> Database {
        let path =
            std::env::temp_dir().join(format!("cp-reread-{}-{name}.pgn", std::process::id()));
        let text: String = games
            .iter()
            .map(|w| format!("[White \"{w}\"]\n[Black \"x\"]\n[Result \"*\"]\n\n1. e4 *\n\n"))
            .collect();
        std::fs::write(&path, text).unwrap();
        let db = chess_import::open(
            &path,
            &mut |_| {},
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .db;
        let _ = std::fs::remove_file(&path);
        db
    }

    #[test]
    fn a_choice_never_moves_to_other_games_when_the_file_is_read_again() {
        let before = database("before", &["a", "b", "c", "d"]);
        // A game added at the end: the chosen ones are the same records.
        let mut v = table();
        v.click(1, egui::Modifiers::NONE);
        v.click(2, ctrl());
        v.before_reread(&before);
        v.reread(&database("added", &["a", "b", "c", "d", "e"]));
        assert_eq!(v.marked.records(), vec![1, 2]);
        assert!(v.notice.is_none());
        // A game put back at the start (undo of a delete): the numbers moved; let go,
        // and said.
        v.before_reread(&before);
        v.reread(&database("undo", &["z", "a", "b", "c", "d"]));
        assert!(v.marked.is_empty());
        assert!(v.notice.is_some());
    }

    #[test]
    fn the_copy_asks_for_the_chosen_records_and_the_tables_sort() {
        let mut v = table();
        v.sort = (SortKey::White, false);
        v.click(5, egui::Modifiers::NONE);
        v.click(2, ctrl());
        let mut actions = Vec::new();
        v.ask_copy(Path::new("/x.pgn"), &mut actions);
        match actions.as_slice() {
            [
                LibraryAction::CopySelection {
                    records,
                    sort,
                    ascending,
                    hidden,
                    ..
                },
            ] => {
                assert_eq!(records, &vec![2, 5]);
                assert_eq!((*sort, *ascending, *hidden), (SortKey::White, false, 0));
            }
            _ => panic!("one copy"),
        }
        // None while another copy is written.
        v.batch = Some(BatchStatus {
            name: "x".into(),
            done: 0,
            total: 1,
            cancelling: false,
        });
        actions.clear();
        v.ask_copy(Path::new("/x.pgn"), &mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn numbers_group_and_shorten() {
        assert_eq!(grouped(11_741_260), "11\u{202f}741\u{202f}260");
        assert_eq!(grouped(999), "999");
        assert_eq!(short_count(49), "49");
    }

    #[test]
    fn form_builds_valid_filters_only() {
        let mut f = Form {
            year_from: "2020".into(),
            year_to: "2010".into(),
            elo_min: "abc".into(),
            eco: "B9".into(),
            ..Form::default()
        };
        let filter = f.filter();
        assert_eq!(
            (filter.year_from, filter.year_to),
            (None, None),
            "from > to"
        );
        assert_eq!(filter.elo_min, None);
        assert_eq!(filter.eco, vec![(190, 199)]);
        assert_eq!(filter.side, Side::Any);
        f.year_to = "2024".into();
        assert_eq!(f.filter().year_to, Some(2024));
        assert_eq!(f.active(), 3);
    }
}
