//! The search board of the library: the user plays moves and the game list shows the
//! games that reach the position, with the moves played next and their results.
//! Layout and behaviour: `docs/design/board-search.md` (decision 13).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

use chess_import::position::{MoveStat, PositionOutcome, PositionQuery, Tally};
use egui::{Color32, RichText, Sense, Stroke, Ui, vec2};
use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, EnPassantMode, Move, Position};

use crate::assets::Sound;
use crate::board::{self, Arrow, BoardState, BoardView};
use crate::i18n::{tr, trf};
use crate::settings::Settings;

/// Seconds the "Undo" of a reset stays offered.
const UNDO_SECONDS: f64 = 8.0;
/// Continuations next to the board from this panel width; below it, under the board.
const WIDE: f32 = 820.0;
const CONTINUATIONS_WIDTH: f32 = 360.0;
/// Arrow of a previewed move (continuation or a game's next move): blue, unlike the
/// engine's green arrows.
pub const PREVIEW_ARROW: Color32 = Color32::from_rgba_premultiplied(40, 110, 200, 190);

/// What the board asks of its owner.
pub enum BoardEvent {
    /// The position changed: search again.
    Changed,
    Sound(Sound),
    CopyText(String, &'static str),
    /// Open the moves as a new game (PGN text).
    OpenAsGame(String),
}

/// The board a reset replaced, and when.
struct Replaced {
    start: Chess,
    start_fen: Option<String>,
    line: Vec<Move>,
    cursor: usize,
    at: f64,
}

/// The search board: a start position, the moves played from it and the move shown.
pub struct SearchBoard {
    start: Chess,
    /// FEN of a start that is not the usual one (a position taken from a game).
    start_fen: Option<String>,
    line: Vec<Move>,
    /// Moves of `line` played on the board (the rest is kept until a new move).
    cursor: usize,
    pub flipped: bool,
    board: BoardState,
    /// What a reset replaced (for "Undo").
    undo: Option<Replaced>,
    /// Continuation under the pointer (its arrow is drawn).
    hover: Option<UciMove>,
    /// The first search of the database also prepares its position table (a PGN's,
    /// once per session): the waiting line says so.
    pub preparing: bool,
    /// The position cannot be searched now (no memory for the table): no continuations.
    pub unavailable: bool,
}

impl SearchBoard {
    pub fn new() -> SearchBoard {
        SearchBoard {
            start: Chess::default(),
            start_fen: None,
            line: Vec::new(),
            cursor: 0,
            flipped: false,
            board: BoardState::default(),
            undo: None,
            hover: None,
            preparing: false,
            unavailable: false,
        }
    }

    /// Dev hook (screenshots, manual checks): `CP_BOARD="e4 c5 Nf3"` opens the search
    /// board with these moves.
    pub fn from_env() -> Option<SearchBoard> {
        let line = std::env::var("CP_BOARD").ok()?;
        let mut pos = Chess::default();
        let mut moves = Vec::new();
        for san in line.split_whitespace() {
            let Some(m) = san
                .parse::<shakmaty::san::San>()
                .ok()
                .and_then(|s| s.to_move(&pos).ok())
            else {
                break;
            };
            pos.play_unchecked(m);
            moves.push(m);
        }
        let mut board = SearchBoard::new();
        board.set(Chess::default(), moves, false);
        Some(board)
    }

    /// Sets the board to `line` played from `start` (a position taken from a game).
    pub fn set(&mut self, start: Chess, line: Vec<Move>, flipped: bool) {
        self.start_fen = (start != Chess::default())
            .then(|| Fen::from_position(&start, EnPassantMode::Legal).to_string());
        self.start = start;
        self.cursor = line.len();
        self.line = line;
        self.flipped = flipped;
        self.board.clear();
        self.undo = None;
    }

    pub fn position(&self) -> Chess {
        let mut pos = self.start.clone();
        for m in &self.line[..self.cursor] {
            pos.play_unchecked(*m);
        }
        pos
    }

