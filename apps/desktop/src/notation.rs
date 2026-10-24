//! Move list: main line as a two-column table (Lichess style), comments and variations
//! as full-width blocks between rows; nested variations inline in parentheses.

use std::collections::HashMap;

use crate::i18n::tr;
use crate::review::{Mark, judgement_color, judgement_name, judgement_symbol};
use crate::translate_ui::Mark as Translated;
use chess_core::shakmaty::{Color, Position};
use chess_core::{Game, Nag, NodeId};
use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, vec2};

pub struct Notation<'a> {
    pub game: &'a Game,
    pub cursor: NodeId,
    pub scroll_to_cursor: bool,
    /// Engine assessments of main-line moves (whole-game analysis), shown apart from NAGs.
    pub marks: Option<&'a HashMap<NodeId, Mark>>,
    /// Engine line shown on the board: (start node, SAN moves, moves played so far).
    pub preview: Option<(NodeId, &'a [String], usize)>,
    /// Translated comments by their original text (none: the original is shown).
    pub translations: Option<&'a HashMap<String, Translated>>,
    /// The comment being written, drawn where the comment stands (user-comments.md §3).
    pub editor: Option<&'a mut CommentEditor>,
}

/// Result of a frame of the notation panel.
#[derive(Default)]
pub struct NotationResponse {
    pub clicked: Option<NodeId>,
    pub context: Option<(NodeId, NotationCommand)>,
    /// A move of the better line of this marked move was clicked: (marked node, moves to show).
    pub best: Option<(NodeId, usize)>,
    /// Open the editor for this comment (a double click, the menu).
    pub edit: Option<Slot>,
    /// Delete this comment (the menu).
    pub delete: Option<Slot>,
    /// Put or take off this symbol on this move (the menu), at once.
    pub nag: Option<(NodeId, Nag)>,
    /// The editor's «Done» or «Cancel».
    pub editor: Option<EditorOutcome>,
}

#[derive(Clone, Copy, Debug)]
pub enum NotationCommand {
    Promote,
    Delete,
}

/// Which comment of the game: after a move (`After(ROOT)`: the start of the game), or
/// before a move (an introduction to a line, only an existing one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    After(NodeId),
    Before(NodeId),
}

