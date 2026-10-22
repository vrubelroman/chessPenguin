//! Chessboard widget: drawing, click/drag move input, promotion picker, arrows.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2, vec2};
use shakmaty::{Board, Chess, Color, File, Move, Piece, Position, Rank, Role, Square};

use crate::assets::{BoardTheme, PieceSet};

#[derive(Clone, Copy, Debug)]
pub struct Arrow {
    pub from: Square,
    pub to: Square,
    pub color: Color32,
    /// Relative thickness, 1.0 = default.
    pub weight: f32,
}

/// A round label in the top right corner of a square, as on Lichess.
#[derive(Clone, Copy, Debug)]
pub struct Badge {
    pub square: Square,
    pub text: &'static str,
    pub color: Color32,
}

/// Interaction state that lives across frames (per board instance).
#[derive(Default)]
pub struct BoardState {
    selected: Option<Square>,
    drag: Option<Square>,
    promotion: Option<(Square, Square)>,
    /// The position drawn last: a different one next frame is animated from it.
    shown: Option<Board>,
    anim: Option<Anim>,
    /// A move was dropped by dragging the piece from this square: that piece is
    /// already in place (a castling rook or a captured pawn still move).
    dropped: Option<Square>,
}

impl BoardState {
    /// Forgets the selection, the drag and the promotion choice.
    pub fn clear(&mut self) {
        self.selected = None;
        self.drag = None;
        self.promotion = None;
    }

    /// A piece is selected or dragged, or a promotion is being chosen.
    pub fn is_busy(&self) -> bool {
        self.selected.is_some() || self.drag.is_some() || self.promotion.is_some()
    }
}

/// Pieces moving from one position to the next, as on Lichess (chessground): each piece
/// of the new position that was not there slides from the nearest square that held the
/// same piece; a piece left without a new square fades out.
struct Anim {
    start: f64,
    duration: f64,
    slides: Vec<(Piece, Square, Square)>,
    fadings: Vec<(Piece, Square)>,
}

impl Anim {
    fn new(prev: &Board, cur: &Board, start: f64, duration: f64) -> Anim {
        let mut missing: Vec<(Piece, Square)> = Vec::new();
        let mut new: Vec<(Piece, Square)> = Vec::new();
        for sq in Square::ALL {
            match (prev.piece_at(sq), cur.piece_at(sq)) {
                (Some(a), Some(b)) if a == b => {}
                (a, b) => {
                    missing.extend(a.map(|p| (p, sq)));
                    new.extend(b.map(|p| (p, sq)));
                }
            }
        }
        let mut slides = Vec::new();
        for (piece, to) in new {
            let nearest = missing
                .iter()
                .enumerate()
                .filter(|(_, (p, _))| *p == piece)
                .min_by_key(|(_, (_, from))| {
                    let df = i32::from(from.file()) - i32::from(to.file());
                    let dr = i32::from(from.rank()) - i32::from(to.rank());
                    df * df + dr * dr
                })
                .map(|(i, _)| i);
            if let Some(i) = nearest {
                let (_, from) = missing.swap_remove(i);
                slides.push((piece, from, to));
            }
        }
        Anim {
            start,
            duration,
            slides,
            fadings: missing,
        }
    }

    /// How much of the way is left: 1 at the start, 0 at the end (eased like chessground).
    fn rest(&self, now: f64) -> Option<f32> {
        let t = 1.0 - (now - self.start) / self.duration;
        (t > 0.0).then(|| {
            let t = t as f32;
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                (t - 1.0) * (2.0 * t - 2.0) * (2.0 * t - 2.0) + 1.0
            }
        })
    }
}

pub struct BoardView<'a> {
    pub pos: &'a Chess,
    pub orientation: Color,
    pub last_move: Option<(Square, Square)>,
    pub arrows: &'a [Arrow],
    /// Circled squares (from `[%csl]` comment annotations).
    pub circles: &'a [(Square, Color32)],
    /// Move assessment shown on the destination square of the last move ("??" in red, …).
    pub badge: Option<Badge>,
    pub interactive: bool,
    pub show_coords: bool,
    pub theme: BoardTheme,
    pub pieces: PieceSet,
    /// Seconds a move takes to slide (0: no animation).
    pub animation: f32,
}

