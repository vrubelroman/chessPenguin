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
    /// points more than the move took; not a piece selling itself (an attacked piece
    /// taking something on its way, a desperado), nor one whose capture loses as much
    /// back at once (the move uncovered an attack on a bigger piece: taking is no answer
    /// to it). An attacked piece moved quietly to another square where it is lost is
    /// offered: it could have stepped away (18…Ra3!! of Pillsbury — Lasker).
    pub sacrifice: bool,
    /// A pawn moved (not taking) to a square where the opponent wins it: offered, but
    /// brilliant only for a quicker mate (29…b5+!! of Pillsbury — Lasker).
    pub pawn_offered: bool,
    pub capture: bool,
    /// A capture that wins two points or more.
    pub winning_capture: bool,
    /// The capture only wins back what the opponent has just taken, elsewhere (18.fxe6!
    /// of Pillsbury — Lasker after 17…Rxc3): no gain, so it can be great.
    pub takes_back: bool,
    /// The first rules' sacrifice ([`Rules::Strict`]): any piece the opponent can win
    /// after the move, also one that hung before it.
    pub sacrifice_any: bool,
    /// A piece (not the king) under attack moves away without loss.
    pub escape: bool,
    /// The move goes into a position of the opening table (chess.com's «book»).
    pub book: bool,
}

/// Which rules decide !! and !. Both stay, to compare them and to go back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rules {
    /// The first rules (docs/research/brilliant-moves.md §3–4): any piece left to be won
    /// is a sacrifice; the move must change the outcome to be great; a piece saving
    /// itself is never great; one mark a combination.
    Strict,
    /// Calibrated on chess.com's marks (2026-10-28): only what the move gives up is a
    /// sacrifice; the only good move is great; every move of a combination counts.
    #[default]
    ChessCom,
}

impl MoveFeatures {
    /// `prev`: the opponent's move just before (for a recapture); `own`: the mover's move
    /// before that.
    pub fn of(before: &Chess, m: &Move, prev: Option<&Move>, own: Option<&Move>) -> MoveFeatures {
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
        // The heaviest loss among `pieces` and the value of the piece lost there.
        let worst = |pieces: &[&(i32, i32, Square)]| {
            let threat = pieces.iter().map(|t| t.0).max().unwrap_or(0);
            let offered = pieces
                .iter()
                .filter(|t| t.0 == threat)
                .map(|t| t.1)
                .max()
                .unwrap_or(0);
            (threat, offered)
        };
        let (threat, offered) = worst(&given);
        // Taking the offered piece loses at least as much straight away: 18.Bxa7 opening
        // the d-file on the queen (18…Rxa7 19.Rxd8) offers nothing — chess.com gives no !!.
        let taking_loses = given
            .iter()
            .filter(|t| t.0 == threat && t.1 == offered)
            .any(|t| taking_loses(before.board(), &after, t.2, threat));
        let (threat_any, offered_any) = worst(&threats.iter().collect::<Vec<_>>());
        // The moved piece was hanging where it stood: losing it is no sacrifice.
        let from_threat = m
            .from()
            .map_or(0, |f| pseudo_threat(before.board(), f, mover.other()));
        let lost_anyway = from_threat >= 2;
        let desperado = lost_anyway && m.is_capture();
        let on_target = threats.iter().find(|t| t.2 == m.to()).map_or(0, |t| t.0);
        let gain = taken - on_target;
        MoveFeatures {
            legal: before.legal_moves().len(),
            check: before.is_check(),
            promotion: m.is_promotion(),
            mates: after.is_checkmate(),
            recapture: m.is_capture() && prev.is_some_and(|p| p.is_capture() && p.to() == m.to()),
            sacrifice: offered >= 3 && taken - threat <= -2 && !desperado && !taking_loses,
            pawn_offered: m.role() == Role::Pawn
                && !m.is_capture()
                && pseudo_threat(after.board(), m.to(), mover.other()) > 0,
            capture: m.is_capture(),
            winning_capture: m.is_capture() && gain >= 2,
            takes_back: m.is_capture()
                && prev.is_some_and(|p| {
                    // Not the piece the mover had just put there: taking it accepted an
                    // offer, and winning something now goes on with the combination
                    // (24…Bxe4+ of Rotlewi — Rubinstein after 23…Rd2 24.Qxd2).
                    p.capture().is_some_and(|r| gain - value(r) < 2)
                        && own.is_none_or(|o| o.to() != p.to())
                }),
            sacrifice_any: offered_any >= 3 && taken - threat_any <= -2 && !lost_anyway,
            escape: lost_anyway && from_threat < 900 && gain >= 0,
            book: chess_openings::in_book(chess_openings::key(&after)),
        }
    }

