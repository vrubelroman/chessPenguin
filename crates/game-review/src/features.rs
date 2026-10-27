//! What a move does on the board, without an engine: a piece offered (a sacrifice), a
//! piece taken back or saved, a mate (docs/research/brilliant-moves.md §3.1, §4). The
//! engine's evaluations then say whether it was brilliant (!!) or great (!).

use chess_core::shakmaty::{Bitboard, Board, Chess, Color, Move, Position, Role, Square};

/// The move as the rules of !! and ! see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveFeatures {
    /// Legal moves in the position: one is a forced move, never marked.
    pub legal: usize,
    /// The mover was in check.
    pub check: bool,
    pub promotion: bool,
    /// The move mates.
    pub mates: bool,
    /// Takes back on the square where the opponent has just taken.
    pub recapture: bool,
    /// After the move the opponent can win a piece (a knight or more) for at least two
    /// points more than the move took; a piece that was lost anyway does not count.
    pub sacrifice: bool,
    /// A capture that wins two points or more.
    pub winning_capture: bool,
}

impl MoveFeatures {
    /// `prev`: the opponent's move just before (for a recapture).
    pub fn of(before: &Chess, m: &Move, prev: Option<&Move>) -> MoveFeatures {
        let mover = before.turn();
        let mut after = before.clone();
        after.play_unchecked(*m);
        let taken = m.capture().map_or(0, value);
        // The opponent's best gain on each of the mover's pieces after the move.
        let threats = threats(&after);
        // Offered by this move: the piece moved, or one the move left open to a bigger
        // loss than before; a piece that hung already was not given by it (chess.com's
        // reading: 18…Rg8 of the Evergreen game, the knight on e7 under the pawn on f6
        // before and after, is no sacrifice).
        let given: Vec<&(i32, i32, Square)> = threats
            .iter()
            .filter(|t| t.2 == m.to() || pseudo_threat(before.board(), t.2, mover.other()) < t.0)
            .collect();
        let threat = given.iter().map(|t| t.0).max().unwrap_or(0);
        let offered = given
            .iter()
            .filter(|t| t.0 == threat)
            .map(|t| t.1)
            .max()
            .unwrap_or(0);
        // The moved piece was hanging where it stood: losing it is no sacrifice.
        let from_threat = m
            .from()
            .map_or(0, |f| pseudo_threat(before.board(), f, mover.other()));
        let lost_anyway = from_threat >= 2;
        let on_target = threats.iter().find(|t| t.2 == m.to()).map_or(0, |t| t.0);
        let gain = taken - on_target;
        MoveFeatures {
            legal: before.legal_moves().len(),
            check: before.is_check(),
            promotion: m.is_promotion(),
            mates: after.is_checkmate(),
            recapture: m.is_capture() && prev.is_some_and(|p| p.is_capture() && p.to() == m.to()),
            sacrifice: offered >= 3 && taken - threat <= -2 && !lost_anyway,
            winning_capture: m.is_capture() && gain >= 2,
        }
    }

    /// Could be brilliant: a sacrifice by choice.
    pub fn brilliant_shape(&self) -> bool {
        self.sacrifice && self.legal > 1 && !self.check && !self.promotion
    }

    /// Could be great: not a forced, obvious or finishing move. A piece saving itself on
    /// the only good square can be (chess.com: 13.Qa4 of the Evergreen game).
    pub fn great_shape(&self) -> bool {
        self.legal > 1 && !self.mates && !self.recapture && !self.winning_capture
    }

    /// Worth asking the engine for the best other move.
    pub fn candidate(&self) -> bool {
        self.brilliant_shape() || self.great_shape()
    }
}

/// Exchange values; the king's makes taking into a defended square never pay.
fn value(r: Role) -> i32 {
    match r {
        Role::Pawn => 1,
        Role::Knight | Role::Bishop => 3,
        Role::Rook => 5,
        Role::Queen => 9,
        Role::King => 1000,
    }
}

/// Static exchange on `sq`: what `side` gains taking first with the piece on `first`,
/// both sides then taking with their cheapest piece and free to stop (x-rays count).
fn see(board: &Board, sq: Square, first: Square, side: Color) -> i32 {
    let (Some(target), Some(attacker)) = (board.role_at(sq), board.role_at(first)) else {
        return 0;
    };
    let mut gains = vec![value(target)];
    let mut occupied = board.occupied() ^ Bitboard::from_square(first);
    let mut on_square = value(attacker);
    let mut turn = side.other();
    loop {
        let cheapest = (board.attacks_to(sq, turn, occupied) & occupied)
            .into_iter()
            .filter_map(|a| Some((value(board.role_at(a)?), a)))
            .min();
        let Some((v, a)) = cheapest else { break };
        let last = gains[gains.len() - 1];
        gains.push(on_square - last);
        on_square = v;
        occupied ^= Bitboard::from_square(a);
        turn = turn.other();
    }
    for d in (1..gains.len()).rev() {
        gains[d - 1] = -(-gains[d - 1]).max(gains[d]);
    }
    gains[0]
}

/// The pieces (a knight or more) the side to move can win, with legal first captures:
/// (gain, value of the piece, its square).
fn threats(pos: &Chess) -> Vec<(i32, i32, Square)> {
    let us = pos.turn();
    let board = pos.board();
    let legal = pos.legal_moves();
    let mut out = Vec::new();
    for sq in board.by_color(us.other()) {
        let Some(r) = board.role_at(sq) else { continue };
        if matches!(r, Role::King | Role::Pawn) {
            continue;
        }
        let first = legal
            .iter()
            .filter(|m| m.to() == sq && m.is_capture() && !m.is_en_passant())
            .filter_map(|m| Some((value(m.role()), m.from()?)))
            .min();
        if let Some((_, from)) = first {
            let g = see(board, sq, from, us);
            if g > 0 {
                out.push((g, value(r), sq));
            }
        }
    }
    out
}

