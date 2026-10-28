//! The player: picked from the names of the database (never free text, so that
//! "Вафин" offers "Вафин Радель · 271" and "Вафина Раяна · 10" and the user chooses),
//! as one or several name ids when the database spells the same person in several
//! ways ("Kasparov, G." and "Kasparov, Garry"), and the player's games by colour.

use std::sync::atomic::{AtomicBool, AtomicUsize};

use chess_import::Database;
use chess_import::chessbase::base::{Filter, NameFilter, Side, SortKey};
use serde::{Deserialize, Serialize};

/// A name of the database that matches a query, with its number of games.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: u32,
    pub name: String,
    pub games: u32,
}

/// Up to `limit` names containing every word of `query` (an initial with its full
/// stop finds the full name), names starting with the first word first, then by
/// number of games; and the number of all matches.
pub fn candidates(db: &Database, query: &str, limit: usize) -> (Vec<Candidate>, usize) {
    let (list, total) = db.suggest_players(query, limit);
    let list = list
        .into_iter()
        .map(|s| Candidate {
            id: s.id,
            name: s.name,
            games: s.games,
        })
        .collect();
    (list, total)
}

/// The player of a report: one or more name ids of the database.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Player {
    pub ids: Vec<u32>,
}

impl Player {
    pub fn new(ids: impl IntoIterator<Item = u32>) -> Player {
        let mut ids: Vec<u32> = ids.into_iter().collect();
        ids.sort_unstable();
        ids.dedup();
        Player { ids }
    }

    /// The names behind the ids, as the game list shows them.
    pub fn names(&self, db: &Database) -> Vec<String> {
        self.ids
            .iter()
            .map(|&id| db.player_name(id).to_owned())
            .collect()
    }
}

/// The player's games for `side`: (record index, the player had White), in record
/// order. `None` when cancelled.
pub fn games_of(
    db: &Database,
    player: &Player,
    side: Side,
    cancel: &AtomicBool,
) -> Option<Vec<(u32, bool)>> {
    let mut out = Vec::new();
    for &id in &player.ids {
        for (s, white) in [(Side::White, true), (Side::Black, false)] {
            if side != Side::Any && side != s {
                continue;
            }
            let filter = Filter {
                player: NameFilter {
                    text: String::new(),
                    exact: Some(id),
                },
                side: s,
                games_only: true,
                ..Filter::default()
            };
            let found = db.query(&filter, SortKey::Number, true, &AtomicUsize::new(0), cancel)?;
            out.extend(found.into_iter().map(|i| (i, white)));
        }
    }
    // The same record with the same colour twice: two ids of the player on one side
    // cannot happen, but two ids on both sides of one game can (a game against
    // oneself in a spelling-merged list): it is kept once per colour.
    out.sort_unstable();
    out.dedup();
    Some(out)
}