    /// A position is a search condition once it differs from the usual start.
    /// A piece is picked up or dragged, or a promotion is being chosen.
    pub fn is_busy(&self) -> bool {
        self.board.is_busy()
    }

    /// Puts the piece back (Escape).
    pub fn drop_pick(&mut self) {
        self.board.clear();
    }

    pub fn is_set(&self) -> bool {
        self.cursor > 0 || self.start_fen.is_some()
    }

    /// The query for the shown position (also for the start, whose continuations are
    /// shown without filtering the list).
    pub fn query(&self) -> PositionQuery {
        let line = self
            .start_fen
            .is_none()
            .then(|| self.line[..self.cursor].to_vec());
        PositionQuery::new(self.position(), line)
    }

    /// Identity of the shown position (for caching and comparing requests).
    pub fn key(&self) -> u64 {
        chess_import::position::key(&self.position())
    }

    /// "1.e4 c5 2.Nf3 d6" (with "🏠" before a start taken from a game).
    fn san_line(&self, upto: usize) -> Vec<(String, usize)> {
        let mut pos = self.start.clone();
        let mut out = Vec::new();
        for (i, m) in self.line.iter().enumerate() {
            let label = move_label(&pos, *m, i == 0);
            out.push((label, i + 1));
            pos.play_unchecked(*m);
            if i + 1 >= upto {
                break;
            }
        }
        out
    }

    /// The last move shown, with its number ("14…Nf6").
    fn last_label(&self) -> Option<String> {
        let i = self.cursor.checked_sub(1)?;
        let mut pos = self.start.clone();
        for m in &self.line[..i] {
            pos.play_unchecked(*m);
        }
        Some(crate::san::show(&move_label(&pos, *self.line.get(i)?, true)).into_owned())
    }

    /// Text of the position chip: the moves up to 8 half-moves, then "after 14…Nf6".
    pub fn chip_text(&self) -> String {
        let moves = self.san_line(self.cursor);
        let prefix = if self.start_fen.is_some() {
            "🏠 "
        } else {
            ""
        };
        if moves.len() <= 8 {
            let text: Vec<String> = moves
                .into_iter()
                .map(|(s, _)| crate::san::show(&s).into_owned())
                .collect();
            format!("{prefix}{}", text.join(" "))
        } else {
            let last = self.last_label().unwrap_or_default();
            format!("{prefix}{}", trf("after {move}", &[("move", &last)]))
        }
    }

