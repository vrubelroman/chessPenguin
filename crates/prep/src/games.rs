//! The player's games as the report sees them: the first plies of the main line with
//! the keys of its positions, the result from the player's side, the date and the
//! opponent's rating. Games from a set-up position are left out.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chess_core::shakmaty::san::SanPlus;
use chess_core::shakmaty::{Chess, Position};
use chess_import::Database;
use chess_import::pgn_catalog::date_number;
use chess_import::position::{key, uci};
use serde::{Deserialize, Serialize};

use crate::openings::{self, OpeningSet};

/// A game's result from the player's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    Win,
    Draw,
    Loss,
    Unknown,
}

impl Outcome {
    /// From the result tag and the player's colour.
    pub fn of(result: &str, white: bool) -> Outcome {
        match (result.trim(), white) {
            ("1-0", true) | ("0-1", false) => Outcome::Win,
            ("1-0", false) | ("0-1", true) => Outcome::Loss,
            ("1/2-1/2" | "½-½", _) => Outcome::Draw,
            _ => Outcome::Unknown,
        }
    }
}

/// Games and their results from the player's side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub games: u32,
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
}

impl Tally {
    pub fn add(&mut self, outcome: Outcome) {
        self.games += 1;
        match outcome {
            Outcome::Win => self.wins += 1,
            Outcome::Draw => self.draws += 1,
            Outcome::Loss => self.losses += 1,
            Outcome::Unknown => {}
        }
    }

    /// Points per game, 0..1, over the games with a result.
    pub fn score(&self) -> Option<f64> {
        let decided = self.wins + self.draws + self.losses;
        (decided > 0).then(|| (f64::from(self.wins) + 0.5 * f64::from(self.draws)) / f64::from(decided))
    }
}

/// Where a game left the book (see [`crate::PrepReport::find_book_exits`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookExit {
    /// Moves played when the line reached its first position the book rarely has:
    /// move `ply - 1` (0-based, [`GameInfo::ucis`]) led out of the book.
    pub ply: u32,
    /// That move was the player's.
    pub by_player: bool,
    /// Games of the book with the position (below the threshold).
    pub count: u32,
}

/// One game of the player.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameInfo {
    /// Record index in the database.
    pub index: u32,
    /// The player had White.
    pub white: bool,
    pub opponent: String,
    /// 0: unknown.
    pub opponent_elo: u16,
    /// PGN form ("2026.10.18", "1997.05.??"), empty when unknown.
    pub date: String,
    pub outcome: Outcome,
    /// The first moves of the main line (at most the report's plies), UCI and SAN.
    pub ucis: Vec<String>,
    pub sans: Vec<String>,
    /// Keys of the positions before each move and after the last: `ucis.len() + 1`.
    pub keys: Vec<u64>,
    /// The deepest named position of the line ([`crate::PrepReport::openings`]).
    pub opening: Option<usize>,
    /// Plies of the line up to the named position (`keys[named]` is it).
    pub named: Option<u32>,
    /// Filled in by [`crate::PrepReport::find_book_exits`]; `None` while the line
    /// is within the book, or before the book was looked at.
    pub book: Option<BookExit>,
}

impl GameInfo {
    /// `yyyymmdd` for ordering (0: no date).
    pub fn date_number(&self) -> u32 {
        date_number(&self.date)
    }

    /// Move `ply` (0-based) was the player's.
    pub fn player_moves_at(&self, ply: usize) -> bool {
        ply.is_multiple_of(2) == self.white
    }

    /// "12. Nf3" / "12... Nf6" for move `ply` (0-based).
    pub fn move_label(&self, ply: usize) -> String {
        let san = self.sans.get(ply).map_or("", String::as_str);
        move_label(ply, san)
    }
}

/// "12. Nf3" / "12... Nf6" for move `ply` (0-based from the usual start position).
pub fn move_label(ply: usize, san: &str) -> String {
    let number = ply / 2 + 1;
    if ply.is_multiple_of(2) {
        format!("{number}. {san}")
    } else {
        format!("{number}... {san}")
    }
}

/// What a pass over the games produced.
pub struct Collected {
    pub games: Vec<GameInfo>,
    pub openings: OpeningSet,
    /// Games left out: from a set-up position, or unreadable.
    pub skipped: u32,
    /// Final positions of lines that end in checkmate or stalemate: (key, position).
    pub terminal: Vec<(u64, Chess)>,
}

/// Reads the first `plies` of each of `found` (record, the player had White);
/// `progress` counts the games done. `None` when cancelled.
pub fn collect(
    db: &Database,
    found: &[(u32, bool)],
    plies: usize,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<Collected> {
    let mut out = Collected {
        games: Vec::with_capacity(found.len()),
        openings: OpeningSet::default(),
        skipped: 0,
        terminal: Vec::new(),
    };
    for &(index, white) in found {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        progress.fetch_add(1, Ordering::Relaxed);
        let Some(line) = db.main_line(index as usize, plies) else {
            out.skipped += 1;
            continue;
        };
        if !line.standard_start {
            out.skipped += 1;
            continue;
        }
        let row = db.row(index as usize);
        let mut pos = Chess::default();
        let mut keys = Vec::with_capacity(line.moves.len() + 1);
        let mut ucis = Vec::with_capacity(line.moves.len());
        let mut sans = Vec::with_capacity(line.moves.len());
        keys.push(key(&pos));
        for m in &line.moves {
            ucis.push(uci(m).to_string());
            sans.push(SanPlus::from_move_and_play_unchecked(&mut pos, *m).to_string());
            keys.push(key(&pos));
        }
        if pos.is_checkmate() || pos.is_stalemate() {
            out.terminal.push((key(&pos), pos));
        }
        let (opening, named) = match openings::deepest(&keys) {
            Some((at, id)) => (Some(out.openings.intern(id)), Some(at as u32)),
            None => (None, None),
        };
        out.games.push(GameInfo {
            index,
            white,
            opponent: if white { row.black } else { row.white },
            opponent_elo: if white { row.black_elo } else { row.white_elo },
            date: row.date,
            outcome: Outcome::of(row.result, white),
            ucis,
            sans,
            keys,
            opening,
            named,
            book: None,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_and_tallies() {
        assert_eq!(Outcome::of("1-0", true), Outcome::Win);
        assert_eq!(Outcome::of("1-0", false), Outcome::Loss);
        assert_eq!(Outcome::of("0-1", false), Outcome::Win);
        assert_eq!(Outcome::of("1/2-1/2", true), Outcome::Draw);
        assert_eq!(Outcome::of("*", true), Outcome::Unknown);
        let mut t = Tally::default();
        for o in [Outcome::Win, Outcome::Win, Outcome::Draw, Outcome::Loss, Outcome::Unknown] {
            t.add(o);
        }
        assert_eq!((t.games, t.wins, t.draws, t.losses), (5, 2, 1, 1));
        assert_eq!(t.score(), Some(0.625));
        assert_eq!(Tally::default().score(), None);
        assert_eq!(move_label(0, "e4"), "1. e4");
        assert_eq!(move_label(1, "c5"), "1... c5");
        assert_eq!(move_label(22, "Nf3"), "12. Nf3");
    }
}
