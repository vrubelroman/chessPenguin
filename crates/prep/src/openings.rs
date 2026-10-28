//! Opening names from the Lichess `chess-openings` table: a position's ECO code and
//! name, looked up by the position itself, so a line reaching a named position by
//! another move order gets its name too. A line is named by its deepest named
//! position, as the table's README says ("go back through the moves until a named
//! position is found").
//!
//! The five TSV files in `data/` (`eco`, `name`, `pgn`) were downloaded unmodified from
//! <https://github.com/lichess-org/chess-openings> at revision
//! `f06a50d5c7c140e50b068049fa31d289e2c3d347` (2026-10-27), on 2026-10-28; the data set is
//! in the public domain (CC0 1.0). Their SHA-256 sums are in `THIRD_PARTY_NOTICES.md`.
//! The table is parsed on first use (3 865 lines, a fraction of a second).

use std::collections::HashMap;
use std::sync::OnceLock;

use chess_core::shakmaty::san::SanPlus;
use chess_core::shakmaty::{Chess, Position};
use chess_import::position::key;
use serde::{Deserialize, Serialize};

const FILES: [&str; 5] = [
    include_str!("../data/a.tsv"),
    include_str!("../data/b.tsv"),
    include_str!("../data/c.tsv"),
    include_str!("../data/d.tsv"),
    include_str!("../data/e.tsv"),
];

/// A named opening position of the table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    /// "B30".
    pub eco: String,
    /// "Sicilian Defense: Nyezhmetdinov-Rossolimo Attack".
    pub name: String,
    /// The table's moves to the position, in SAN ("1. e4 c5 2. Nf3 Nc6 3. Bb5").
    pub line: String,
    /// Half-moves of `line`.
    pub plies: u8,
}

impl Opening {
    /// The part of the name before the colon ("Sicilian Defense").
    pub fn family(&self) -> &str {
        self.name.split(':').next().unwrap_or(&self.name).trim()
    }

    /// The part after the colon ("Nyezhmetdinov-Rossolimo Attack"), if any.
    pub fn variation(&self) -> Option<&str> {
        self.name.split_once(':').map(|(_, v)| v.trim())
    }
}

struct Table {
    entries: Vec<Opening>,
    /// Position key → entry. A position reached by two lines of the table keeps the
    /// first (the files are in ECO order).
    by_key: HashMap<u64, usize>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Table {
            entries: Vec::with_capacity(4000),
            by_key: HashMap::with_capacity(4000),
        };
        for text in FILES {
            for line in text.lines().skip(1) {
                let mut cols = line.split('\t');
                let (Some(eco), Some(name), Some(pgn)) = (cols.next(), cols.next(), cols.next())
                else {
                    continue;
                };
                let Some((pos, plies)) = play(pgn) else {
                    continue;
                };
                let k = key(&pos);
                if t.by_key.contains_key(&k) {
                    continue;
                }
                t.by_key.insert(k, t.entries.len());
                t.entries.push(Opening {
                    eco: eco.to_owned(),
                    name: name.to_owned(),
                    line: pgn.to_owned(),
                    plies,
                });
            }
        }
        t
    })
}

/// Plays a table line ("1. e4 c5 2. Nf3"); `None` if a move is not legal.
fn play(pgn: &str) -> Option<(Chess, u8)> {
    let mut pos = Chess::default();
    let mut plies = 0u8;
    for tok in pgn.split_whitespace() {
        if tok.ends_with('.') {
            continue;
        }
        let m = tok.parse::<SanPlus>().ok()?.san.to_move(&pos).ok()?;
        pos.play_unchecked(m);
        plies = plies.checked_add(1)?;
    }
    Some((pos, plies))
}

/// Number of named positions.
pub fn count() -> usize {
    table().entries.len()
}

/// The entry of a named position (by its [`key`]).
pub fn by_position(k: u64) -> Option<usize> {
    table().by_key.get(&k).copied()
}

/// Entry `id` (from [`by_position`] or [`deepest`]).
pub fn get(id: usize) -> &'static Opening {
    &table().entries[id]
}

/// The name of a line (the positions `keys`, in order): its deepest named position,
/// its index in `keys` and its entry — but a deeper position whose name is only a
/// more general one ("Sicilian Defense" after 3.d4 in the "Modern Variations") does
/// not take the name back: the most precise name met stays.
pub fn deepest(keys: &[u64]) -> Option<(usize, usize)> {
    let mut out: Option<(usize, usize)> = None;
    for (i, k) in keys.iter().enumerate() {
        if let Some(id) = by_position(*k)
            && out.is_none_or(|(_, cur)| !more_general(id, cur))
        {
            out = Some((i, id));
        }
    }
    out
}

