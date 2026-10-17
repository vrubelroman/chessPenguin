//! Move list: main line as a two-column table (Lichess style), comments and variations
//! as full-width blocks between rows; nested variations inline in parentheses.

use crate::i18n::tr;
use chess_core::shakmaty::Color;
use chess_core::{CommentPart, Game, Nag, NodeId, parse_comment};
use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, vec2};

pub struct Notation<'a> {
    pub game: &'a Game,
    pub cursor: NodeId,
    pub scroll_to_cursor: bool,
}

/// Result of a frame of the notation panel.
#[derive(Default)]
pub struct NotationResponse {
    pub clicked: Option<NodeId>,
    pub context: Option<(NodeId, NotationCommand)>,
}

#[derive(Clone, Copy, Debug)]
pub enum NotationCommand {
    Promote,
    Delete,
}

const ROW_H: f32 = 26.0;
const NUM_W: f32 = 38.0;

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
            let interrupted = !after.is_empty() || !vars.is_empty() || !n.unparsed().is_empty();
            if interrupted || color == Color::Black {
                row.flush(ui, &self, &mut out);
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
            color = Some(match nag.0 {
                1 => Color32::from_rgb(0x62, 0x9c, 0x2e),
                2 => Color32::from_rgb(0xe6, 0x8f, 0x00),
                3 => Color32::from_rgb(0x22, 0xac, 0x9a),
                4 => Color32::from_rgb(0xdf, 0x53, 0x53),
                5 => Color32::from_rgb(0xea, 0x45, 0xd8),
                6 => Color32::from_rgb(0x56, 0xb4, 0xe9),
                _ => continue,
            });
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
                // Source line breaks are formatting, not content: reflow to the panel width.
                for word in t.split_whitespace() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(word);
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