impl Slot {
    pub fn node(self) -> NodeId {
        match self {
            Slot::After(id) | Slot::Before(id) => id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    Done,
    Cancel,
}

/// A comment being written: its draft text and symbols (user-comments.md §3.4).
pub struct CommentEditor {
    pub slot: Slot,
    pub text: String,
    /// The text as the editor opened (unchanged: the comment's bytes stay).
    pub initial: String,
    /// The draft symbols of the move (all of them; the two groups are toggled).
    pub nags: Vec<Nag>,
    pub initial_nags: Vec<Nag>,
    /// «Comment after 4…Nf6», and its short form for a narrow panel.
    pub title: String,
    pub short_title: String,
    /// A translation of this comment was on the screen: the original is edited.
    pub translated: bool,
    /// Put the keyboard into the field (on opening, on Enter from the game).
    pub focus: bool,
    /// Scroll the editor into view (on opening).
    pub scroll: bool,
    /// The text field's id, to know whether it has the keyboard.
    pub field_id: Option<egui::Id>,
}

impl CommentEditor {
    /// Something to write: the text or the symbols changed.
    pub fn changed(&self) -> bool {
        self.text.trim() != self.initial.trim() || self.nags != self.initial_nags
    }

    pub fn text_changed(&self) -> bool {
        self.text.trim() != self.initial.trim()
    }
}

/// The six symbols of a move and the eight of a position, as the editor and the move's
/// menu offer them (user-comments.md §3.4): (NAG, symbol, name).
pub const MOVE_NAGS: [(Nag, &str, &str); 6] = [
    (Nag(1), "!", "Good move"),
    (Nag(2), "?", "Mistake"),
    (Nag(3), "!!", "Brilliant move"),
    (Nag(4), "??", "Blunder"),
    (Nag(5), "!?", "Interesting move"),
    (Nag(6), "?!", "Dubious move"),
];
pub const POSITION_NAGS: [(Nag, &str, &str); 8] = [
    (Nag(10), "=", "Equal position"),
    (Nag(13), "∞", "Unclear position"),
    (Nag(14), "⩲", "White is slightly better"),
    (Nag(15), "⩱", "Black is slightly better"),
    (Nag(16), "±", "White is better"),
    (Nag(17), "∓", "Black is better"),
    (Nag(18), "+−", "White is winning"),
    (Nag(19), "−+", "Black is winning"),
];

fn is_position_nag(n: Nag) -> bool {
    (10..=19).contains(&n.0)
}

/// `nag` put on or taken off `nags`: one symbol of its group (move or position) at a
/// time; the same symbol again takes it off; the other symbols stay.
pub fn toggle_nag(nags: &[Nag], nag: Nag) -> Vec<Nag> {
    let same_group = |n: &Nag| {
        if nag.is_move_assessment() {
            n.is_move_assessment()
        } else {
            is_position_nag(*n)
        }
    };
    let had = nags
        .iter()
        .any(|n| *n == nag || (n.0 >= 10 && n.0 <= 12 && nag.0 == 10));
    let mut out: Vec<Nag> = nags.iter().copied().filter(|n| !same_group(n)).collect();
    if !had {
        out.push(nag);
        out.sort();
    }
    out
}

/// The symbol of a group that is on: `=` for $10–$12.
fn group_current(nags: &[Nag], position: bool) -> Option<Nag> {
    nags.iter()
        .copied()
        .find(|n| {
            if position {
                is_position_nag(*n)
            } else {
                n.is_move_assessment()
            }
        })
        .map(|n| if (10..=12).contains(&n.0) { Nag(10) } else { n })
}

const ROW_H: f32 = 26.0;
const NUM_W: f32 = 38.0;
/// A move's cell is no wider than this: in a wide panel (the notation under the board)
/// the two moves of a row stay together.
const CELL_MAX: f32 = 160.0;
/// Moves of an engine's better line shown under a marked move.
const MARK_LINE_MOVES: usize = 6;

impl Notation<'_> {
    pub fn show(mut self, ui: &mut Ui) -> NotationResponse {
        let mut out = NotationResponse::default();
        let mut editor = self.editor.take();
        let game = self.game;
        let editing = editor.as_ref().map(|e| e.slot);
        let root_comments = comment_text(
            game.node(NodeId::ROOT)
                .map(|n| n.comments_after())
                .unwrap_or(&[]),
        );
        if editing == Some(Slot::After(NodeId::ROOT)) {
            editor_block(ui, &mut editor, &mut out);
        } else if !root_comments.is_empty() {
            comment_block(
                ui,
                &self,
                &root_comments,
                Slot::After(NodeId::ROOT),
                &mut out,
            );
        }

        let mut row = Row::default();
        let mut node = NodeId::ROOT;
        while let Some(child) = game.main_child(node) {
            let Some(n) = game.node(child) else { break };
            let before = comment_text(n.comments_before());
            if editing == Some(Slot::Before(child)) {
                row.flush(ui, &self, &mut out);
                editor_block(ui, &mut editor, &mut out);
            } else if !before.is_empty() {
                row.flush(ui, &self, &mut out);
                comment_block(ui, &self, &before, Slot::Before(child), &mut out);
            }
            let (num, color) = game.move_number(child).unwrap_or((0, Color::White));
            if color == Color::White {
                row.flush(ui, &self, &mut out);
                row.num = num;
                row.white = Some(child);
            } else {
                if row.white.is_none() {
                    row.num = num;
                }
                row.black = Some(child);
            }
            let after = comment_text(n.comments_after());
            let vars = game.variations(node);
            let mark = self.marks.and_then(|m| m.get(&child));
            let editing_after = editing == Some(Slot::After(child));
            let interrupted = editing_after
                || !after.is_empty()
                || !vars.is_empty()
                || !n.unparsed().is_empty()
                || mark.is_some();
            if interrupted || color == Color::Black {
                row.flush(ui, &self, &mut out);
                if let Some(mark) = mark {
                    mark_block(ui, &self, child, mark, &mut out);
                }
                if editing_after {
                    editor_block(ui, &mut editor, &mut out);
                } else if !after.is_empty() {
                    comment_block(ui, &self, &after, Slot::After(child), &mut out);
                }
                for u in n.unparsed() {
                    unparsed_block(ui, u);
                }
                for &v in vars {
                    variation_block(ui, &self, v, &mut editor, &mut out);
                }
            }
            node = child;
        }
        row.flush(ui, &self, &mut out);

        let result = game.result().as_str();
        if result != "*" {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(result).strong().size(16.0)));
        }
        out
    }
}