    /// PGN of the moves, for copying or opening as a new game.
    fn pgn(&self) -> String {
        let mut out = String::new();
        if let Some(fen) = &self.start_fen {
            out.push_str(&format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n"));
        }
        let moves: Vec<String> = self
            .san_line(self.cursor)
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        out.push_str(&moves.join(" "));
        out.push_str(" *\n");
        out
    }

    fn go(&mut self, cursor: usize) -> bool {
        let cursor = cursor.min(self.line.len());
        if cursor == self.cursor {
            return false;
        }
        self.cursor = cursor;
        self.board.clear();
        true
    }

    /// Plays a move at the cursor: the rest of the line is replaced unless it starts
    /// with the same move.
    fn play(&mut self, m: Move) {
        if self.line.get(self.cursor) != Some(&m) {
            self.line.truncate(self.cursor);
            self.line.push(m);
        }
        self.cursor += 1;
        self.board.clear();
    }

    fn reset(&mut self, now: f64) {
        if !self.is_set() {
            return;
        }
        let old = std::mem::replace(self, SearchBoard::new());
        self.flipped = old.flipped;
        self.undo = Some(Replaced {
            start: old.start,
            start_fen: old.start_fen,
            line: old.line,
            cursor: old.cursor,
            at: now,
        });
    }

    fn undo_reset(&mut self) {
        if let Some(r) = self.undo.take() {
            self.start = r.start;
            self.start_fen = r.start_fen;
            self.line = r.line;
            self.cursor = r.cursor;
            self.board.clear();
        }
    }

    fn sound_after(&self) -> Sound {
        let pos = self.position();
        let capture = self
            .cursor
            .checked_sub(1)
            .and_then(|i| self.line.get(i))
            .is_some_and(|m| m.is_capture());
        if pos.is_checkmate() {
            Sound::Checkmate
        } else if pos.is_check() {
            Sound::Check
        } else if capture {
            Sound::Capture
        } else {
            Sound::Move
        }
    }

    /// Keys of the board (only while no text field has the focus): arrows, Home/End,
    /// Backspace, 1-9, F, Ctrl+Backspace.
    pub fn keys(
        &mut self,
        ctx: &egui::Context,
        outcome: Option<&PositionOutcome>,
        events: &mut Vec<BoardEvent>,
    ) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let now = ctx.input(|i| i.time);
        let mut changed = false;
        ctx.input_mut(|i| {
            // Ctrl+Backspace (not with Shift: that clears all conditions).
            if !i.modifiers.shift && i.consume_key(Modifiers::COMMAND, Key::Backspace) {
                self.reset(now);
                changed = true;
            }
            if i.consume_key(Modifiers::COMMAND, Key::Z)
                && self
                    .undo
                    .as_ref()
                    .is_some_and(|u| now - u.at < UNDO_SECONDS)
            {
                self.undo_reset();
                changed = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                changed |= self.go(self.cursor.saturating_sub(1));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                changed |= self.go(self.cursor + 1);
            }
            if i.consume_key(Modifiers::NONE, Key::Home) {
                changed |= self.go(0);
            }
            if i.consume_key(Modifiers::NONE, Key::End) {
                changed |= self.go(self.line.len());
            }
            if i.consume_key(Modifiers::NONE, Key::Backspace) && self.cursor > 0 {
                self.line.truncate(self.cursor - 1);
                changed |= self.go(self.cursor - 1);
            }
            if i.consume_key(Modifiers::NONE, Key::F) {
                self.flipped = !self.flipped;
            }
        });
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        if let Some(out) = outcome {
            let pos = self.position();
            for (n, key) in digits.into_iter().enumerate() {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
                    && let Some(m) = out.moves.get(n).and_then(|s| s.uci.to_move(&pos).ok())
                {
                    self.play(m);
                    events.push(BoardEvent::Sound(self.sound_after()));
                    changed = true;
                }
            }
        }
        if changed {
            events.push(BoardEvent::Changed);
        }
    }

    /// The board panel: header, board, line of moves and continuations.
    /// `outcome` belongs to the shown position (`None` while it is being counted);
    /// `table_hover` is the next move of the game under the pointer in the table;
    /// `books` shows continuations as chapters without results.
    pub fn panel(
        &mut self,
        ui: &mut Ui,
        settings: &Settings,
        outcome: Option<&PositionOutcome>,
        counting: Option<f32>,
        table_hover: Option<UciMove>,
        books: bool,
    ) -> Vec<BoardEvent> {
        let mut events = Vec::new();
        let now = ui.input(|i| i.time);
        self.header(ui, now, &mut events);
        ui.add_space(4.0);
        let pos = self.position();
        let hover = self.hover.or(table_hover);
        self.hover = None;
        let wide = ui.available_width() >= WIDE;
        if wide {
            let height = ui.available_height();
            ui.horizontal_top(|ui| {
                let side = (height - 64.0 - 16.0)
                    .min(ui.available_width() - CONTINUATIONS_WIDTH - 24.0)
                    .clamp(240.0, 640.0);
                ui.vertical(|ui| {
                    ui.set_width(side);
                    self.board(ui, settings, &pos, side, hover, &mut events);
                    self.line_ui(ui, &mut events);
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    self.continuations(ui, &pos, outcome, counting, books, &mut events);
                });
            });
        } else {
            let side = (ui.available_width() - 16.0)
                .min(ui.available_height() - 64.0 - 156.0 - 16.0)
                .clamp(240.0, 640.0);
            ui.vertical_centered(|ui| {
                self.board(ui, settings, &pos, side, hover, &mut events);
            });
            self.line_ui(ui, &mut events);
            ui.add_space(4.0);
            self.continuations(ui, &pos, outcome, counting, books, &mut events);
        }
        events
    }

