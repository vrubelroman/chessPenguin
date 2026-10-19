//! Position search: the games of a database that reach a given position (by any move
//! order), and what was played next. Shared by both kinds of database: a large
//! ChessBase database walks the main lines of its games (`chessbase::search`), a PGN
//! database keeps the positions of all its games, variations included, in memory
//! (`pgn_positions`). Decision 13 in `docs/decisions.md`.

use std::collections::HashMap;

use shakmaty::uci::UciMove;
use shakmaty::zobrist::Zobrist64;
use shakmaty::{Chess, EnPassantMode, Move, Position};

/// What to look for.
#[derive(Clone, Debug)]
pub struct PositionQuery {
    /// The position to find.
    pub position: Chess,
    /// Moves the user played to reach it from the usual start position, when known;
    /// a game that reaches the position by exactly these moves is "in this move order".
    pub line: Option<Vec<Move>>,
}

impl PositionQuery {
    pub fn new(position: Chess, line: Option<Vec<Move>>) -> PositionQuery {
        PositionQuery { position, line }
    }

    /// Identity of the target position.
    pub fn key(&self) -> u64 {
        key(&self.position)
    }
}

/// Identity of a position for searching: board, side to move, castling rights and a
/// legal en passant square (the move counters do not matter).
pub fn key(pos: &Chess) -> u64 {
    pos.zobrist_hash::<Zobrist64>(EnPassantMode::Legal).0
}

/// The same position as far as a search is concerned (see [`key`]).
pub fn same_position(a: &Chess, b: &Chess) -> bool {
    a.board() == b.board()
        && a.turn() == b.turn()
        && a.castles().castling_rights() == b.castles().castling_rights()
        && a.ep_square(EnPassantMode::Legal) == b.ep_square(EnPassantMode::Legal)
}

/// A game that reaches the position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    /// Record index in the database.
    pub index: u32,
    /// Half-moves from the game's start to the first time the position appears.
    pub ply: u32,
    /// Reached by exactly the moves of [`PositionQuery::line`].
    pub same_order: bool,
    /// Found in a variation, not in the main line (PGN databases only).
    pub in_variation: bool,
    /// The move the game played next (`None`: it ends in the position); filled in by
    /// [`Collector`].
    pub next: Option<UciMove>,
}

/// Results of games after a move (or of games ending in the position).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub games: u32,
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    /// Sum and number of the ratings of the games' players (for the average).
    pub elo_sum: u64,
    pub elo_count: u32,
    /// Latest year among the games (0: unknown).
    pub last_year: u16,
}

impl Tally {
    /// Adds one game: its result ("1-0", "1/2-1/2", "0-1", other), the players'
    /// ratings (0 = unknown) and its year (0 = unknown).
    pub fn add(&mut self, result: GameResult, white_elo: u16, black_elo: u16, year: u16) {
        self.games += 1;
        match result {
            GameResult::WhiteWins => self.white_wins += 1,
            GameResult::Draw => self.draws += 1,
            GameResult::BlackWins => self.black_wins += 1,
            GameResult::Unknown => {}
        }
        // The average rating counts games where both ratings are known, like the
        // "strongest first" sort.
        if white_elo > 0 && black_elo > 0 {
            self.elo_sum += u64::from(white_elo) + u64::from(black_elo);
            self.elo_count += 2;
        }
        self.last_year = self.last_year.max(year);
    }

    pub fn merge(&mut self, other: &Tally) {
        self.games += other.games;
        self.white_wins += other.white_wins;
        self.draws += other.draws;
        self.black_wins += other.black_wins;
        self.elo_sum += other.elo_sum;
        self.elo_count += other.elo_count;
        self.last_year = self.last_year.max(other.last_year);
    }