#[derive(Default)]
struct Row {
    num: u32,
    white: Option<NodeId>,
    black: Option<NodeId>,
}

impl Row {
    fn flush(&mut self, ui: &mut Ui, n: &Notation, out: &mut NotationResponse) {
        if self.white.is_none() && self.black.is_none() {
            return;
        }
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
        let painter = ui.painter();
        let visuals = ui.visuals();
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, vec2(NUM_W, ROW_H)),
            0.0,
            visuals.faint_bg_color,
        );
        painter.text(
            rect.left_center() + vec2(NUM_W - 8.0, 0.0),
            Align2::RIGHT_CENTER,
            self.num.to_string(),
            FontId::proportional(14.0),
            visuals.weak_text_color(),
        );
        let cell_w = ((width - NUM_W) / 2.0).min(CELL_MAX);
        let cells = [(self.white, 0.0), (self.black, cell_w)];
        for (id, off) in cells {
            let cell =
                egui::Rect::from_min_size(rect.min + vec2(NUM_W + off, 0.0), vec2(cell_w, ROW_H));
            match id {
                Some(id) => {
                    let resp = ui.interact(cell, ui.id().with(("mv", id.index())), Sense::click());
                    paint_move_cell(ui, cell, n.game, id, id == n.cursor, resp.hovered());
                    if let Some(mark) = n.marks.and_then(|m| m.get(&id)) {
                        paint_badge(ui, cell, mark);
                    }
                    handle_move_response(ui, &resp, n, id, out);
                }
                None => {
                    ui.painter().text(
                        cell.left_center() + vec2(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        "…",
                        FontId::proportional(15.0),
                        ui.visuals().weak_text_color(),
                    );
                }
            }
        }
        *self = Row::default();
    }
}

fn handle_move_response(
    ui: &Ui,
    resp: &egui::Response,
    n: &Notation,
    id: NodeId,
    out: &mut NotationResponse,
) {
    if resp.clicked() {
        out.clicked = Some(id);
    }
    if n.scroll_to_cursor && id == n.cursor {
        resp.scroll_to_me(Some(egui::Align::Center));
    }
    let is_variation = n.game.child_index(id).is_some_and(|i| i > 0) || !n.game.is_mainline(id);
    // The menu is about the move it opens on: the move becomes the current one.
    if resp.secondary_clicked() && id != n.cursor {
        out.clicked = Some(id);
    }
    resp.context_menu(|ui| {
        let node = n.game.node(id);
        let has_comment = node.is_some_and(|nd| !comment_text(nd.comments_after()).is_empty());
        let label = if has_comment {
            tr("Edit comment…")
        } else {
            tr("Comment…")
        };
        if ui
            .add(egui::Button::new(label).shortcut_text("Enter"))
            .clicked()
        {
            out.edit = Some(Slot::After(id));
            ui.close();
        }
        if has_comment && ui.button(tr("Delete comment")).clicked() {
            out.delete = Some(Slot::After(id));
            ui.close();
        }
        // The symbols act at once and keep the menu open: «?» off, «?!» on.
        let nags = node.map(|nd| nd.nags().to_vec()).unwrap_or_default();
        symbol_row(ui, &nags, &MOVE_NAGS, true, &mut |nag| {
            out.nag = Some((id, nag))
        });
        symbol_row(ui, &nags, &POSITION_NAGS, false, &mut |nag| {
            out.nag = Some((id, nag))
        });
        if n.game.main_child(NodeId::ROOT) == Some(id) {
            ui.separator();
            if ui.button(tr("Comment at the start of the game…")).clicked() {
                out.edit = Some(Slot::After(NodeId::ROOT));
                ui.close();
            }
        }
        ui.separator();
        if is_variation && ui.button(tr("Make main line")).clicked() {
            out.context = Some((id, NotationCommand::Promote));
            ui.close();
        }
        if ui.button(tr("Delete move and continuation")).clicked() {
            out.context = Some((id, NotationCommand::Delete));
            ui.close();
        }
    });
    let _ = ui;
}