/// Entry `a`'s name is a more general form of entry `b`'s ("Sicilian Defense" of
/// "Sicilian Defense: Modern Variations"; "…: Najdorf Variation" of "…: Najdorf
/// Variation, English Attack").
pub fn more_general(a: usize, b: usize) -> bool {
    is_prefix_name(&get(a).name, &get(b).name)
}

/// `general` names a family or a variation `precise` is part of.
pub fn is_prefix_name(general: &str, precise: &str) -> bool {
    precise.len() > general.len()
        && precise.starts_with(general)
        && matches!(precise[general.len()..].chars().next(), Some(':' | ','))
}

/// The openings a report refers to, each once, in the order they were first met.
#[derive(Default)]
pub struct OpeningSet {
    pub list: Vec<Opening>,
    by_table: HashMap<usize, usize>,
}

impl OpeningSet {
    /// The place of table entry `id` in [`OpeningSet::list`].
    pub fn intern(&mut self, id: usize) -> usize {
        if let Some(&i) = self.by_table.get(&id) {
            return i;
        }
        self.list.push(get(id).clone());
        self.by_table.insert(id, self.list.len() - 1);
        self.list.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::shakmaty::san::San;

    fn keys(moves: &[&str]) -> Vec<u64> {
        let mut pos = Chess::default();
        let mut out = vec![key(&pos)];
        for s in moves {
            let m = s.parse::<San>().unwrap().to_move(&pos).unwrap();
            pos.play_unchecked(m);
            out.push(key(&pos));
        }
        out
    }

    #[test]
    fn the_table_loads() {
        // 3 865 lines; a few positions are reached by two lines of the table.
        assert!(count() > 3800, "{}", count());
        let start = by_position(key(&Chess::default()));
        assert!(start.is_none(), "the start position has no name");
    }

    #[test]
    fn the_deepest_named_position_names_the_line_whatever_the_move_order() {
        // 1.e4 c5 2.Nf3 Nc6 3.Bb5: the Rossolimo; 3...g6 is named too, 4.O-O is not.
        let k = keys(&["e4", "c5", "Nf3", "Nc6", "Bb5", "g6", "O-O"]);
        let (at, id) = deepest(&k).expect("named");
        let o = get(id);
        assert_eq!(o.eco, "B31");
        assert!(o.name.starts_with("Sicilian Defense: Nyezhmetdinov-Rossolimo Attack"), "{}", o.name);
        assert_eq!(at, 6, "the position after 3...g6");
        assert_eq!(o.family(), "Sicilian Defense");
        assert!(o.variation().is_some_and(|v| v.starts_with("Nyezhmetdinov")));
        assert_eq!(o.plies, 6);
        assert_eq!(o.line, "1. e4 c5 2. Nf3 Nc6 3. Bb5 g6");
        // A transposition: 1.Nf3 d5 2.d4 reaches the table's 1.d4 d5 2.Nf3.
        let a = deepest(&keys(&["Nf3", "d5", "d4"])).expect("named");
        let b = deepest(&keys(&["d4", "d5", "Nf3"])).expect("named");
        assert_eq!(a.1, b.1);
        assert_eq!(get(a.1).line, "1. d4 d5 2. Nf3");
        // A family without a variation.
        let o = get(deepest(&keys(&["e4", "c5"])).expect("named").1);
        assert_eq!(o.name, "Sicilian Defense");
        assert_eq!(o.variation(), None);
        // Nothing before the first move.
        assert_eq!(deepest(&keys(&[])), None);
        // A deeper position with a more general name keeps the precise one: the
        // table names 1.e4 c5 2.Nf3 d6 3.d4 just «Sicilian Defense».
        let line = keys(&["e4", "c5", "Nf3", "d6", "d4"]);
        let after_d4 = by_position(line[5]).map(|id| get(id).name.clone());
        assert_eq!(after_d4.as_deref(), Some("Sicilian Defense"));
        let (at, id) = deepest(&line).expect("named");
        assert_eq!((at, get(id).name.as_str()), (4, "Sicilian Defense: Modern Variations"));
        assert!(is_prefix_name("Sicilian Defense", "Sicilian Defense: Modern Variations"));
        assert!(is_prefix_name("A: B", "A: B, C"));
        assert!(!is_prefix_name("Sicilian Defense", "Sicilian Defense"));
        assert!(!is_prefix_name("English", "English Opening"));
    }

    #[test]
    fn a_set_keeps_each_opening_once() {
        let mut set = OpeningSet::default();
        let id = deepest(&keys(&["e4", "c5"])).unwrap().1;
        let other = deepest(&keys(&["d4"])).unwrap().1;
        assert_eq!(set.intern(id), 0);
        assert_eq!(set.intern(other), 1);
        assert_eq!(set.intern(id), 0);
        assert_eq!(set.list.len(), 2);
    }
}