/// What `by` would win taking on `sq`, whoever is to move (a piece already hanging).
fn pseudo_threat(board: &Board, sq: Square, by: Color) -> i32 {
    let first = board
        .attacks_to(sq, by, board.occupied())
        .into_iter()
        .filter_map(|a| Some((value(board.role_at(a)?), a)))
        .min();
    first.map_or(0, |(_, a)| see(board, sq, a, by).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::shakmaty::fen::Fen;
    use chess_core::shakmaty::san::San;
    use chess_core::shakmaty::{CastlingMode, Chess};

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap()
    }

    /// The features of the last of `moves` (SAN) played from `start`.
    fn last(start: Chess, moves: &str) -> MoveFeatures {
        let mut pos = start;
        let mut prev = None;
        let sans: Vec<&str> = moves
            .split_whitespace()
            .filter(|t| !t.ends_with('.'))
            .collect();
        for (i, san) in sans.iter().enumerate() {
            let m = san.parse::<San>().unwrap().to_move(&pos).unwrap();
            if i + 1 == sans.len() {
                return MoveFeatures::of(&pos, &m, prev.as_ref());
            }
            pos.play_unchecked(m);
            prev = Some(m);
        }
        unreachable!()
    }

    fn features(fen: &str, san: &str) -> MoveFeatures {
        last(position(fen), san)
    }

    const OPERA: &str = "1. e4 e5 2. Nf3 d6 3. d4 Bg4 4. dxe5 Bxf3 5. Qxf3 dxe5 6. Bc4 Nf6 \
        7. Qb3 Qe7 8. Nc3 c6 9. Bg5 b5 10. Nxb5 cxb5 11. Bxb5+ Nbd7 12. O-O-O Rd8 \
        13. Rxd7 Rxd7 14. Rd1 Qe6 15. Bxd7+ Nxd7 16. Qb8+";

    #[test]
    fn a_queen_offered_for_mate_is_a_sacrifice() {
        let f = last(Chess::default(), OPERA);
        assert!(f.sacrifice && f.brilliant_shape(), "{f:?}");
        assert!(!f.mates && f.candidate());
        // 15.Bxd7+ Nxd7: taken back, not a move of note.
        let f = last(Chess::default(), &OPERA[..OPERA.find(" 16.").unwrap()]);
        assert!(f.recapture && !f.great_shape(), "{f:?}");
    }

    #[test]
    fn trades_and_good_captures_are_not_sacrifices() {
        let f = last(
            Chess::default(),
            "1. e4 e5 2. Nf3 Nc6 3. d4 exd4 4. Nxd4 Nxd4",
        );
        assert!(!f.sacrifice && f.recapture, "{f:?}");
        // A free knight taken: a winning capture, never !.
        let f = features("4k3/8/8/3n4/8/8/8/3QK3 w - - 0 1", "Qxd5");
        assert!(
            f.winning_capture && !f.great_shape() && !f.sacrifice,
            "{f:?}"
        );
    }

    #[test]
    fn a_piece_saved_or_lost_anyway_is_no_sacrifice() {
        // The bishop on c4 attacked by the pawn on d5: Bb3 saves it, which can be the only
        // good move (great), never a sacrifice.
        let f = features("4k3/8/8/3p4/2B5/8/8/4K3 w - - 0 1", "Bb3");
        assert!(f.great_shape() && !f.sacrifice, "{f:?}");
        // Taking the pawn instead, into the knight: it was lost anyway (desperado).
        let f = features("4k3/8/5n2/3p4/2B5/8/8/4K3 w - - 0 1", "Bxd5");
        assert!(!f.sacrifice, "{f:?}");
    }

    #[test]
    fn only_what_the_move_gives_up_is_offered() {
        // The knight on e5 hangs to the pawn on d6 before and after Ra2: not given by it.
        let f = features("4k3/8/3p4/4N3/8/8/8/R3K3 w - - 0 1", "Ra2");
        assert!(!f.sacrifice, "{f:?}");
        // The rook on e1 guards it; Ra1 takes the guard away: the knight is given up.
        let f = features("4k3/8/3p4/4N3/8/8/8/4RK2 w - - 0 1", "Ra1");
        assert!(f.sacrifice, "{f:?}");
    }

    #[test]
    fn a_piece_no_one_can_legally_take_is_not_offered() {
        // The rook on d7 is pinned by the bishop: the knight on d5 is no offer to it.
        let f = features("4k3/3r4/8/8/B7/2N5/8/4K3 w - - 0 1", "Nd5");
        assert!(!f.sacrifice, "{f:?}");
        let f = features("4k3/3r4/8/8/8/2N5/8/4K3 w - - 0 1", "Nd5");
        assert!(f.sacrifice && f.brilliant_shape(), "{f:?}");
    }

    #[test]
    fn exchanges_follow_the_cheapest_piece_and_x_rays() {
        // Rook takes a pawn defended by a pawn: loses four.
        let b = position("4k3/8/2p5/3p4/8/8/8/3RK3 w - - 0 1");
        assert_eq!(see(b.board(), Square::D5, Square::D1, Color::White), -4);
        // Two rooks behind each other against one defender: the pawn is won.
        let b = position("3rk3/8/8/3p4/8/8/3R4/3RK3 w - - 0 1");
        assert_eq!(see(b.board(), Square::D5, Square::D2, Color::White), 1);
    }
}