/// A row of symbol toggles: the one on is pressed; `on` gets the symbol clicked.
fn symbol_row(
    ui: &mut Ui,
    nags: &[Nag],
    set: &[(Nag, &str, &'static str)],
    moves: bool,
    on: &mut dyn FnMut(Nag),
) {
    let current = group_current(nags, !moves);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (nag, sym, name) in set {
            let mut text = RichText::new(*sym).size(15.0);
            if moves && let Some(c) = assessment_color(*nag) {
                text = text.color(c);
            }
            let r = ui
                .add_sized(
                    [28.0, 24.0],
                    egui::Button::selectable(current == Some(*nag), text),
                )
                .on_hover_text(symbol_hint(*nag, name));
            if r.clicked() {
                on(*nag);
            }
        }
    });
}

/// «Good move (Ctrl+1)».
fn symbol_hint(nag: Nag, name: &'static str) -> String {
    if nag.is_move_assessment() {
        format!("{} (Ctrl+{})", tr(name), nag.0)
    } else {
        tr(name).to_owned()
    }
}

fn paint_move_cell(
    ui: &Ui,
    cell: egui::Rect,
    game: &Game,
    id: NodeId,
    current: bool,
    hovered: bool,
) {
    let visuals = ui.visuals();
    if current {
        ui.painter()
            .rect_filled(cell, 2.0, visuals.selection.bg_fill);
    } else if hovered {
        ui.painter()
            .rect_filled(cell, 2.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let node = game.node(id);
    let san = game.game_move(id).map(|m| m.san.as_str()).unwrap_or("?");
    let san = crate::san::show(san);
    let (sym, color) = node
        .map(|n| move_glyph(n.nags()))
        .unwrap_or((String::new(), None));
    let text_color = if current {
        visuals.selection.stroke.color
    } else {
        color.unwrap_or(visuals.text_color())
    };
    ui.painter().text(
        cell.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        format!("{san}{sym}"),
        FontId::proportional(16.0),
        text_color,
    );
}

/// Engine verdict at the right edge of a move cell: a filled pill, unlike the author's NAGs.
fn paint_badge(ui: &Ui, cell: egui::Rect, mark: &Mark) {
    let color = judgement_color(mark.judgement);
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(
        judgement_symbol(mark.judgement).to_string(),
        font,
        Color32::WHITE,
    );
    let size = vec2(galley.size().x + 10.0, 16.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(cell.right() - size.x - 6.0, cell.center().y - size.y / 2.0),
        size,
    );
    ui.painter().rect_filled(rect, 8.0, color);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// "Mistake. Best: 23…Nf6 24.Nd5 …" under a marked main-line move. Each move opens the line
/// on the board up to it; the move shown on the board is highlighted like the game cursor.
fn mark_block(ui: &mut Ui, n: &Notation, id: NodeId, mark: &Mark, out: &mut NotationResponse) {
    let color = judgement_color(mark.judgement);
    let start = n
        .game
        .position(mark.base)
        .map(|p| (p.fullmoves().get(), p.turn()));
    let shown = n
        .preview
        .filter(|(base, san, _)| *base == mark.base && *san == mark.best_san.as_slice())
        .map(|(_, _, shown)| shown);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 3,
            bottom: 3,
        })
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(3.0, 2.0);
                ui.label(
                    RichText::new(format!("{}.", judgement_name(mark.judgement)))
                        .color(color)
                        .strong(),
                );
                if mark.best_san.is_empty() {
                    return;
                }
                ui.label(RichText::new(tr("Best:")).weak());
                // Short by default; grows while the line is stepped through on the board.
                let visible = MARK_LINE_MOVES.max(shown.unwrap_or(0));
                for (i, san) in mark.best_san.iter().enumerate().take(visible) {
                    let label = match start {
                        Some((num, turn)) => {
                            let ply = i as u32 + u32::from(turn == Color::Black);
                            let num = num + ply / 2;
                            if ply.is_multiple_of(2) {
                                format!("{num}.{san}")
                            } else if i == 0 {
                                format!("{num}…{san}")
                            } else {
                                san.clone()
                            }
                        }
                        None => san.clone(),
                    };
                    let label = crate::san::show(&label).into_owned();
                    let text = RichText::new(label).size(14.5);
                    let current = shown == Some(i + 1);
                    let button = if current {
                        let sel = ui.visuals().selection;
                        egui::Button::new(text.color(sel.stroke.color).strong())
                            .fill(sel.bg_fill)
                            .stroke(Stroke::new(1.0, sel.stroke.color))
                    } else {
                        egui::Button::new(text).frame_when_inactive(false)
                    };
                    let resp = ui
                        .add(button)
                        .on_hover_text(tr("Show this line on the board"));
                    if current && n.scroll_to_cursor {
                        resp.scroll_to_me(Some(egui::Align::Center));
                    }
                    if resp.clicked() {
                        out.best = Some((id, i + 1));
                    }
                }
                if mark.best_san.len() > visible {
                    ui.label(RichText::new("…").weak());
                }
            });
        });
}

