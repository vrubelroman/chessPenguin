//! Opening names: the table lives in the `chess-openings` crate (the game review uses it
//! too); here only what a report keeps of it.

use std::collections::HashMap;

pub use chess_openings::*;

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
    use chess_core::shakmaty::Chess;
    use chess_core::shakmaty::Position;
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