impl BoardView<'_> {
    /// Draws the board into the available square area. Returns a move the user made, if any.
    pub fn show(self, ui: &mut egui::Ui, state: &mut BoardState) -> Option<Move> {
        let avail = ui.available_size();
        let side = avail.x.min(avail.y).max(64.0).floor();
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
        let sq_size = side / 8.0;
        let geo = Geometry {
            rect,
            sq: sq_size,
            flipped: self.orientation == Color::Black,
        };
        let painter = ui.painter_at(rect);
        let legal = self.pos.legal_moves();
        let mut result = None;

        // --- input -----------------------------------------------------------------------
        if self.interactive && state.promotion.is_none() {
            let pointer = response.interact_pointer_pos();
            if response.drag_started() {
                let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
                if let Some(sq) = origin.and_then(|p| geo.square_at(p))
                    && self.own_piece(sq)
                {
                    // The piece is taken from where it stands now, not from mid-flight.
                    state.anim = None;
                    state.drag = Some(sq);
                    state.selected = Some(sq);
                }
            }
            if response.drag_stopped() {
                if let Some(from) = state.drag.take() {
                    let target = ui
                        .input(|i| i.pointer.latest_pos())
                        .and_then(|p| geo.square_at(p));
                    if let Some(to) = target.filter(|&t| t != from) {
                        result = self.try_move(state, &legal, from, to);
                        state.dropped = result.is_some().then_some(from);
                    }
                }
            } else if response.clicked()
                && let Some(sq) = pointer.and_then(|p| geo.square_at(p))
            {
                match state.selected {
                    // try_move also covers castling by clicking the own rook,
                    // and reselects when the target is just another own piece.
                    Some(from) if from != sq => {
                        result = self.try_move(state, &legal, from, sq);
                    }
                    Some(from) if from == sq => state.selected = None,
                    _ => state.selected = self.own_piece(sq).then_some(sq),
                }
            }
            if response.secondary_clicked() {
                state.clear();
            }
        }

        // --- animation ---------------------------------------------------------------------
        let now = ui.input(|i| i.time);
        let board = self.pos.board();
        if state.shown.as_ref() != Some(board) {
            state.anim = None;
            if let Some(prev) = &state.shown
                && self.animation > 0.0
            {
                let mut anim = Anim::new(prev, board, now, f64::from(self.animation));
                if let Some(from) = state.dropped {
                    anim.slides.retain(|s| s.1 != from);
                }
                state.anim = Some(anim);
            }
            state.dropped = None;
            state.shown = Some(board.clone());
        }
        let rest = state.anim.as_ref().and_then(|a| a.rest(now));
        if rest.is_none() {
            state.anim = None;
        } else {
            ui.ctx().request_repaint();
        }

        // --- squares -----------------------------------------------------------------------
        if let Some(uri) = self.theme.texture {
            egui::Image::new(uri).paint_at(ui, rect);
        }
        for sq in Square::ALL
            .into_iter()
            .filter(|_| self.theme.texture.is_none())
        {
            let light = (u32::from(sq.file()) + u32::from(sq.rank())) % 2 == 1;
            let fill = if light {
                self.theme.light
            } else {
                self.theme.dark
            };
            painter.rect_filled(geo.square_rect(sq), 0.0, fill);
        }
        let hl = |painter: &egui::Painter, sq: Square, c: Color32| {
            painter.rect_filled(geo.square_rect(sq), 0.0, c);
        };
        if let Some((from, to)) = self.last_move {
            hl(&painter, from, self.theme.last_move);
            hl(&painter, to, self.theme.last_move);
        }
        if let Some(sel) = state.selected {
            hl(&painter, sel, self.theme.selected);
        }
        if self.pos.is_check()
            && let Some(k) = self.pos.board().king_of(self.pos.turn())
        {
            let r = geo.square_rect(k);
            painter.circle_filled(
                r.center(),
                sq_size * 0.48,
                Color32::from_rgba_unmultiplied(230, 40, 40, 150),
            );
        }

        if self.show_coords {
            self.draw_coords(&painter, &geo);
        }

        // --- pieces --------------------------------------------------------------------------
        let anim = state.anim.as_ref().zip(rest);
        if let Some((a, rest)) = anim {
            for &(piece, sq) in &a.fadings {
                let r = geo.square_rect(sq).shrink(sq_size * 0.02);
                self.pieces
                    .paint_tinted(ui, piece, r, Color32::WHITE.gamma_multiply(rest));
            }
        }
        for (sq, piece) in self.pos.board().clone() {
            if state.drag == Some(sq) && response.dragged() {
                continue;
            }
            if anim.is_some_and(|(a, _)| a.slides.iter().any(|s| s.2 == sq)) {
                continue;
            }
            self.pieces
                .paint(ui, piece, geo.square_rect(sq).shrink(sq_size * 0.02));
        }
        // Moving pieces over the others.
        if let Some((a, rest)) = anim {
            for &(piece, from, to) in &a.slides {
                let r = geo.square_rect(to);
                let offset = (geo.square_rect(from).center() - r.center()) * rest;
                self.pieces
                    .paint(ui, piece, r.translate(offset).shrink(sq_size * 0.02));
            }
        }

        // legal targets of the selected piece
        if let Some(sel) = state.selected {
            for m in legal.iter().filter(|m| m.from() == Some(sel)) {
                let to = ui_target(m);
                let r = geo.square_rect(to);
                if self.pos.board().piece_at(to).is_some() && !m.is_castle() {
                    painter.circle_stroke(
                        r.center(),
                        sq_size * 0.44,
                        Stroke::new(sq_size * 0.07, self.theme.dot),
                    );
                } else {
                    painter.circle_filled(r.center(), sq_size * 0.15, self.theme.dot);
                }
            }
        }

        for (sq, c) in self.circles {
            let r = geo.square_rect(*sq);
            painter.circle_stroke(r.center(), sq_size * 0.46, Stroke::new(sq_size * 0.07, *c));
        }
        for a in self.arrows {
            draw_arrow(&painter, &geo, a);
        }
        if let Some(b) = self.badge {
            let r = geo.square_rect(b.square);
            let radius = sq_size * 0.19;
            // Over the corner of the square, kept inside the board.
            let c = egui::pos2(
                (r.right() - radius * 0.35).min(rect.right() - radius - 1.0),
                (r.top() + radius * 0.35).max(rect.top() + radius + 1.0),
            );
            painter.circle_filled(
                c + vec2(0.0, sq_size * 0.015),
                radius,
                Color32::from_black_alpha(90),
            );
            painter.circle(
                c,
                radius,
                b.color,
                Stroke::new(sq_size * 0.02, Color32::WHITE),
            );
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                b.text,
                egui::FontId::proportional(radius * 1.05),
                Color32::WHITE,
            );
        }

        // dragged piece follows the cursor
        if let (Some(from), true) = (state.drag, response.dragged())
            && let (Some(piece), Some(p)) = (
                self.pos.board().piece_at(from),
                ui.input(|i| i.pointer.latest_pos()),
            )
        {
            let r = Rect::from_center_size(p, Vec2::splat(sq_size * 1.1));
            self.pieces.paint(ui, piece, r);
        }

        // --- promotion picker ---------------------------------------------------------------
        if let Some((from, to)) = state.promotion {
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(140));
            let color = self.pos.turn();
            let roles = [Role::Queen, Role::Knight, Role::Rook, Role::Bishop];
            let top_down = geo.square_rect(to).center().y < rect.center().y;
            for (i, role) in roles.iter().enumerate() {
                let off = if top_down { i as f32 } else { -(i as f32) };
                let r = geo.square_rect(to).translate(vec2(0.0, off * sq_size));
                let hover = ui.rect_contains_pointer(r);
                painter.circle_filled(
                    r.center(),
                    sq_size * 0.5,
                    if hover {
                        Color32::from_gray(240)
                    } else {
                        Color32::from_gray(190)
                    },
                );
                self.pieces
                    .paint(ui, Piece { color, role: *role }, r.shrink(sq_size * 0.08));
                if hover && ui.input(|i| i.pointer.primary_clicked()) {
                    result = legal
                        .iter()
                        .find(|m| {
                            m.from() == Some(from) && m.to() == to && m.promotion() == Some(*role)
                        })
                        .cloned();
                    state.clear();
                }
            }
            if result.is_none()
                && ui.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.secondary_clicked())
            {
                state.clear();
            }
        }

        if result.is_some() {
            state.clear();
        }
        result
    }

    fn own_piece(&self, sq: Square) -> bool {
        self.pos
            .board()
            .piece_at(sq)
            .is_some_and(|p| p.color == self.pos.turn())
    }

    fn try_move(
        &self,
        state: &mut BoardState,
        legal: &shakmaty::MoveList,
        from: Square,
        to: Square,
    ) -> Option<Move> {
        let candidates: Vec<&Move> = legal
            .iter()
            .filter(|m| m.from() == Some(from) && (ui_target(m) == to || m.to() == to))
            .collect();
        match candidates.as_slice() {
            [] => {
                state.selected = self.own_piece(to).then_some(to);
                None
            }
            [m] => Some(*(*m)),
            _ if candidates.iter().all(|m| m.is_promotion()) => {
                state.promotion = Some((from, to));
                state.selected = None;
                None
            }
            _ => candidates.first().map(|m| *(*m)),
        }
    }

    fn draw_coords(&self, painter: &egui::Painter, geo: &Geometry) {
        let font = FontId::proportional((geo.sq * 0.17).max(9.0));
        for i in 0..8u32 {
            let file = File::new(i);
            let rank = Rank::new(i);
            let bottom_rank = if geo.flipped {
                Rank::Eighth
            } else {
                Rank::First
            };
            let left_file = if geo.flipped { File::H } else { File::A };
            let fsq = Square::from_coords(file, bottom_rank);
            let rsq = Square::from_coords(left_file, rank);
            let color_for = |sq: Square| {
                let light = (u32::from(sq.file()) + u32::from(sq.rank())) % 2 == 1;
                if light {
                    self.theme.dark
                } else {
                    self.theme.light
                }
            };
            let fr = geo.square_rect(fsq);
            painter.text(
                fr.right_bottom() + vec2(-geo.sq * 0.06, -geo.sq * 0.02),
                Align2::RIGHT_BOTTOM,
                file.char(),
                font.clone(),
                color_for(fsq),
            );
            let rr = geo.square_rect(rsq);
            painter.text(
                rr.left_top() + vec2(geo.sq * 0.05, geo.sq * 0.03),
                Align2::LEFT_TOP,
                rank.char(),
                font.clone(),
                color_for(rsq),
            );
        }
    }
}