/// Colour of a move-assessment NAG ($1-$6), the Lichess palette.
pub fn assessment_color(nag: Nag) -> Option<Color32> {
    Some(match nag.0 {
        1 => Color32::from_rgb(0x62, 0x9c, 0x2e),
        2 => Color32::from_rgb(0xe6, 0x8f, 0x00),
        3 => Color32::from_rgb(0x22, 0xac, 0x9a),
        4 => Color32::from_rgb(0xdf, 0x53, 0x53),
        5 => Color32::from_rgb(0xea, 0x45, 0xd8),
        6 => Color32::from_rgb(0x56, 0xb4, 0xe9),
        _ => return None,
    })
}

/// Move-assessment glyph (`!`, `?`, …) with its color, plus position assessments.
fn move_glyph(nags: &[Nag]) -> (String, Option<Color32>) {
    let mut s = String::new();
    let mut color = None;
    for nag in nags {
        let Some(sym) = nag.symbol() else {
            s.push_str(&format!(" ${}", nag.0));
            continue;
        };
        if nag.is_move_assessment() {
            let Some(c) = assessment_color(*nag) else {
                continue;
            };
            color = Some(c);
            s.push_str(sym);
        } else {
            s.push(' ');
            s.push_str(sym);
        }
    }
    (s, color)
}

/// The text of comments as shown (see [`chess_core::comment_display_text`]).
pub fn comment_text(comments: &[String]) -> String {
    chess_core::comment_display_text(comments)
}

fn block_frame(ui: &Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
}

/// The text to show for a comment and its translation mark.
fn translated<'a>(n: &'a Notation, text: &'a str) -> (&'a str, Option<&'a Translated>) {
    match n.translations.and_then(|t| t.get(text)) {
        Some(m @ Translated::Translated(t)) => (t.as_str(), Some(m)),
        m => (text, m),
    }
}

/// "⇄" (machine translation, the original in the tooltip), "⋯" (on its way), "⚠" (failed).
/// "⇄" (machine translation, the original in the tooltip), "⋯" (on its way), "⚠" (failed),
/// drawn at `at` without taking part in the layout (it may not widen the panel).
fn translation_mark(ui: &mut Ui, at: egui::Pos2, mark: &Translated, original: &str) {
    let (sym, color, tip, name) = mark_info(ui, mark, original);
    let rect = ui.painter().text(
        at,
        Align2::RIGHT_TOP,
        sym,
        FontId::proportional(13.0),
        color,
    );
    let id = ui.id().with(("mark", at.x as i32, at.y as i32));
    let r = ui.interact(rect.expand(2.0), id, Sense::hover());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, name));
    r.on_hover_ui(|ui| {
        ui.set_max_width(360.0);
        ui.label(tip);
    });
}

/// The mark after a comment inside a line of text (a variation): part of the flow.
fn inline_mark(ui: &mut Ui, mark: &Translated, original: &str) {
    let (sym, color, tip, name) = mark_info(ui, mark, original);
    let r =
        ui.add(egui::Label::new(RichText::new(sym).size(13.0).color(color)).sense(Sense::hover()));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, name));
    r.on_hover_ui(|ui| {
        ui.set_max_width(360.0);
        ui.label(tip);
    });
}