    fn header(&mut self, ui: &mut Ui, now: f64, events: &mut Vec<BoardEvent>) {
        ui.horizontal(|ui| {
            let pos = self.position();
            let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            let (fill, stroke) = if pos.turn() == Color::White {
                (Color32::WHITE, Color32::from_gray(60))
            } else {
                (Color32::from_gray(20), Color32::from_gray(200))
            };
            ui.painter()
                .circle(rect.center(), 5.0, fill, Stroke::new(1.0, stroke));
            let text = if !self.is_set() {
                tr("Starting position").to_string()
            } else {
                let side = if pos.turn() == Color::White {
                    tr("White to move")
                } else {
                    tr("Black to move")
                };
                match self.last_label() {
                    Some(last) => {
                        format!("{side} · {}", trf("after {move}", &[("move", &last)]))
                    }
                    None => side.to_string(),
                }
            };
            ui.label(text);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("☰", |ui| {
                    if ui.button(tr("Open as a new game")).clicked() {
                        events.push(BoardEvent::OpenAsGame(self.pgn()));
                        ui.close();
                    }
                    if ui.button(tr("Copy FEN")).clicked() {
                        let fen = Fen::from_position(&self.position(), EnPassantMode::Legal);
                        events.push(BoardEvent::CopyText(fen.to_string(), tr("FEN copied")));
                        ui.close();
                    }
                    if ui.button(tr("Copy moves (PGN)")).clicked() {
                        events.push(BoardEvent::CopyText(self.pgn(), tr("PGN copied")));
                        ui.close();
                    }
                });
                let undo_open = self
                    .undo
                    .as_ref()
                    .is_some_and(|u| now - u.at < UNDO_SECONDS);
                if undo_open && !self.is_set() {
                    if ui.button(tr("Undo")).on_hover_text("Ctrl+Z").clicked() {
                        self.undo_reset();
                        events.push(BoardEvent::Changed);
                    }
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(500));
                } else if ui
                    .add_enabled(self.is_set(), egui::Button::new(tr("Reset")))
                    .on_hover_text(tr("Starting position (Ctrl+Backspace)"))
                    .clicked()
                {
                    self.reset(now);
                    events.push(BoardEvent::Changed);
                }
                if ui
                    .button("🔃")
                    .on_hover_text(tr("Flip the board (F)"))
                    .clicked()
                {
                    self.flipped = !self.flipped;
                }
            });
        });
    }

    fn board(
        &mut self,
        ui: &mut Ui,
        settings: &Settings,
        pos: &Chess,
        side: f32,
        hover: Option<UciMove>,
        events: &mut Vec<BoardEvent>,
    ) {
        let last_move = self
            .cursor
            .checked_sub(1)
            .and_then(|i| self.line.get(i))
            .and_then(|m| Some((m.from()?, board::ui_target(m))));
        let arrows: Vec<Arrow> = hover
            .and_then(|u| u.to_move(pos).ok())
            .and_then(|m| {
                Some(Arrow {
                    from: m.from()?,
                    to: board::ui_target(&m),
                    color: PREVIEW_ARROW,
                    weight: 1.0,
                })
            })
            .into_iter()
            .collect();
        let view = BoardView {
            pos,
            orientation: if self.flipped {
                Color::Black
            } else {
                Color::White
            },
            last_move,
            arrows: &arrows,
            circles: &[],
            badge: None,
            interactive: true,
            show_coords: settings.show_coords,
            theme: settings.board_theme.theme(),
            pieces: settings.piece_set,
            animation: settings.animation.seconds(),
            premove: None,
            auto_queen: settings.auto_queen,
        };
        let played = ui
            .allocate_ui(vec2(side, side), |ui| view.show(ui, &mut self.board))
            .inner;
        if let Some(m) = played {
            // A click on the board takes the keys away from text fields.
            ui.ctx().memory_mut(|m| m.surrender_focus(egui::Id::NULL));
            self.play(m);
            events.push(BoardEvent::Sound(self.sound_after()));
            events.push(BoardEvent::Changed);
        }
    }

    fn line_ui(&mut self, ui: &mut Ui, events: &mut Vec<BoardEvent>) {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            if ui
                .add_enabled(self.cursor > 0, egui::Button::new("◀"))
                .on_hover_text(tr("Move back (Left arrow)"))
                .clicked()
                && self.go(self.cursor - 1)
            {
                events.push(BoardEvent::Changed);
            }
            if ui
                .add_enabled(self.cursor < self.line.len(), egui::Button::new("▶"))
                .on_hover_text(tr("Move forward (Right arrow)"))
                .clicked()
                && self.go(self.cursor + 1)
            {
                events.push(BoardEvent::Changed);
            }
            egui::ScrollArea::vertical()
                .max_height(60.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if self.start_fen.is_some() {
                            ui.label("🏠").on_hover_text(format!(
                                "{}\n{}",
                                tr("Start position of the game"),
                                self.start_fen.clone().unwrap_or_default()
                            ));
                        }
                        if self.line.is_empty() {
                            ui.label(RichText::new(tr("Play moves on the board")).weak());
                        }
                        let all = self.san_line(self.line.len());
                        let mut clicked = None;
                        for (label, n) in all {
                            let current = n == self.cursor;
                            let mut text =
                                RichText::new(crate::san::show(&label).into_owned()).size(14.0);
                            if n > self.cursor {
                                text = text.weak();
                            }
                            let r = ui.add(egui::Button::selectable(current, text).frame(false));
                            if current {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                clicked = Some(n);
                            }
                        }
                        if let Some(n) = clicked
                            && self.go(n)
                        {
                            events.push(BoardEvent::Changed);
                        }
                    });
                });
        });
    }

    fn continuations(
        &mut self,
        ui: &mut Ui,
        pos: &Chess,
        outcome: Option<&PositionOutcome>,
        counting: Option<f32>,
        books: bool,
        events: &mut Vec<BoardEvent>,
    ) {
        let title = if books {
            tr("Continuations in the book")
        } else {
            tr("Continuations")
        };
        let Some(out) = outcome else {
            ui.label(RichText::new(title).strong());
            ui.add_space(8.0);
            if self.unavailable {
                ui.label(RichText::new("—").weak());
                return;
            }
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                let p = format!("{:.0}", counting.unwrap_or(0.0) * 100.0);
                let text = match counting {
                    _ if self.preparing => {
                        trf("Preparing the position search (once)… {p} %", &[("p", &p)])
                    }
                    Some(c) if c > 0.0 => trf("Counting… {p} %", &[("p", &p)]),
                    _ => tr("Counting…").to_string(),
                };
                ui.label(RichText::new(text).weak());
            });
            return;
        };
        let total: u32 = out.moves.iter().map(|m| m.tally.games).sum::<u32>() + out.ended.games;
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            let n = crate::large_db::grouped(out.matches.len());
            let label = if books {
                trf("· {n} chapters", &[("n", &n)])
            } else {
                trf("· {n} games", &[("n", &n)])
            };
            ui.label(RichText::new(label).weak());
        });
        if out.moves.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(tr("No continuations in this selection")).weak());
        }
        let mut play = None;
        egui::ScrollArea::vertical()
            .id_salt("continuations")
            .auto_shrink([false, true])
            .max_height((ui.available_height() - 24.0).max(60.0))
            .show(ui, |ui| {
                for (i, stat) in out.moves.iter().enumerate() {
                    let r = continuation_row(ui, i, pos, stat, total, books);
                    if r.hovered() {
                        self.hover = Some(stat.uci);
                    }
                    if r.clicked() {
                        play = stat.uci.to_move(pos).ok();
                    }
                }
            });
        if out.ended.games > 0 {
            ui.label(
                RichText::new(trf(
                    "Ended here: {n}",
                    &[("n", &crate::large_db::grouped(out.ended.games as usize))],
                ))
                .weak(),
            );
        }
        if let Some(m) = play {
            self.play(m);
            events.push(BoardEvent::Sound(self.sound_after()));
            events.push(BoardEvent::Changed);
        }
    }
}

