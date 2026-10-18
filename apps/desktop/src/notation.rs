//! Move list: main line as a two-column table (Lichess style), comments and variations
//! as full-width blocks between rows; nested variations inline in parentheses.

use std::collections::HashMap;

use crate::i18n::tr;
use crate::review::{Mark, judgement_color, judgement_name, judgement_symbol};
use chess_core::shakmaty::{Color, Position};
use chess_core::{CommentPart, Game, Nag, NodeId, parse_comment};
use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, vec2};

pub struct Notation<'a> {
    pub game: &'a Game,
    pub cursor: NodeId,
    pub scroll_to_cursor: bool,
    /// Engine assessments of main-line moves (whole-game analysis), shown apart from NAGs.
    pub marks: Option<&'a HashMap<NodeId, Mark>>,
    /// Engine line shown on the board: (start node, SAN moves, moves played so far).
    pub preview: Option<(NodeId, &'a [String], usize)>,
}

/// Result of a frame of the notation panel.
#[derive(Default)]
pub struct NotationResponse {
    pub clicked: Option<NodeId>,
    pub context: Option<(NodeId, NotationCommand)>,
    /// A move of the better line of this marked move was clicked: (marked node, moves to show).
    pub best: Option<(NodeId, usize)>,
}

#[derive(Clone, Copy, Debug)]
pub enum NotationCommand {
    Promote,
    Delete,
}

const ROW_H: f32 = 26.0;
const NUM_W: f32 = 38.0;
/// Moves of an engine's better line shown under a marked move.
const MARK_LINE_MOVES: usize = 6;

impl Notation<'_> {
    pub fn show(self, ui: &mut Ui) -> NotationResponse {
        let mut out = NotationResponse::default();
        let game = self.game;
        let root_comments = comment_text(
            game.node(NodeId::ROOT)
                .map(|n| n.comments_after())
                .unwrap_or(&[]),
        );
        if !root_comments.is_empty() {
            comment_block(ui, &root_comments);
        }

        let mut row = Row::default();
        let mut node = NodeId::ROOT;
        while let Some(child) = game.main_child(node) {
            let Some(n) = game.node(child) else { break };
            let before = comment_text(n.comments_before());
            if !before.is_empty() {
                row.flush(ui, &self, &mut out);
                comment_block(ui, &before);
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
            let interrupted =
                !after.is_empty() || !vars.is_empty() || !n.unparsed().is_empty() || mark.is_some();
            if interrupted || color == Color::Black {
                row.flush(ui, &self, &mut out);
                if let Some(mark) = mark {
                    mark_block(ui, &self, child, mark, &mut out);
                }
                if !after.is_empty() {
                    comment_block(ui, &after);
                }
                for u in n.unparsed() {
                    unparsed_block(ui, u);
                }
                for &v in vars {
                    variation_block(ui, &self, v, &mut out);
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
        let cell_w = (width - NUM_W) / 2.0;
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
    resp.context_menu(|ui| {
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

fn comment_text(comments: &[String]) -> String {
    let mut out = String::new();
    for c in comments {
        for part in parse_comment(c) {
            if let CommentPart::Text(t) = part {
                // Single line breaks are formatting, not content: reflow to the panel width.
                // A blank line starts a new paragraph (book chapters imported from ChessBase).
                let mut blank = false;
                for line in t.lines() {
                    if line.trim().is_empty() {
                        blank = true;
                        continue;
                    }
                    if blank && !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    blank = false;
                    for word in line.split_whitespace() {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push(' ');
                        }
                        out.push_str(word);
                    }
                }
            }
        }
    }
    out
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

fn comment_block(ui: &mut Ui, text: &str) {
    block_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(text).color(comment_color(ui)).size(14.5));
    });
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

fn variation_block(ui: &mut Ui, n: &Notation, start: NodeId, out: &mut NotationResponse) {
    block_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 2.0);
            inline_line(ui, n, start, out);
        });
    });
}

/// Renders a line starting at `first` (a variation move) as flowing text.
fn inline_line(ui: &mut Ui, n: &Notation, first: NodeId, out: &mut NotationResponse) {
    let game = n.game;
    let mut node = first;
    let mut need_number = true;
    loop {
        let Some(nd) = game.node(node) else { return };
        let before = comment_text(nd.comments_before());
        if !before.is_empty() {
            ui.label(RichText::new(before).color(comment_color(ui)));
            need_number = true;
        }
        let (num, color) = game.move_number(node).unwrap_or((0, Color::White));
        let san = game.game_move(node).map(|m| m.san.as_str()).unwrap_or("?");
        let (sym, sym_color) = move_glyph(nd.nags());
        let label = match (color, need_number) {
            (Color::White, _) => format!("{num}.{san}{sym}"),
            (Color::Black, true) => format!("{num}…{san}{sym}"),
            (Color::Black, false) => format!("{san}{sym}"),
        };
        need_number = false;
        let current = node == n.cursor;
        let mut text = RichText::new(label).size(15.0);
        if let Some(c) = sym_color
            && !current
        {
            text = text.color(c);
        }
        // Same strong highlight as the main-line table, so the current move is visible inside text.
        let button = if current {
            let sel = ui.visuals().selection;
            egui::Button::new(text.color(sel.stroke.color).strong())
                .fill(sel.bg_fill)
                .stroke(egui::Stroke::new(1.0, sel.stroke.color))
        } else {
            egui::Button::new(text).frame_when_inactive(false)
        };
        let resp = ui.add(button);
        handle_move_response(ui, &resp, n, node, out);

        let after = comment_text(nd.comments_after());
        if !after.is_empty() {
            ui.label(RichText::new(after).color(comment_color(ui)));
            need_number = true;
        }
        for u in nd.unparsed() {
            ui.label(
                RichText::new(u)
                    .monospace()
                    .color(ui.visuals().warn_fg_color),
            );
        }
        // Alternatives to this move are printed right after it (PGN order),
        // except for the line's own first move whose siblings belong to the outer level.
        if node != first
            && let Some(parent) = game.parent(node)
        {
            for &v in game.variations(parent) {
                ui.label(RichText::new("(").weak());
                inline_line(ui, n, v, out);
                ui.label(RichText::new(")").weak());
                need_number = true;
            }
        }
        match game.main_child(node) {
            Some(next) => node = next,
            None => return,
        }
    }
}