fn mark_info(
    ui: &Ui,
    mark: &Translated,
    original: &str,
) -> (&'static str, Color32, String, &'static str) {
    match mark {
        Translated::Translated(_) => {
            let mut o: String = original.chars().take(1000).collect();
            if original.chars().count() > 1000 {
                o.push('…');
            }
            (
                "⇄",
                ui.visuals().weak_text_color(),
                format!("{}\n{o}", tr("Machine translation. Original:")),
                tr("Machine translation"),
            )
        }
        Translated::Pending => (
            "⋯",
            ui.visuals().weak_text_color(),
            tr("Translating…").to_owned(),
            tr("Translating…"),
        ),
        Translated::Bad => (
            "⚠",
            ui.visuals().warn_fg_color,
            tr("This comment could not be translated — the original is shown. Press “Retry” in the line above the notation.").to_owned(),
            tr("Not translated"),
        ),
    }
}

/// Room kept on the right of a comment for its translation mark.
const MARK_ROOM: f32 = 18.0;

fn comment_block(ui: &mut Ui, n: &Notation, text: &str, slot: Slot, out: &mut NotationResponse) {
    let (shown, mark) = translated(n, text);
    let mut frame = block_frame(ui);
    if mark.is_some() {
        frame = frame.inner_margin(egui::Margin {
            left: 10,
            right: 10 + MARK_ROOM as i8,
            top: 6,
            bottom: 6,
        });
    }
    let r = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(shown).color(comment_color(ui)).size(14.5));
    });
    // The whole block, not only its text, takes the double click and the right button.
    let resp = ui.interact(
        r.response.rect,
        ui.id().with((
            "comment",
            slot.node().index(),
            matches!(slot, Slot::Before(_)),
        )),
        Sense::click(),
    );
    comment_response(&resp, slot, out);
    if let Some(m) = mark {
        let at = r.response.rect.right_top() + vec2(-8.0, 7.0);
        translation_mark(ui, at, m, text);
    }
}

/// A double click opens the comment for editing; the right button offers that and
/// deleting (user-comments.md §5.2).
fn comment_response(resp: &egui::Response, slot: Slot, out: &mut NotationResponse) {
    if resp.hovered() {
        resp.ctx.set_cursor_icon(egui::CursorIcon::Text);
    }
    if resp.double_clicked() {
        out.edit = Some(slot);
    }
    resp.context_menu(|ui| {
        if ui.button(tr("Edit comment…")).clicked() {
            out.edit = Some(slot);
            ui.close();
        }
        if ui.button(tr("Delete comment")).clicked() {
            out.delete = Some(slot);
            ui.close();
        }
    });
}

