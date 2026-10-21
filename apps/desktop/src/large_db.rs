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
use crate::settings::Settings;
use crate::viewed::{self, Viewed};

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
}

struct Job {
    request: Request,
    work: Background<Answer>,
}

/// What the right side of the library shows for a large database.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Hidden,
    Filters,
    Board,
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
    /// Record index of the selected row.
    pub selected: Option<usize>,
    scroll_to_selected: bool,
    scroll_to_top: bool,
    focus_player: bool,
    pane: Pane,
    /// Last text of each name field, to notice typing.
    typed: [String; 3],
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
        let (board, mut pane) = match SearchBoard::from_env() {
            Some(b) => (b, Pane::Board),
            None => (SearchBoard::new(), Pane::Filters),
        };
        if std::env::var("CP_PANE").as_deref() == Ok("hidden") {
            pane = Pane::Hidden;
        }
        // `CP_SORT=date`: newest games first (screenshots).
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
            selected: None,
            scroll_to_selected: false,
            scroll_to_top: false,
            focus_player: false,
            pane,
            typed: Default::default(),
        }
    }

    /// Selects a record and scrolls to it (returning from a game).
    pub fn reveal(&mut self, index: usize) {
        self.selected = Some(index);
        self.scroll_to_selected = true;
    }

    pub fn pane(&self) -> Pane {
        self.pane
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
                let new_filter = self.shown.as_ref().is_none_or(|s| {
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
                self.outcome = answer.outcome;
                self.in_order = answer.in_order;
                self.shown = Some(job.request);
                // A selected game that left the list is no longer selected, so Enter
                // cannot open a game the user does not see.
                if let Some(sel) = self.selected
                    && new_filter
                    && !self.view.contains(&(sel as u32))
                {
                    self.selected = None;
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
        let (db, req) = (db.clone(), request.clone());
        let work = Background::spawn(ctx, now, move |progress, cancel| {
            let base = db.as_chessbase()?;
            let mut outcome = cached;
            if let Some((_, query)) = &req.position
                && outcome.is_none()
            {
                let candidates = if req.filter == Filter::default() {
                    None
                } else {
                    let ignore = AtomicUsize::new(0);
                    Some(base.query(&req.filter, SortKey::Number, true, &ignore, cancel)?)
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
                base.sort_records(&mut v, req.sort, req.ascending);
                v
            } else {
                base.query(&req.filter, req.sort, req.ascending, progress, cancel)?
            };
            Some(Answer {
                view,
                outcome,
                in_order,
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
                if let Some(b) = db.as_chessbase() {
                    let r = match field {
                        NameField::Event => b.suggest_events(&q, SUGGESTIONS),
                        _ => b.suggest_players(&q, SUGGESTIONS),
                    };
                    let _ = tx.send(r);
                }
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
        let total = db.as_chessbase().map_or(1, |b| b.records());
        let counting = self
            .job
            .as_ref()
            .map(|j| (j.work.progress() as f32 / total.max(1) as f32).min(1.0));
        if let Some((b, work)) = &self.booster {
            let p = (b.progress() as f32 / (*work).max(1) as f32).min(1.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
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
        let outcome = self.board_outcome().cloned();
        let mut events = Vec::new();
        self.board.keys(ui.ctx(), outcome.as_ref(), &mut events);
        events.extend(self.board.panel(
            ui,
            settings,
            outcome.as_ref(),
            counting,
            self.table_hover,
            false,
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

    /// The pane switch `[Filters | Board]`; a click on the shown pane hides it.
    fn pane_switch(&mut self, ui: &mut Ui) {
        let active = self.form.active();
        let filters = if active > 0 && self.pane != Pane::Filters {
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
        for (pane, label, hint) in [
            (
                Pane::Board,
                board,
                tr("Find games by position: play moves on the board (Ctrl+B)"),
            ),
            (
                Pane::Filters,
                filters,
                tr("Filters: players, tournament, years, rating… (Ctrl+F)"),
            ),
        ] {
            let on = self.pane == pane;
            let hint = if on { tr("Hide the panel") } else { hint };
            if ui.selectable_label(on, label).on_hover_text(hint).clicked() {
                self.pane = if on { Pane::Hidden } else { pane };
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
            out.push(Condition {
                text,
                pane: Pane::Board,
                clear: Clear::Position,
                mini: Some((
                    self.board.position(),
                    self.board.flipped,
                    settings.board_theme,
                    settings.piece_set,
                )),
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
        let mut clear = None;
        let mut goto = None;
        let mut toggle_order = false;
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                    if open {
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
        if let Some(c) = clear {
            self.clear(c);
        }
        if let Some(p) = goto {
            self.pane = p;
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
                self.annotations_group(ui);
            });
    }

    fn reset(&mut self) {
        self.form = Form::default();
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
        ui.label(tr("Player"));
        self.name_field(ui, db, NameField::Player);
        let player_set = self.form.player.is_set();
        ui.add_enabled_ui(player_set, |ui| {
            ui.horizontal(|ui| {
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
        ui.label(tr("Opponent"));
        self.name_field(ui, db, NameField::Opponent);
    }

    fn event_group(&mut self, ui: &mut Ui, db: &Arc<Database>) {
        let count = usize::from(self.form.event.is_set())
            + usize::from(!self.form.annotator.trim().is_empty());
        Self::group_header(ui, tr("Tournament"), count);
        ui.label(tr("Tournament or place"));
        self.name_field(ui, db, NameField::Event);
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

        let years_ok = {
            let a = number(&self.form.year_from, 1..=2100);
            let b = number(&self.form.year_to, 1..=2100);
            match (a, b) {
                (Ok(Some(a)), Ok(Some(b))) => (a <= b, a <= b),
                (a, b) => (a.is_ok(), b.is_ok()),
            }
        };
        ui.horizontal(|ui| {
            ui.label(tr("Years"));
            typed |= number_field(ui, &mut self.form.year_from, "", years_ok.0).changed();
            ui.label("–");
            typed |= number_field(ui, &mut self.form.year_to, "", years_ok.1).changed();
        });

        ui.horizontal(|ui| {
            ui.label(tr("Elo from"));
            let ok = number(&self.form.elo_min, 1..=3500).is_ok();
            typed |= number_field(ui, &mut self.form.elo_min, "", ok).changed();
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
                .width(96.0)
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

        ui.horizontal(|ui| {
            ui.label("ECO");
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

        let moves_ok = {
            let a = number(&self.form.moves_from, 0..=999);
            let b = number(&self.form.moves_to, 0..=999);
            match (a, b) {
                (Ok(Some(a)), Ok(Some(b))) => (a <= b, a <= b),
                (a, b) => (a.is_ok(), b.is_ok()),
            }
        };
        ui.horizontal(|ui| {
            ui.label(tr("Moves"));
            typed |= number_field(ui, &mut self.form.moves_from, "", moves_ok.0).changed();
            ui.label("–");
            typed |= number_field(ui, &mut self.form.moves_to, "", moves_ok.1).changed();
        });
        if typed {
            self.typed_at = now;
        }
    }

    fn annotations_group(&mut self, ui: &mut Ui) {
        let f = &self.form;
        let count = usize::from(f.with_comments)
            + usize::from(f.with_variations)
            + usize::from(f.hide_texts);
        Self::group_header(ui, tr("Annotations"), count);
        ui.checkbox(&mut self.form.with_comments, tr("with comments"));
        ui.checkbox(&mut self.form.with_variations, tr("with variations"));
        let mut show_texts = !self.form.hide_texts;
        ui.checkbox(&mut show_texts, tr("show texts"))
            .on_hover_text(tr("Guiding texts of the database; left out by any condition on players, dates, ratings, results or moves"));
        self.form.hide_texts = !show_texts;
    }

    /// A name field with suggestions; picking one filters by exactly that name.
    fn name_field(&mut self, ui: &mut Ui, db: &Arc<Database>, field: NameField) {
        let slot = field as usize;
        let value = match field {
            NameField::Player => &mut self.form.player,
            NameField::Opponent => &mut self.form.opponent,
            NameField::Event => &mut self.form.event,
        };
        let mut picked_this_frame = None;
        let resp = ui
            .horizontal(|ui| {
                if value.exact.is_some() {
                    ui.label(RichText::new("=").strong()).on_hover_text(tr(
                        "Exactly this name; edit the text to search by words again",
                    ));
                }
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
        viewed: &mut Viewed,
        notes: usize,
        settings: &Settings,
        actions: &mut Vec<LibraryAction>,
    ) -> bool {
        let mut open_notes = false;
        let Some(base) = db.as_chessbase() else {
            return false;
        };
        self.update(db, ui.ctx());
        let now = ui.input(|i| i.time);
        let progress = self.slow_progress(now, base.records());
        let total = base.games() + base.texts();

        // The pane switch comes first (right): it stays visible in a narrow window,
        // the rest of the line is shortened.
        ui.horizontal(|ui| {
          ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The switch packs its segments; the rest keeps the usual spacing.
            ui.scope(|ui| self.pane_switch(ui));
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        path.file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )
                    .strong(),
                )
                .truncate(),
            );
            if ui.available_width() > 520.0 {
                ui.label(RichText::new(tr("· ChessBase, read-only")).weak())
                    .on_hover_text(tr(
                        "Read in place; the original files are not changed. Save a game to keep your edits.",
                    ));
            }
            let counter = match &self.shown {
                Some(s)
                    if s.use_position
                        || s.filter != Filter::default()
                        || self.form.active() > 0 =>
                {
                    trf(
                    "· found {n} of {total}",
                        &[("n", &grouped(self.view.len())), ("total", &grouped(total))],
                    )
                }
                _ if base.texts() > 0 => trf(
                    "· {n} games · {t} texts",
                    &[("n", &grouped(base.games())), ("t", &grouped(base.texts()))],
                ),
                _ => trf("· {n} games", &[("n", &grouped(base.games()))]),
            };
            let mut counter = RichText::new(counter);
            if progress.is_some() {
                counter = counter.weak();
            }
            ui.add(egui::Label::new(counter).truncate());
            if let Some(p) = progress {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(format!("{:.0} %", p * 100.0)).weak());
            }
            if notes > 0 && crate::widgets::notes_button(ui, notes).clicked() {
                open_notes = true;
            }
            });
          });
        });
        self.chips(ui, settings);
        ui.add_space(2.0);

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.pane = Pane::Filters;
            self.focus_player = true;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
            self.pane = Pane::Board;
        }
        // Ctrl+Shift+Backspace: all conditions, the position included.
        if ui.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Backspace,
            )
        }) {
            self.reset();
            self.board = SearchBoard::new();
            self.only_order = false;
        }

        let use_position = self.shown.as_ref().is_some_and(|s| s.use_position);
        // The position the shown rows were found for (the board may be ahead of it).
        let shown_position = self
            .shown
            .as_ref()
            .filter(|s| s.use_position)
            .and_then(|s| s.position.as_ref())
            .map(|(_, q)| q.position.clone());
        if self.shown.is_some() && self.view.is_empty() && self.job.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                if use_position {
                    ui.label(
                        RichText::new(tr("The position does not occur in these games")).size(16.0),
                    );
                    ui.add_space(6.0);
                    if self.form.active() > 0
                        && ui.button(tr("Search the whole database")).clicked()
                    {
                        self.reset();
                    }
                } else {
                    ui.label(RichText::new(tr("Nothing found")).size(16.0));
                    ui.add_space(6.0);
                    if ui.button(tr("Reset filters")).clicked() {
                        self.reset();
                    }
                }
            });
            return open_notes;
        }

        self.keyboard(ui, path, actions);

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
            columns.push((tr("Move"), None, Column::exact(52.0)));
        }
        if show_number {
            columns.push(("№", Some(SortKey::Number), Column::exact(84.0)));
        }
        // Without the event the names share the rest of the width.
        let name = if show_event {
            Column::initial(170.0).at_least(60.0).clip(true)
        } else {
            Column::remainder().at_least(60.0).clip(true)
        };
        columns.extend([
            (tr("White"), Some(SortKey::White), name),
            ("Elo", Some(SortKey::WhiteElo), Column::exact(44.0)),
            (tr("Black"), Some(SortKey::Black), name),
            ("Elo", Some(SortKey::BlackElo), Column::exact(44.0)),
            (tr("Result"), Some(SortKey::Result), Column::exact(56.0)),
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
            columns.push((tr("Year"), Some(SortKey::Date), Column::exact(48.0)));
        } else {
            columns.push((tr("Date"), Some(SortKey::Date), Column::exact(88.0)));
        }

        if progress.is_some() {
            ui.set_opacity(0.6);
        }
        let mut table = TableBuilder::new(ui)
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
        let view = &self.view;
        let selected = self.selected;
        let sort = self.effective_sort();
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
                        let arrow = match key {
                            Some(k) if sort.0 == *k => {
                                if sort.1 { " ⏶" } else { " ⏷" }
                            }
                            _ => "",
                        };
                        let r = ui.add(
                            egui::Button::new(RichText::new(format!("{label}{arrow}")).strong())
                                .frame(false),
                        );
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
                    let r = base.row(index);
                    row.set_selected(selected == Some(index));
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
                                    RichText::new(grouped(index + 1)).weak()
                                }
                                (_, Some(SortKey::White)) if is_text => {
                                    RichText::new(format!("📄 {}", r.title)).italics()
                                }
                                (_, Some(SortKey::White)) => RichText::new(r.white.as_str()),
                                (_, Some(SortKey::WhiteElo)) => RichText::new(elo(r.white_elo)),
                                (_, Some(SortKey::Black)) => RichText::new(r.black.as_str()),
                                (_, Some(SortKey::BlackElo)) => RichText::new(elo(r.black_elo)),
                                (_, Some(SortKey::Result)) => RichText::new(result),
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
                    let resp = row.response();
                    let resp = if show_round && show_event {
                        resp
                    } else {
                        resp.on_hover_text(format!("{} · {}", r.event, r.round))
                    };
                    resp.context_menu(|ui| {
                        let label = if seen {
                            tr("Mark as not viewed")
                        } else {
                            tr("Mark as viewed")
                        };
                        if ui.button(label).clicked() {
                            toggle_viewed = Some((key, !seen));
                            ui.close();
                        }
                    });
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
        if let Some(k) = sort_click {
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
            self.selected = Some(i);
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

    /// Up/Down move the selection, Page Up/Down by a page, Enter opens.
    fn keyboard(&mut self, ui: &mut Ui, path: &Path, actions: &mut Vec<LibraryAction>) {
        // In a filter field the arrows still move through the table (unless the
        // suggestions are open), but Enter belongs to the field.
        let typing = ui.ctx().egui_wants_keyboard_input();
        if typing && self.suggestions.is_some() {
            return;
        }
        let (up, down, page_up, page_down, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Enter),
            )
        });
        let enter = enter && !typing;
        let step = if page_up || page_down { 20 } else { 1 };
        if up || down || page_up || page_down {
            let pos = self
                .selected
                .and_then(|r| self.view.iter().position(|&i| i as usize == r));
            let last = self.view.len().saturating_sub(1);
            let next = match (pos, down || page_down) {
                (None, _) => 0,
                (Some(p), true) => (p + step).min(last),
                (Some(p), false) => p.saturating_sub(step),
            };
            if let Some(&i) = self.view.get(next) {
                self.selected = Some(i as usize);
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

/// A condition of the list, shown as a chip.
struct Condition {
    text: String,
    pane: Pane,
    clear: Clear,
    /// Position (and how to draw it) for the tooltip of the position chip.
    mini: Option<(
        shakmaty::Chess,
        bool,
        crate::assets::BoardThemeId,
        crate::assets::PieceSet,
    )>,
}

/// What removing a chip clears.
#[derive(Clone, Copy)]
enum Clear {
    Position,
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
                        show_coords: false,
                        theme: theme.theme(),
                        pieces: *pieces,
                        animation: 0.0,
                    };
                    ui.allocate_ui(vec2(160.0, 160.0), |ui| {
                        view.show(ui, &mut crate::board::BoardState::default())
                    });
                }),
                None => r,
            };
            out.0 = r.clicked();
            out.1 = crate::widgets::small_close_button(ui)
                .on_hover_text(tr("Remove the condition"))
                .clicked();
        });
    out
}

/// A one-line text field across the panel, with a red frame when `ok` is false.
/// A non-empty field has a × to clear it (the response then reports a change).
fn text_field(ui: &mut Ui, text: &mut String, hint: &str, ok: bool) -> egui::Response {
    ui.horizontal(|ui| {
        let clear = if text.is_empty() { 0.0 } else { 26.0 };
        let mut edit = egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width((ui.available_width() - clear).max(40.0));
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
        if !text.is_empty()
            && crate::widgets::small_close_button(ui)
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
fn number_field(ui: &mut Ui, text: &mut String, hint: &str, ok: bool) -> egui::Response {
    let mut edit = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(48.0)
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

#[cfg(test)]
mod tests {
    use super::*;

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