/// Square the user clicks to make a move: for castling, the king's destination.
pub fn ui_target(m: &Move) -> Square {
    match (m, m.castling_side()) {
        (Move::Castle { king, .. }, Some(side)) => side.king_to(if king.rank() == Rank::First {
            Color::White
        } else {
            Color::Black
        }),
        _ => m.to(),
    }
}

struct Geometry {
    rect: Rect,
    sq: f32,
    flipped: bool,
}

impl Geometry {
    fn square_rect(&self, sq: Square) -> Rect {
        let (f, r) = (u32::from(sq.file()) as f32, u32::from(sq.rank()) as f32);
        let (x, y) = if self.flipped {
            (7.0 - f, r)
        } else {
            (f, 7.0 - r)
        };
        Rect::from_min_size(
            self.rect.min + vec2(x * self.sq, y * self.sq),
            Vec2::splat(self.sq),
        )
    }

    fn center(&self, sq: Square) -> Pos2 {
        self.square_rect(sq).center()
    }

    fn square_at(&self, p: Pos2) -> Option<Square> {
        if !self.rect.contains(p) {
            return None;
        }
        let x = ((p.x - self.rect.min.x) / self.sq).floor().clamp(0.0, 7.0) as u32;
        let y = ((p.y - self.rect.min.y) / self.sq).floor().clamp(0.0, 7.0) as u32;
        let (f, r) = if self.flipped { (7 - x, y) } else { (x, 7 - y) };
        Some(Square::from_coords(File::new(f), Rank::new(r)))
    }
}