    /// Average rating of the players, if any is known.
    pub fn average_elo(&self) -> Option<u32> {
        (self.elo_count > 0).then(|| (self.elo_sum / u64::from(self.elo_count)) as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameResult {
    WhiteWins,
    Draw,
    BlackWins,
    Unknown,
}

impl GameResult {
    pub fn from_tag(tag: &str) -> GameResult {
        match tag.trim() {
            "1-0" => GameResult::WhiteWins,
            "0-1" => GameResult::BlackWins,
            "1/2-1/2" | "½-½" => GameResult::Draw,
            _ => GameResult::Unknown,
        }
    }
}

/// A move played in the position, with the games that played it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveStat {
    pub uci: UciMove,
    /// SAN in the searched position ("Nf3", "O-O").
    pub san: String,
    pub tally: Tally,
    /// Games among [`Tally::games`] that played the move in their main line; 0 when
    /// it was only seen in variations (PGN databases).
    pub main_line_games: u32,
}

/// The answer to a [`PositionQuery`].
#[derive(Clone, Debug, Default)]
pub struct PositionOutcome {
    /// Matching games in record order.
    pub matches: Vec<Match>,
    /// Moves played next, most played first.
    pub moves: Vec<MoveStat>,
    /// Games whose main line ends in the position.
    pub ended: Tally,
}

/// Collects matches and continuation statistics; one per search thread, then merged.
#[derive(Default)]
pub struct Collector {
    pub matches: Vec<Match>,
    moves: HashMap<UciMove, Tally>,
    /// Per move: games that played it in their main line.
    main_line: HashMap<UciMove, u32>,
    ended: Tally,
    /// The last match whose first continuation was recorded (it decides `next`).
    continued: Option<u32>,
}

impl Collector {
    /// Records a matching game: what it played next (`None`: the game ends here) and
    /// its result, ratings and year.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        m: Match,
        next: Option<UciMove>,
        result: GameResult,
        white_elo: u16,
        black_elo: u16,
        year: u16,
    ) {
        self.add_match(m);
        self.add_continuation(m.index, next, true, result, white_elo, black_elo, year);
    }

    /// Records a matching game without what it played next; its continuations follow
    /// with [`Collector::add_continuation`] (a PGN game can reach the position in
    /// several branches and continue differently in each).
    pub fn add_match(&mut self, m: Match) {
        self.matches.push(m);
    }

    /// Records a matching game whose [`Match::next`] is already decided (the move
    /// after its chosen occurrence); its continuations do not change it.
    pub fn add_match_with_next(&mut self, m: Match) {
        self.matches.push(m);
        self.continued = Some(m.index);
    }

    /// Records one continuation of game `index` (added with [`Collector::add_match`]):
    /// a move played next (`main_line`: in the game's main line) or `None` when the
    /// game ends here. The first move recorded for a game is its [`Match::next`].
    #[allow(clippy::too_many_arguments)]
    pub fn add_continuation(
        &mut self,
        index: u32,
        next: Option<UciMove>,
        main_line: bool,
        result: GameResult,
        white_elo: u16,
        black_elo: u16,
        year: u16,
    ) {
        // The first continuation belongs to the chosen occurrence: it is the game's
        // next move, even when it is the end of the game (later ones come from other
        // branches of a PGN game).
        if self.continued != Some(index)
            && let Some(m) = self.matches.last_mut()
            && m.index == index
        {
            m.next = next;
            self.continued = Some(index);
        }
        let tally = match next {
            Some(uci) => {
                if main_line {
                    *self.main_line.entry(uci).or_default() += 1;
                }
                self.moves.entry(uci).or_default()
            }
            None => &mut self.ended,
        };
        tally.add(result, white_elo, black_elo, year);
    }

    pub fn merge(&mut self, other: Collector) {
        self.matches.extend(other.matches);
        for (uci, t) in other.moves {
            self.moves.entry(uci).or_default().merge(&t);
        }
        for (uci, n) in other.main_line {
            *self.main_line.entry(uci).or_default() += n;
        }
        self.ended.merge(&other.ended);
    }

    /// The outcome for `position`: matches in record order, moves by popularity with
    /// SAN. Moves that are not legal in the position are left out.
    pub fn finish(mut self, position: &Chess) -> PositionOutcome {
        self.matches.sort_unstable_by_key(|m| m.index);
        let mut moves: Vec<MoveStat> = self
            .moves
            .into_iter()
            .filter_map(|(uci, tally)| {
                let m = uci.to_move(position).ok()?;
                let san = shakmaty::san::SanPlus::from_move(position.clone(), m).to_string();
                let main_line_games = self.main_line.get(&uci).copied().unwrap_or(0);
                Some(MoveStat {
                    uci,
                    san,
                    tally,
                    main_line_games,
                })
            })
            .collect();
        moves.sort_by(|a, b| {
            b.tally
                .games
                .cmp(&a.tally.games)
                .then_with(|| a.san.cmp(&b.san))
        });
        PositionOutcome {
            matches: self.matches,
            moves,
            ended: self.ended,
        }
    }
}

/// UCI notation of a move as played in standard chess (castling as the king's move).
pub fn uci(m: &Move) -> UciMove {
    m.to_uci(shakmaty::CastlingMode::Standard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::san::San;

    fn play(moves: &[&str]) -> Chess {
        let mut pos = Chess::default();
        for s in moves {
            let m = s.parse::<San>().unwrap().to_move(&pos).unwrap();
            pos.play_unchecked(m);
        }
        pos
    }

    #[test]
    fn transpositions_are_the_same_position() {
        let a = play(&["Nf3", "d5", "d4"]);
        let b = play(&["d4", "d5", "Nf3"]);
        assert!(same_position(&a, &b));
        assert_eq!(key(&a), key(&b));
        assert!(!same_position(&a, &play(&["d4", "d5"])));
    }

    #[test]
    fn collector_counts_and_names_moves() {
        let pos = Chess::default();
        let e4 = uci(&"e4".parse::<San>().unwrap().to_move(&pos).unwrap());
        let d4 = uci(&"d4".parse::<San>().unwrap().to_move(&pos).unwrap());
        let m = |index| Match {
            index,
            ply: 0,
            same_order: true,
            in_variation: false,
            next: None,
        };
        let mut a = Collector::default();
        a.add(m(3), Some(e4), GameResult::WhiteWins, 2700, 2600, 2020);
        a.add(m(1), Some(d4), GameResult::Draw, 0, 0, 0);
        let mut b = Collector::default();
        b.add(m(2), Some(e4), GameResult::BlackWins, 2500, 0, 2024);
        b.add(m(5), None, GameResult::Unknown, 0, 0, 0);
        // A game ending in its chosen occurrence keeps no next move, whatever its other
        // branches play.
        b.add_match(m(7));
        b.add_continuation(7, None, true, GameResult::Draw, 0, 0, 0);
        b.add_continuation(7, Some(e4), false, GameResult::Draw, 0, 0, 0);
        a.merge(b);
        let out = a.finish(&pos);
        assert_eq!(
            out.matches.iter().map(|m| m.index).collect::<Vec<_>>(),
            vec![1, 2, 3, 5, 7]
        );
        assert_eq!(out.matches[4].next, None, "ends in its chosen occurrence");
        assert_eq!(out.matches[2].next, Some(e4));
        assert_eq!(out.moves[0].san, "e4");
        assert_eq!(out.moves[0].tally.games, 3);
        // Only game 3 has both ratings.
        assert_eq!(out.moves[0].tally.average_elo(), Some(2650));
        assert_eq!(out.moves[0].tally.last_year, 2024);
        assert_eq!(out.moves[1].san, "d4");
        assert_eq!(out.ended.games, 2);
    }
}