    /// Could be brilliant: a sacrifice by choice.
    pub fn brilliant_shape(&self, rules: Rules) -> bool {
        let sacrifice = match rules {
            Rules::Strict => self.sacrifice_any,
            Rules::ChessCom => self.sacrifice,
        };
        sacrifice
            && self.legal > 1
            && !self.check
            && !self.promotion
            && !(rules == Rules::ChessCom && self.book)
    }

    /// Could be great: not a forced, obvious or finishing move. With [`Rules::ChessCom`]
    /// a piece saving itself on the only good square can be (13.Qa4 of the Evergreen
    /// game), a book move cannot (6…Bb4+ of Steinitz — von Bardeleben).
    pub fn great_shape(&self, rules: Rules) -> bool {
        self.legal > 1
            && !(rules == Rules::ChessCom && self.book)
            && !self.mates
            && !self.recapture
            && !(self.winning_capture && !(rules == Rules::ChessCom && self.takes_back))
            && !(rules == Rules::Strict && self.escape)
    }

    /// Worth asking the engine for the best other move.
    /// (Under either rules: one analysis serves both.)
    pub fn candidate(&self) -> bool {
        [Rules::Strict, Rules::ChessCom]
            .into_iter()
            .any(|r| self.brilliant_shape(r) || self.pawn_shape(r) || self.great_shape(r))
    }