/// The comment editor on the spot of its comment (user-comments.md §3.4).
fn editor_block(ui: &mut Ui, editor: &mut Option<&mut CommentEditor>, out: &mut NotationResponse) {
    let Some(ed) = editor.as_deref_mut() else {
        return;
    };
    let focused = ed
        .field_id
        .is_some_and(|id| ui.ctx().memory(|m| m.focused() == Some(id)));
    let sel = ui.visuals().selection;
    let stroke = if focused {
        Stroke::new(1.5, sel.stroke.color)
    } else {
        Stroke::new(1.0, ui.visuals().weak_text_color())
    };
    let width = ui.available_width();
    let narrow = width < 360.0;
    let r = egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .stroke(stroke)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The title and the buttons.
            ui.horizontal(|ui| {
                let buttons = if narrow { 2.0 * 30.0 } else { 150.0 };
                let room = ui.available_width() - buttons;
                let title = if narrow
                    || crate::game_view::text_width(ui, &ed.title, egui::TextStyle::Body) > room
                {
                    &ed.short_title
                } else {
                    &ed.title
                };
                ui.add(egui::Label::new(RichText::new(title).strong()).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if narrow {
                        if ui
                            .add(egui::Button::new("×").min_size(vec2(0.0, 30.0)))
                            .on_hover_text(tr("Cancel (Esc)"))
                            .clicked()
                        {
                            out.editor = Some(EditorOutcome::Cancel);
                        }
                        if crate::widgets::primary(ui, "✔")
                            .on_hover_text(tr("Done (Enter)"))
                            .clicked()
                        {
                            out.editor = Some(EditorOutcome::Done);
                        }
                    } else {
                        if ui
                            .add(egui::Button::new(tr("Cancel")).min_size(vec2(0.0, 30.0)))
                            .on_hover_text("Esc")
                            .clicked()
                        {
                            out.editor = Some(EditorOutcome::Cancel);
                        }
                        if crate::widgets::primary(ui, tr("Done"))
                            .on_hover_text("Enter")
                            .clicked()
                        {
                            out.editor = Some(EditorOutcome::Done);
                        }
                    }
                });
            });
            // The symbols: the move's six, then the position's eight (a «±▾» menu when
            // they do not fit on the line). Not for the start of the game.
            if ed.slot != Slot::After(NodeId::ROOT) && !matches!(ed.slot, Slot::Before(_)) {
                let both = 14.0 * 30.0 + 24.0 <= ui.available_width();
                ui.horizontal(|ui| {
                    let mut toggled = None;
                    symbol_row(ui, &ed.nags, &MOVE_NAGS, true, &mut |nag| {
                        toggled = Some(nag)
                    });
                    if both {
                        ui.separator();
                        symbol_row(ui, &ed.nags, &POSITION_NAGS, false, &mut |nag| {
                            toggled = Some(nag)
                        });
                    } else {
                        let current = group_current(&ed.nags, true)
                            .and_then(|n| n.symbol())
                            .unwrap_or("±");
                        ui.menu_button(format!("{current}▾"), |ui| {
                            symbol_row(ui, &ed.nags, &POSITION_NAGS[..4], false, &mut |nag| {
                                toggled = Some(nag)
                            });
                            symbol_row(ui, &ed.nags, &POSITION_NAGS[4..], false, &mut |nag| {
                                toggled = Some(nag)
                            });
                        })
                        .response
                        .on_hover_text(tr("Position assessment"));
                    }
                    if let Some(nag) = toggled {
                        ed.nags = toggle_nag(&ed.nags, nag);
                    }
                });
            }
            // The text: 2 to 12 lines, then it scrolls inside.
            let color = comment_color(ui);
            let row_h = ui.fonts_mut(|f| f.row_height(&FontId::proportional(14.5)));
            let resp = egui::ScrollArea::vertical()
                .id_salt("comment_editor_scroll")
                .max_height(12.0 * row_h + 8.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut ed.text)
                            .id_salt("comment_editor")
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .font(FontId::proportional(14.5))
                            .text_color(color)
                            .hint_text(tr("Your comment…")),
                    )
                })
                .inner;
            ed.field_id = Some(resp.id);
            if std::mem::take(&mut ed.focus) {
                resp.request_focus();
                // The cursor at the end: one adds to a comment more often than replaces it.
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                    let end = egui::text::CCursor::new(ed.text.chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                    state.store(ui.ctx(), resp.id);
                }
            }
            if !narrow {
                ui.label(
                    RichText::new(tr(
                        "Enter — done · Esc — cancel · Shift+Enter — new paragraph",
                    ))
                    .weak()
                    .small(),
                );
            }
            if ed.translated {
                ui.label(
                    RichText::new(tr(
                        "⇄ You are editing the original; its translation will be dropped",
                    ))
                    .weak()
                    .small(),
                );
            }
        });
    if std::mem::take(&mut ed.scroll) {
        r.response.scroll_to_me(None);
    }
}

fn unparsed_block(ui: &mut Ui, text: &str) {
    block_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(text)
                .monospace()
                .color(ui.visuals().warn_fg_color),
        )
        .on_hover_text(tr("PGN fragment that could not be parsed; kept as is"));
    });
}

fn comment_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x9e, 0xc4, 0xe0)
    } else {
        Color32::from_rgb(0x2a, 0x5b, 0x8a)
    }
}

/// One piece of a variation's flowing text.
enum Token {
    Move {
        id: NodeId,
        label: String,
        color: Option<Color32>,
    },
    Comment {
        slot: Slot,
        text: String,
    },
    Unparsed(String),
    Open,
    Close,
}

fn variation_block(
    ui: &mut Ui,
    n: &Notation,
    start: NodeId,
    editor: &mut Option<&mut CommentEditor>,
    out: &mut NotationResponse,
) {
    let mut tokens = Vec::new();
    line_tokens(n, start, &mut tokens);
    let editing = editor.as_ref().map(|e| e.slot);
    // The flow is cut where the editor stands: after the move for «after», before it
    // for «before»; a comment being edited is replaced by the editor.
    let cut = editing.and_then(|slot| {
        tokens
            .iter()
            .position(|t| matches!(t, Token::Comment { slot: s, .. } if *s == slot))
            .map(|i| (i, true))
            .or_else(|| match slot {
                Slot::After(id) => tokens
                    .iter()
                    .position(|t| matches!(t, Token::Move { id: m, .. } if *m == id))
                    .map(|i| (i + 1, false)),
                Slot::Before(id) => tokens
                    .iter()
                    .position(|t| matches!(t, Token::Move { id: m, .. } if *m == id))
                    .map(|i| (i, false)),
            })
    });
    block_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match cut {
            None => flow(ui, n, &tokens, out),
            Some((at, replaces)) => {
                flow(ui, n, &tokens[..at], out);
                editor_block(ui, editor, out);
                let rest = if replaces { at + 1 } else { at };
                flow(ui, n, &tokens[rest..], out);
            }
        }
    });
}

