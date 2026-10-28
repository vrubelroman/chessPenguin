//! Where the player's games leave the book: a second database (a big one from the
//! shelf, or an opening database) is the book, and a game has left it at the first
//! position the book has fewer than a threshold of games for. Whose move led out —
//! the player's or the opponent's — says whether it is his own choice.
//!
//! The positions of a line are searched the way the reference panel finds the end
//! of the theory ([`theory_end`]): the first plies one by one (an opening index
//! answers them at once), then by doubling and halving. Counts are kept per
//! position within one pass: the player's games share their openings.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chess_core::shakmaty::uci::UciMove;
use chess_core::shakmaty::{Chess, Position};
use chess_import::Database;
use chess_import::position::{PositionQuery, theory_end};

use crate::games::{BookExit, GameInfo};

/// How the book is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookOptions {
    /// A position with fewer games than this in the book is out of it.
    pub threshold: u32,
    /// The book is the database the report was built from: the player's own games
    /// do not count.
    pub same_database: bool,
}

impl Default for BookOptions {
    fn default() -> Self {
        BookOptions {
            threshold: crate::DEFAULT_BOOK_THRESHOLD,
            same_database: false,
        }
    }
}

/// Fills in [`GameInfo::book`] of every game; `progress` counts the games done.
/// Returns how many games left the book within their line, and the book's count of
/// every position looked at (by key). `None` when cancelled (or when the book cannot
/// be searched: a PGN book without memory for its table).
pub(crate) fn find_exits(
    games: &mut [GameInfo],
    book: &Database,
    options: &BookOptions,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<(u32, HashMap<u64, u32>)> {
    let exclude: HashSet<u32> = if options.same_database {
        games.iter().map(|g| g.index).collect()
    } else {
        HashSet::new()
    };
    let mut counts: HashMap<u64, u32> = HashMap::new();
    let mut left = 0;
    for game in games.iter_mut() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        progress.fetch_add(1, Ordering::Relaxed);
        let mut pos = Chess::default();
        let mut positions = vec![pos.clone()];
        let mut moves = Vec::with_capacity(game.ucis.len());
        for u in &game.ucis {
            let Some(m) = u.parse::<UciMove>().ok().and_then(|u| u.to_move(&pos).ok()) else {
                break;
            };
            pos.play_unchecked(m);
            moves.push(m);
            positions.push(pos.clone());
        }
        let keys = &game.keys;
        let mut count_of = |i: usize| -> Option<u32> {
            if let Some(c) = counts.get(&keys[i]) {
                return Some(*c);
            }
            let q = PositionQuery::new(positions[i].clone(), Some(moves[..i].to_vec()));
            let out = book.search_position(None, &q, &AtomicUsize::new(0), cancel)?;
            let c = out
                .matches
                .iter()
                .filter(|m| !exclude.contains(&m.index))
                .count() as u32;
            counts.insert(keys[i], c);
            Some(c)
        };
        let end = theory_end(positions.len(), 0, |i| {
            Some(count_of(i)? >= options.threshold)
        })?;
        game.book = end.first_unknown.map(|i| {
            left += 1;
            BookExit {
                ply: i as u32,
                // Position 0 is the start: a book without it is empty.
                by_player: i > 0 && game.player_moves_at(i - 1),
                count: counts.get(&game.keys[i]).copied().unwrap_or(0),
            }
        });
    }
    Some((left, counts))
}