/// "3.d4" / "3…Nc6"; the first move of a line starting with Black gets its number too.
fn move_label(pos: &Chess, m: Move, first: bool) -> String {
    let san = SanPlus::from_move(pos.clone(), m).to_string();
    let n = pos.fullmoves().get();
    match pos.turn() {
        Color::White => format!("{n}.{san}"),
        Color::Black if first => format!("{n}…{san}"),
        Color::Black => san,
    }
}

/// One continuation: number, move, games, share, results bar, average rating, year.
pub fn continuation_row(
    ui: &mut Ui,
    i: usize,
    pos: &Chess,
    stat: &MoveStat,
    total: u32,
    books: bool,
) -> egui::Response {
    let height = 22.0;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let visuals = ui.visuals();
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let few = stat.tally.games < 10 && !books;
    let text_color = if few {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    };
    let font = egui::FontId::proportional(13.0);
    let y = rect.center().y;
    let mut x = rect.left() + 4.0;
    let text = |x: f32, s: &str, color: Color32, align: egui::Align2| {
        ui.painter()
            .text(egui::pos2(x, y), align, s, font.clone(), color);
    };
    // Every row is numbered; 1-9 also play the move.
    text(
        x + 18.0,
        &(i + 1).to_string(),
        visuals.weak_text_color(),
        egui::Align2::RIGHT_CENTER,
    );
    x += 26.0;
    let label = stat
        .uci
        .to_move(pos)
        .map(|m| move_label(pos, m, true))
        .unwrap_or_else(|_| stat.san.clone());
    let label = crate::san::show(&label);
    text(x, &label, text_color, egui::Align2::LEFT_CENTER);
    x += 78.0;
    let games = crate::large_db::grouped(stat.tally.games as usize);
    if books {
        text(
            x,
            &trf("{n} chapters", &[("n", &games)]),
            text_color,
            egui::Align2::LEFT_CENTER,
        );
        return resp;
    }
    text(x + 56.0, &games, text_color, egui::Align2::RIGHT_CENTER);
    x += 62.0;
    let share = if total > 0 {
        stat.tally.games as f32 / total as f32
    } else {
        0.0
    };
    let show_share = width >= 330.0;
    if show_share {
        text(
            x + 34.0,
            &format!("{:.0}%", share * 100.0),
            visuals.weak_text_color(),
            egui::Align2::RIGHT_CENTER,
        );
        x += 40.0;
    }
    let tail = if width >= 300.0 { 92.0 } else { 48.0 };
    let bar_width = (rect.right() - tail - x).max(60.0);
    let bar = egui::Rect::from_min_size(egui::pos2(x, y - 7.0), vec2(bar_width, 14.0));
    results_bar(ui, bar, &stat.tally, text_color);
    let resp = resp.on_hover_text(results_text(&stat.tally));
    x = bar.right() + 6.0;
    let elo = stat
        .tally
        .average_elo()
        .map_or_else(|| "—".to_string(), |e| e.to_string());
    text(
        x,
        &elo,
        visuals.weak_text_color(),
        egui::Align2::LEFT_CENTER,
    );
    if width >= 300.0 && stat.tally.last_year > 0 {
        text(
            x + 44.0,
            &stat.tally.last_year.to_string(),
            visuals.weak_text_color(),
            egui::Align2::LEFT_CENTER,
        );
    }
    resp
}