/// Lays out tokens as flowing text.
fn flow(ui: &mut Ui, n: &Notation, tokens: &[Token], out: &mut NotationResponse) {
    if tokens.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 2.0);
        for t in tokens {
            match t {
                Token::Move { id, label, color } => {
                    let current = *id == n.cursor;
                    let mut text = RichText::new(label).size(15.0);
                    if let Some(c) = color
                        && !current
                    {
                        text = text.color(*c);
                    }
                    // Same strong highlight as the main-line table, so the current move is
                    // visible inside text.
                    let button = if current {
                        let sel = ui.visuals().selection;
                        egui::Button::new(text.color(sel.stroke.color).strong())
                            .fill(sel.bg_fill)
                            .stroke(egui::Stroke::new(1.0, sel.stroke.color))
                    } else {
                        egui::Button::new(text).frame_when_inactive(false)
                    };
                    let resp = ui.add(button);
                    handle_move_response(ui, &resp, n, *id, out);
                }
                Token::Comment { slot, text } => {
                    let (shown, mark) = translated(n, text);
                    let resp = ui.add(
                        egui::Label::new(RichText::new(shown).color(comment_color(ui)).size(14.5))
                            .sense(Sense::click()),
                    );
                    comment_response(&resp, *slot, out);
                    if let Some(m) = mark {
                        inline_mark(ui, m, text);
                    }
                }
                Token::Unparsed(u) => {
                    ui.label(
                        RichText::new(u)
                            .monospace()
                            .color(ui.visuals().warn_fg_color),
                    );
                }
                Token::Open => {
                    ui.label(RichText::new("(").weak());
                }
                Token::Close => {
                    ui.label(RichText::new(")").weak());
                }
            }
        }
    });
}

/// The tokens of a line starting at `first` (a variation move), nested lines inside.
fn line_tokens(n: &Notation, first: NodeId, tokens: &mut Vec<Token>) {
    let game = n.game;
    let mut node = first;
    let mut need_number = true;
    loop {
        let Some(nd) = game.node(node) else { return };
        let before = comment_text(nd.comments_before());
        if !before.is_empty() {
            tokens.push(Token::Comment {
                slot: Slot::Before(node),
                text: before,
            });
            need_number = true;
        }
        let (num, color) = game.move_number(node).unwrap_or((0, Color::White));
        let san = game.game_move(node).map(|m| m.san.as_str()).unwrap_or("?");
        let san = crate::san::show(san);
        let (sym, sym_color) = move_glyph(nd.nags());
        let label = match (color, need_number) {
            (Color::White, _) => format!("{num}.{san}{sym}"),
            (Color::Black, true) => format!("{num}…{san}{sym}"),
            (Color::Black, false) => format!("{san}{sym}"),
        };
        need_number = false;
        tokens.push(Token::Move {
            id: node,
            label,
            color: sym_color,
        });
        let after = comment_text(nd.comments_after());
        if !after.is_empty() {
            tokens.push(Token::Comment {
                slot: Slot::After(node),
                text: after,
            });
            need_number = true;
        }
        for u in nd.unparsed() {
            tokens.push(Token::Unparsed(u.clone()));
        }
        // Alternatives to this move are printed right after it (PGN order),
        // except for the line's own first move whose siblings belong to the outer level.
        if node != first
            && let Some(parent) = game.parent(node)
        {
            for &v in game.variations(parent) {
                tokens.push(Token::Open);
                line_tokens(n, v, tokens);
                tokens.push(Token::Close);
                need_number = true;
            }
        }
        match game.main_child(node) {
            Some(next) => node = next,
            None => return,
        }
    }
}
