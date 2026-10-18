//! Game list of a large ChessBase database (millions of games): a filter panel on
//! the right and a table of the matching records. Filtering and sorting run in a
//! background thread; a newer request cancels the older one. Layout and behaviour:
//! `docs/design/large-databases.md`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

use chess_import::Database;
use chess_import::chessbase::base::{
    EloRule, Filter, NameFilter, Outcome, RecordKind, Side, SortKey, Suggestion, parse_eco,
};
use egui::{Color32, RichText, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};

use crate::i18n::{tr, trf};
use crate::library::LibraryAction;
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

#[derive(Clone, PartialEq)]
struct Request {
    filter: Filter,
    sort: SortKey,
    ascending: bool,
}

struct Job {
    request: Request,
    cancel: Arc<AtomicBool>,
    scanned: Arc<AtomicUsize>,
    rx: Receiver<Option<Vec<u32>>>,
    started: f64,
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
    /// Matching records, in display order, and the request they answer.
    view: Vec<u32>,
    shown: Option<Request>,
    job: Option<Job>,
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
    panel_open: bool,
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
        LargeView {
            form,
            sort: (SortKey::Number, true),
            view: Vec::new(),
            shown: None,
            job: None,
            typed_at: f64::NEG_INFINITY,
            suggest_job: None,
            suggestions: None,
            pointer_on_list: false,
            selected: None,
            scroll_to_selected: false,
            scroll_to_top: false,
            focus_player: false,
            panel_open: true,
            typed: Default::default(),
        }
    }

    /// Selects a record and scrolls to it (returning from a game).
    pub fn reveal(&mut self, index: usize) {
        self.selected = Some(index);
        self.scroll_to_selected = true;
    }

    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    fn request(&self) -> Request {
        Request {
            filter: self.form.filter(),
            sort: self.sort.0,
            ascending: self.sort.1,
        }
    }

    /// Collects finished work and starts what the form asks for.
    fn update(&mut self, db: &Arc<Database>, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Some(job) = &self.job
            && let Ok(result) = job.rx.try_recv()
        {
            let job = self.job.take().expect("job");
            if let Some(view) = result {
                let new_filter = self
                    .shown
                    .as_ref()
                    .is_none_or(|s| s.filter != job.request.filter);
                self.view = view;
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
        if self.job.is_some() || self.suggest_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn start(&mut self, db: &Arc<Database>, request: Request, now: f64, ctx: &egui::Context) {
        if let Some(old) = self.job.take() {
            old.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let scanned = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let (db, req, c, s, ctx) = (
            db.clone(),
            request.clone(),
            cancel.clone(),
            scanned.clone(),
            ctx.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("db-filter".into())
            .spawn(move || {
                let result = db
                    .as_chessbase()
                    .and_then(|b| b.query(&req.filter, req.sort, req.ascending, &s, &c));
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.job = Some(Job {
                request,
                cancel,
                scanned,
                rx,
                started: now,
            });
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
        (now - j.started > SLOW)
            .then(|| j.scanned.load(Ordering::Relaxed) as f32 / total.max(1) as f32)
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
        if ui.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Backspace,
            )
        }) {
            self.reset();
        }
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
    pub fn table(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        db: &Arc<Database>,
        viewed: &mut Viewed,
        notes: usize,
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

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
                .strong(),
            );
            ui.label(RichText::new(tr("· ChessBase, read-only")).weak())
                .on_hover_text(tr(
                    "Read in place; the original files are not changed. Save a game to keep your edits.",
                ));
            let counter = match &self.shown {
                Some(s) if s.filter != Filter::default() || self.form.active() > 0 => trf(
                    "· found {n} of {total}",
                    &[("n", &grouped(self.view.len())), ("total", &grouped(total))],
                ),
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
            ui.label(counter);
            if let Some(p) = progress {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(format!("{:.0} %", p * 100.0)).weak());
            }
            if notes > 0
                && ui
                    .button(RichText::new(format!("⚠ {notes}")).color(ui.visuals().warn_fg_color))
                    .on_hover_text(tr("Import notes"))
                    .clicked()
            {
                open_notes = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let active = self.form.active();
                let label = if self.panel_open {
                    tr("Filters »").to_string()
                } else if active > 0 {
                    trf("« Filters ({n})", &[("n", &active)])
                } else {
                    tr("« Filters").to_string()
                };
                if ui.button(label).clicked() {
                    self.panel_open = !self.panel_open;
                }
            });
        });
        ui.add_space(4.0);

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.panel_open = true;
            self.focus_player = true;
        }

        if self.shown.is_some() && self.view.is_empty() && self.job.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new(tr("Nothing found")).size(16.0));
                ui.add_space(6.0);
                if ui.button(tr("Reset filters")).clicked() {
                    self.reset();
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
        let width = ui.available_width();
        let show_round = width >= 1000.0;
        let show_moves = width >= 760.0;
        let show_marks = width >= 720.0;
        let mut columns: Vec<(&str, Option<SortKey>, Column)> = vec![
            ("№", Some(SortKey::Number), Column::exact(84.0)),
            (
                tr("White"),
                Some(SortKey::White),
                Column::initial(170.0).at_least(60.0).clip(true),
            ),
            ("Elo", Some(SortKey::WhiteElo), Column::exact(44.0)),
            (
                tr("Black"),
                Some(SortKey::Black),
                Column::initial(170.0).at_least(60.0).clip(true),
            ),
            ("Elo", Some(SortKey::BlackElo), Column::exact(44.0)),
            (tr("Result"), Some(SortKey::Result), Column::exact(56.0)),
        ];
        if show_moves {
            columns.push((tr("Moves"), Some(SortKey::Moves), Column::exact(52.0)));
        }
        columns.push(("ECO", Some(SortKey::Eco), Column::exact(44.0)));
        if show_marks {
            columns.push(("💬", None, Column::exact(40.0)));
        }
        columns.push((
            tr("Event"),
            Some(SortKey::Event),
            Column::remainder().at_least(80.0).clip(true),
        ));
        if show_round {
            columns.push((tr("Round"), None, Column::exact(48.0)));
        }
        columns.push((tr("Date"), Some(SortKey::Date), Column::exact(88.0)));

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
        let view = &self.view;
        let selected = self.selected;
        let sort = self.sort;
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
                    let result = match r.result {
                        _ if is_text => "",
                        "1/2-1/2" => "½-½",
                        x => x,
                    };
                    for (label, key_, _) in &columns {
                        row.col(|ui| {
                            let cell = match (*label, *key_) {
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
                                (_, Some(SortKey::Date)) => RichText::new(crate::library::display_date(&r.date)),
                                _ => RichText::new(if r.round == "?" { "" } else { r.round.as_str() }).weak(),
                            };
                            ui.add(egui::Label::new(cell).selectable(false).truncate());
                        });
                    }
                    let resp = row.response();
                    let resp = if show_round {
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
                    if resp.clicked() {
                        clicked = Some(index);
                    }
                    if resp.double_clicked() {
                        opened = Some(index);
                    }
                });
            });
        if let Some((k, v)) = toggle_viewed {
            viewed.set(k, v);
        }
        if let Some(k) = sort_click {
            self.sort = if self.sort.0 == k {
                (k, !self.sort.1)
            } else {
                (k, true)
            };
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
            });
        }
    }
}

/// A one-line text field across the panel, with a red frame when `ok` is false.
fn text_field(ui: &mut Ui, text: &mut String, hint: &str, ok: bool) -> egui::Response {
    let mut edit = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(f32::INFINITY);
    if !ok {
        edit = edit.text_color(ui.visuals().error_fg_color);
    }
    let r = ui.add(edit);
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