/// White wins (light), draws (grey), Black wins (dark), with the percentages inside
/// wide enough segments: readable without telling colours apart.
pub fn results_bar(ui: &Ui, rect: egui::Rect, t: &Tally, _text: Color32) {
    let decided = t.white_wins + t.draws + t.black_wins;
    let painter = ui.painter();
    if decided == 0 {
        painter.rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Inside,
        );
        return;
    }
    let parts = [
        (
            t.white_wins,
            Color32::from_gray(0xe8),
            Color32::from_gray(0x20),
        ),
        (t.draws, Color32::from_gray(0x8c), Color32::from_gray(0x10)),
        (
            t.black_wins,
            Color32::from_gray(0x2b),
            Color32::from_gray(0xe0),
        ),
    ];
    let mut x = rect.left();
    for (n, fill, ink) in parts {
        let w = rect.width() * n as f32 / decided as f32;
        if w <= 0.0 {
            continue;
        }
        let seg =
            egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w, rect.bottom()));
        painter.rect_filled(seg, 0.0, fill);
        if w >= 26.0 {
            painter.text(
                seg.center(),
                egui::Align2::CENTER_CENTER,
                format!("{:.0}", 100.0 * n as f32 / decided as f32),
                egui::FontId::proportional(10.5),
                ink,
            );
        }
        x += w;
    }
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, Color32::from_gray(0x70)),
        egui::StrokeKind::Inside,
    );
}