    /// Could be brilliant for a quicker mate only: a pawn offered ([`Rules::ChessCom`]).
    pub fn pawn_shape(&self, rules: Rules) -> bool {
        rules == Rules::ChessCom
            && self.pawn_offered
            && self.legal > 1
            && !self.check
            && !self.promotion
            && !self.book
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

/// In `pos` (the opponent to move), taking on `sq` with the cheapest piece lets the
/// mover win at least `threat` elsewhere at once, with a threat the move itself made:
/// there after the move (before the capture), not before it (`before`: the board before
/// the move). One the capture opens is the point of a real sacrifice: 22.Rxe7+!! of
/// Steinitz — von Bardeleben, 22…Qxe7 23.Rxc8+ (the queen left d7); one standing
/// already is too: 37.Rd7!! of Kasparov — Topalov (the queen on c4 under the bishop since
/// 36.Bf1). chess.com marks both.
fn taking_loses(before: &Board, pos: &Chess, sq: Square, threat: i32) -> bool {
    let Some(take) = pos
        .legal_moves()
        .into_iter()
        .filter(|m| m.to() == sq && m.is_capture())
        .min_by_key(|m| value(m.role()))
    else {
        return false;
    };
    let mut after = pos.clone();
    after.play_unchecked(take);
    let mover = after.turn();
    threats(&after).iter().any(|t| {
        t.2 != sq
            && t.0 >= threat
            && pseudo_threat(pos.board(), t.2, mover) >= threat
            && pseudo_threat(before, t.2, mover) < threat
    })
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
        let mut own = None;
        let sans: Vec<&str> = moves
            .split_whitespace()
            .filter(|t| !t.ends_with('.'))
            .collect();
        for (i, san) in sans.iter().enumerate() {
            let m = san.parse::<San>().unwrap().to_move(&pos).unwrap();
            if i + 1 == sans.len() {
                return MoveFeatures::of(&pos, &m, prev.as_ref(), own.as_ref());
            }
            pos.play_unchecked(m);
            own = prev;
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
        assert!(f.sacrifice && f.brilliant_shape(Rules::ChessCom), "{f:?}");
        assert!(!f.mates && f.candidate());
        // 15.Bxd7+ Nxd7: taken back, not a move of note.
        let f = last(Chess::default(), &OPERA[..OPERA.find(" 16.").unwrap()]);
        assert!(f.recapture && !f.great_shape(Rules::ChessCom), "{f:?}");
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
            f.winning_capture && !f.great_shape(Rules::ChessCom) && !f.sacrifice,
            "{f:?}"
        );
    }

    #[test]
    fn a_piece_saved_or_lost_anyway_is_no_sacrifice() {
        // The bishop on c4 attacked by the pawn on d5: Bb3 saves it, which can be the only
        // good move (great), never a sacrifice.
        let f = features("4k3/8/8/3p4/2B5/8/8/4K3 w - - 0 1", "Bb3");
        assert!(f.great_shape(Rules::ChessCom) && !f.sacrifice, "{f:?}");
        // Taking the pawn instead, into the knight: it was lost anyway (desperado).
        let f = features("4k3/8/5n2/3p4/2B5/8/8/4K3 w - - 0 1", "Bxd5");
        assert!(!f.sacrifice, "{f:?}");
    }

    #[test]
    fn only_what_the_move_gives_up_is_offered() {
        // The knight on e5 hangs to the pawn on d6 before and after Ra2: not given by it.
        // (Counting a piece left hanging as given would mark 17…Be6!! of Byrne — Fischer,
        // but also 11.Rg1, 19.e5 of the Immortal game and more that chess.com does not.)
        let f = features("4k3/8/3p4/4N3/8/8/8/R3K3 w - - 0 1", "Ra2");
        assert!(!f.sacrifice, "{f:?}");
        // The rook on e1 guards it; Ra1 takes the guard away: the knight is given up.
        let f = features("4k3/8/3p4/4N3/8/8/8/4RK2 w - - 0 1", "Ra1");
        assert!(f.sacrifice, "{f:?}");
    }

    #[test]
    fn a_piece_whose_capture_loses_more_is_not_offered() {
        // 18.Bxa7 (a game against Rodent, 2026-10-27): the bishop under the rook on a8, but
        // the move opens the d-file on the queen: 18…Rxa7 19.Rxd8. chess.com: no !!.
        let f = last(
            Chess::default(),
            "1. e4 c5 2. Nf3 Nc6 3. d4 cxd4 4. Nxd4 g6 5. Nc3 Bg7 6. Be3 Nf6 7. Bc4 Qa5 \
             8. O-O O-O 9. Bb3 d6 10. f3 Bd7 11. Qe1 Nxd4 12. Bxd4 Bc6 13. Nd5 Qd8 \
             14. Nxf6+ exf6 15. Qc3 d5 16. Rad1 dxe4 17. fxe4 Bxe4 18. Bxa7",
        );
        assert!(!f.sacrifice && !f.brilliant_shape(Rules::ChessCom), "{f:?}");
        assert!(f.sacrifice_any, "the first rules stay as they were: {f:?}");
    }

    #[test]
    fn a_threat_the_capture_opens_keeps_the_sacrifice() {
        // 22.Rxe7+!! (Steinitz — von Bardeleben): 22…Qxe7 23.Rxc8+ wins back, but only
        // because the queen left d7. chess.com: !!.
        let f = last(
            Chess::default(),
            "1. e4 e5 2. Nf3 Nc6 3. Bc4 Bc5 4. c3 Nf6 5. d4 exd4 6. cxd4 Bb4+ 7. Nc3 d5 \
             8. exd5 Nxd5 9. O-O Be6 10. Bg5 Be7 11. Bxd5 Bxd5 12. Nxd5 Qxd5 13. Bxe7 Nxe7 \
             14. Re1 f6 15. Qe2 Qd7 16. Rac1 c6 17. d5 cxd5 18. Nd4 Kf7 19. Ne6 Rhc8 \
             20. Qg4 g6 21. Ng5+ Ke8 22. Rxe7+",
        );
        assert!(f.sacrifice && f.brilliant_shape(Rules::ChessCom), "{f:?}");
    }

    const PILLSBURY: &str = "1. d4 d5 2. c4 e6 3. Nc3 Nf6 4. Nf3 c5 5. Bg5 cxd4 6. Qxd4 Nc6 \
        7. Qh4 Be7 8. O-O-O Qa5 9. e3 Bd7 10. Kb1 h6 11. cxd5 exd5 12. Nd4 O-O 13. Bxf6 Bxf6 \
        14. Qh5 Nxd4 15. exd4 Be6 16. f4 Rac8 17. f5 Rxc3 18. fxe6 Ra3";

    #[test]
    fn an_attacked_piece_moved_into_another_attack_is_offered() {
        // 18…Ra3!! (Pillsbury — Lasker): the rook on c3 under the pawn could step away.
        let f = last(Chess::default(), PILLSBURY);
        assert!(f.sacrifice && f.brilliant_shape(Rules::ChessCom), "{f:?}");
        // 18.fxe6! just before: takes back the knight taken on c3, no gain.
        let f = last(
            Chess::default(),
            &PILLSBURY[..PILLSBURY.rfind(" Ra3").unwrap()],
        );
        assert!(
            f.winning_capture && f.takes_back && f.great_shape(Rules::ChessCom),
            "{f:?}"
        );
        assert!(!f.great_shape(Rules::Strict), "the first rules stay: {f:?}");
    }

    #[test]
    fn taking_back_an_offered_piece_is_no_trade() {
        // 24…Bxe4+ (Rotlewi — Rubinstein) after 23…Rd2 24.Qxd2: the combination goes on.
        let f = last(
            Chess::default(),
            "1. d4 d5 2. Nf3 e6 3. e3 c5 4. c4 Nc6 5. Nc3 Nf6 6. dxc5 Bxc5 7. a3 a6 8. b4 Bd6 \
             9. Bb2 O-O 10. Qd2 Qe7 11. Bd3 dxc4 12. Bxc4 b5 13. Bd3 Rd8 14. Qe2 Bb7 \
             15. O-O Ne5 16. Nxe5 Bxe5 17. f4 Bc7 18. e4 Rac8 19. e5 Bb6+ 20. Kh1 Ng4 \
             21. Be4 Qh4 22. g3 Rxc3 23. gxh4 Rd2 24. Qxd2 Bxe4+",
        );
        assert!(
            f.winning_capture && !f.takes_back && !f.great_shape(Rules::ChessCom),
            "{f:?}"
        );
    }

    #[test]
    fn a_pawn_offered_is_only_for_a_quicker_mate() {
        // 29…b5+!! (Pillsbury — Lasker): the pawn under the king.
        let line = format!(
            "{PILLSBURY} 19. exf7+ Rxf7 20. bxa3 Qb6+ 21. Bb5 Qxb5+ 22. Ka1 Rc7 23. Rd2 Rc4 \
             24. Rhd1 Rc3 25. Qf5 Qc4 26. Kb2 Rxa3 27. Qe6+ Kh7 28. Kxa3 Qc3+ 29. Ka4 b5+"
        );
        let f = last(Chess::default(), &line);
        assert!(f.pawn_offered && f.pawn_shape(Rules::ChessCom), "{f:?}");
        assert!(!f.sacrifice && !f.brilliant_shape(Rules::ChessCom), "{f:?}");
        assert!(!f.pawn_shape(Rules::Strict));
    }

    #[test]
    fn a_piece_no_one_can_legally_take_is_not_offered() {
        // The rook on d7 is pinned by the bishop: the knight on d5 is no offer to it.
        let f = features("4k3/3r4/8/8/B7/2N5/8/4K3 w - - 0 1", "Nd5");
        assert!(!f.sacrifice, "{f:?}");
        let f = features("4k3/3r4/8/8/8/2N5/8/4K3 w - - 0 1", "Nd5");
        assert!(f.sacrifice && f.brilliant_shape(Rules::ChessCom), "{f:?}");
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