fn draw_arrow(painter: &egui::Painter, geo: &Geometry, a: &Arrow) {
    let from = geo.center(a.from);
    let to = geo.center(a.to);
    let dir = (to - from).normalized();
    if !dir.x.is_finite() {
        return;
    }
    let width = geo.sq * 0.16 * a.weight;
    let head_len = geo.sq * 0.42 * a.weight.sqrt();
    let head_w = geo.sq * 0.38 * a.weight.sqrt();
    let tip = to - dir * geo.sq * 0.12;
    let base = tip - dir * head_len;
    let start = from + dir * geo.sq * 0.2;
    let normal = vec2(-dir.y, dir.x);
    painter.line_segment([start, base], Stroke::new(width, a.color));
    painter.add(Shape::convex_polygon(
        vec![tip, base + normal * head_w, base - normal * head_w],
        a.color,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::san::San;
    use shakmaty::{CastlingMode, Chess};

    fn after(fen: &str, moves: &str) -> (Board, Board) {
        let pos: Chess = fen
            .parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap();
        let mut next = pos.clone();
        for san in moves.split_whitespace() {
            let m = san.parse::<San>().unwrap().to_move(&next).unwrap();
            next.play_unchecked(m);
        }
        (pos.board().clone(), next.board().clone())
    }

    fn plan(fen: &str, moves: &str) -> (Vec<String>, Vec<String>) {
        let (a, b) = after(fen, moves);
        let anim = Anim::new(&a, &b, 0.0, 0.25);
        let mut slides: Vec<String> = anim
            .slides
            .iter()
            .map(|(p, f, t)| format!("{}{f}{t}", p.char()))
            .collect();
        let mut fadings: Vec<String> = anim
            .fadings
            .iter()
            .map(|(p, s)| format!("{}{s}", p.char()))
            .collect();
        slides.sort();
        fadings.sort();
        (slides, fadings)
    }

    const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

    #[test]
    fn a_move_slides_one_piece() {
        assert_eq!(plan(START, "e4"), (vec!["Pe2e4".into()], vec![]));
    }

    #[test]
    fn a_captured_piece_fades() {
        let fen = "rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
        assert_eq!(
            plan(fen, "exd5"),
            (vec!["Pe4d5".into()], vec!["pd5".into()])
        );
    }

    #[test]
    fn castling_slides_king_and_rook() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        assert_eq!(
            plan(fen, "O-O"),
            (vec!["Ke1g1".into(), "Rh1f1".into()], vec![])
        );
    }

    #[test]
    fn a_promoted_pawn_fades_and_the_queen_appears() {
        let fen = "8/4P3/8/8/8/8/k7/4K3 w - - 0 1";
        assert_eq!(plan(fen, "e8=Q"), (vec![], vec!["Pe7".into()]));
    }

    #[test]
    fn en_passant_fades_the_pawn_beside() {
        let fen = "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2";
        assert_eq!(
            plan(fen, "exd6"),
            (vec!["Pe5d6".into()], vec!["pd5".into()])
        );
    }

    #[test]
    fn a_jump_over_several_moves_slides_each_piece_once() {
        let (slides, fadings) = plan(START, "e4 e5 Nf3 Nc6");
        assert_eq!(
            slides,
            vec!["Ng1f3", "Pe2e4", "nb8c6", "pe7e5"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert!(fadings.is_empty());
    }
}