fn results_text(t: &Tally) -> String {
    let decided = (t.white_wins + t.draws + t.black_wins).max(1) as f32;
    let pct = |n: u32| format!("{:.0}", 100.0 * n as f32 / decided);
    let mut s = trf(
        "White won {w} ({wp} %) · draws {d} ({dp} %) · Black won {b} ({bp} %)",
        &[
            ("w", &t.white_wins),
            ("wp", &pct(t.white_wins)),
            ("d", &t.draws),
            ("dp", &pct(t.draws)),
            ("b", &t.black_wins),
            ("bp", &pct(t.black_wins)),
        ],
    );
    if t.games < 10 {
        s.push('\n');
        s.push_str(tr("Few games: the statistics are not reliable"));
    }
    s
}

/// "9." after White's move, "9…" after Black's: the move after which a game reached
/// the position, from the number of half-moves.
pub fn ply_label(ply: u32) -> String {
    if ply == 0 {
        "0".into()
    } else if ply % 2 == 1 {
        format!("{}.", ply.div_ceil(2))
    } else {
        format!("{}…", ply / 2)
    }
}

/// A position search running in a background thread.
pub struct Background<T> {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    rx: Receiver<Option<T>>,
    pub started: f64,
}

impl<T: Send + 'static> Background<T> {
    /// Runs `work(progress, cancel)` in a thread; `None` from it means cancelled.
    pub fn spawn(
        ctx: &egui::Context,
        now: f64,
        work: impl FnOnce(&AtomicUsize, &AtomicBool) -> Option<T> + Send + 'static,
    ) -> Option<Background<T>> {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let (c, p, ctx) = (cancel.clone(), progress.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("position-search".into())
            .spawn(move || {
                let r = work(&p, &c);
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok()?;
        Some(Background {
            cancel,
            progress,
            rx,
            started: now,
        })
    }

    /// The result once ready (`Some(None)`: cancelled or failed).
    pub fn poll(&self) -> Option<Option<T>> {
        self.rx.try_recv().ok()
    }

    pub fn progress(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }

    /// Asks the work to stop; its result still arrives.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The move a game played next, drawn as an arrow when its row is under the pointer.
pub fn next_move_of(outcome: &PositionOutcome, index: usize) -> Option<UciMove> {
    let i = outcome
        .matches
        .binary_search_by_key(&(index as u32), |m| m.index)
        .ok()?;
    outcome.matches.get(i)?.next
}

/// Moves from the game's start to `node` (a position taken from a game), or `None`
/// when the path holds a null move (a teaching scheme, not a position of a game).
pub fn line_to(game: &chess_core::Game, node: chess_core::NodeId) -> Option<(Chess, Vec<Move>)> {
    let start = game.position(game.root())?.clone();
    let mut pos = start.clone();
    let mut line = Vec::new();
    for id in game.path(node) {
        if id == game.root() {
            continue;
        }
        let gm = game.game_move(id)?;
        let uci: UciMove = gm.uci()?.parse().ok()?;
        let m = uci.to_move(&pos).ok()?;
        pos.play_unchecked(m);
        line.push(m);
    }
    Some((start, line))
}
