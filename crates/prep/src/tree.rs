//! The repertoire tree of one colour: the player's games laid over each other by
//! move order (it reads as a repertoire: 1.e4 c5 2.Nf3 …), every node carrying the
//! key of its position, so that two nodes reached by different move orders are seen
//! to be the same position (a transposition) and get the same opening name.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::games::{GameInfo, Tally};
use crate::openings::{self, OpeningSet};

/// A position of the tree: the move that led to it from its parent, and the games
/// that went through it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub key: u64,
    /// Half-moves from the start (0: the root).
    pub ply: u8,
    /// The move from the parent (empty for the root).
    pub uci: String,
    pub san: String,
    pub parent: Option<u32>,
    /// Most played first.
    pub children: Vec<u32>,
    pub tally: Tally,
    /// The latest date among the games (PGN form; empty when none has one).
    pub last_date: String,
    /// Sum and number of the known opponent ratings, for the average.
    pub elo_sum: u64,
    pub elo_count: u32,
    /// Games through the node: indices into [`crate::PrepReport::games`], in order.
    pub games: Vec<u32>,
    /// The same position is reached elsewhere in the tree by another move order.
    pub transposition: bool,
    /// The deepest named position on the path to the node
    /// ([`crate::PrepReport::openings`]); `named` when it is this one.
    pub opening: Option<usize>,
    pub named: bool,
}

impl Node {
    pub fn average_elo(&self) -> Option<u32> {
        (self.elo_count > 0).then(|| (self.elo_sum / u64::from(self.elo_count)) as u32)
    }
}

/// The tree of one colour; `nodes[0]` is the root (the start position).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    /// Lays the lines of `games[g]` for `g` in `which` over each other, `plies` deep.
    pub fn build(
        games: &[GameInfo],
        which: &[u32],
        plies: usize,
        openings: &mut OpeningSet,
    ) -> Tree {
        let mut tree = Tree {
            nodes: vec![Node {
                key: games
                    .first()
                    .and_then(|g| g.keys.first().copied())
                    .unwrap_or_default(),
                ply: 0,
                uci: String::new(),
                san: String::new(),
                parent: None,
                children: Vec::new(),
                tally: Tally::default(),
                last_date: String::new(),
                elo_sum: 0,
                elo_count: 0,
                games: Vec::new(),
                transposition: false,
                opening: None,
                named: false,
            }],
        };
        for &g in which {
            let game = &games[g as usize];
            if let Some(&k) = game.keys.first() {
                tree.nodes[0].key = k;
            }
            tree.count(0, g, game);
            let mut at = 0usize;
            for i in 0..game.ucis.len().min(plies) {
                let next = match tree.nodes[at]
                    .children
                    .iter()
                    .find(|&&c| tree.nodes[c as usize].uci == game.ucis[i])
                {
                    Some(&c) => c as usize,
                    None => {
                        let key = game.keys[i + 1];
                        let named = openings::by_position(key);
                        let opening = named
                            .map(|id| openings.intern(id))
                            .or(tree.nodes[at].opening);
                        tree.nodes.push(Node {
                            key,
                            ply: u8::try_from(i + 1).unwrap_or(u8::MAX),
                            uci: game.ucis[i].clone(),
                            san: game.sans[i].clone(),
                            parent: Some(at as u32),
                            children: Vec::new(),
                            tally: Tally::default(),
                            last_date: String::new(),
                            elo_sum: 0,
                            elo_count: 0,
                            games: Vec::new(),
                            transposition: false,
                            opening,
                            named: named.is_some(),
                        });
                        let c = tree.nodes.len() - 1;
                        tree.nodes[at].children.push(c as u32);
                        c
                    }
                };
                tree.count(next, g, game);
                at = next;
            }
        }
        // Most played first among the children.
        let played: Vec<u32> = tree.nodes.iter().map(|n| n.tally.games).collect();
        for n in &mut tree.nodes {
            n.children
                .sort_by(|a, b| played[*b as usize].cmp(&played[*a as usize]).then(a.cmp(b)));
        }
        // Transpositions: one key in several nodes.
        let mut seen: HashMap<u64, u32> = HashMap::new();
        for n in &tree.nodes {
            *seen.entry(n.key).or_default() += 1;
        }
        for n in &mut tree.nodes {
            n.transposition = seen.get(&n.key).is_some_and(|c| *c > 1);
        }
        tree
    }

    fn count(&mut self, node: usize, g: u32, game: &GameInfo) {
        let n = &mut self.nodes[node];
        n.tally.add(game.outcome);
        n.games.push(g);
        if game.opponent_elo > 0 {
            n.elo_sum += u64::from(game.opponent_elo);
            n.elo_count += 1;
        }
        if game.date_number() > chess_import::pgn_catalog::date_number(&n.last_date) {
            n.last_date = game.date.clone();
        }
    }

    /// The statistics of the position with `key` over all its move orders (the
    /// sum of its nodes).
    pub fn position_tally(&self, key: u64) -> Tally {
        let mut t = Tally::default();
        for n in self.nodes.iter().filter(|n| n.key == key) {
            t.games += n.tally.games;
            t.wins += n.tally.wins;
            t.draws += n.tally.draws;
            t.losses += n.tally.losses;
        }
        t
    }

    /// The path of moves to `node`, in SAN.
    pub fn path(&self, node: usize) -> Vec<&str> {
        let mut out = Vec::new();
        let mut at = Some(node);
        while let Some(i) = at {
            let n = &self.nodes[i];
            if n.parent.is_some() {
                out.push(n.san.as_str());
            }
            at = n.parent.map(|p| p as usize);
        }
        out.reverse();
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::games::Outcome;
    use chess_core::shakmaty::san::SanPlus;
    use chess_core::shakmaty::Chess;
    use chess_import::position::{key, uci};

    pub(crate) fn game(index: u32, white: bool, moves: &[&str], result: &str, date: &str, elo: u16) -> GameInfo {
        let mut pos = Chess::default();
        let (mut ucis, mut sans, mut keys) = (Vec::new(), Vec::new(), vec![key(&pos)]);
        for s in moves {
            let m = s.parse::<SanPlus>().unwrap().san.to_move(&pos).unwrap();
            ucis.push(uci(&m).to_string());
            sans.push(SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string());
            keys.push(key(&pos));
        }
        GameInfo {
            index,
            white,
            opponent: "Opp".into(),
            opponent_elo: elo,
            date: date.into(),
            outcome: Outcome::of(result, white),
            ucis,
            sans,
            keys,
            opening: None,
            named: None,
            book: None,
        }
    }

    #[test]
    fn the_tree_counts_games_and_marks_transpositions() {
        let games = vec![
            game(0, true, &["d4", "d5", "Nf3", "Nf6"], "1-0", "2026.01.05", 1500),
            game(1, true, &["Nf3", "d5", "d4", "Nf6"], "0-1", "2026.03.01", 0),
            game(2, true, &["d4", "d5", "c4"], "1/2-1/2", "2025.12.31", 1700),
            game(3, false, &["e4", "c5"], "0-1", "2026.02.02", 1600),
            game(4, true, &["d4", "d5", "c4", "e6", "Nc3", "Nf6", "h3"], "1-0", "", 0),
        ];
        let mut set = OpeningSet::default();
        let tree = Tree::build(&games, &[0, 1, 2], 20, &mut set);
        let root = &tree.nodes[0];
        assert_eq!(root.tally.games, 3);
        assert_eq!(root.last_date, "2026.03.01");
        assert_eq!(root.average_elo(), Some(1600));
        assert_eq!(root.games, vec![0, 1, 2]);
        // 1.d4 (2 games) before 1.Nf3 (1 game).
        let kids: Vec<&str> = root.children.iter().map(|&c| tree.nodes[c as usize].san.as_str()).collect();
        assert_eq!(kids, vec!["d4", "Nf3"]);
        // The position after 1.d4 d5 2.Nf3 / 1.Nf3 d5 2.d4 is in two nodes.
        let shared: Vec<&Node> = tree.nodes.iter().filter(|n| n.transposition).collect();
        let keys: Vec<u64> = shared.iter().map(|n| n.key).collect();
        assert_eq!(shared.len(), 4, "{keys:?}");
        let after_nf3 = tree.nodes.iter().find(|n| n.ply == 3 && n.san == "Nf3").unwrap();
        let after_d4 = tree.nodes.iter().find(|n| n.ply == 3 && n.san == "d4").unwrap();
        assert_eq!(after_nf3.key, after_d4.key);
        assert_eq!(tree.position_tally(after_nf3.key).games, 2);
        assert!(!tree.nodes.iter().find(|n| n.san == "c4").unwrap().transposition);
        assert_eq!(tree.path(after_d4.parent.unwrap() as usize), vec!["Nf3", "d5"]);
        // Names: 1.d4 d5 2.Nf3 is named in the table, 2...Nf6 is too (Zukertort);
        // both move orders get the same name.
        assert!(after_nf3.named && after_d4.named);
        assert_eq!(after_nf3.opening, after_d4.opening);
        let name = &set.list[after_nf3.opening.unwrap()].name;
        assert!(name.starts_with("Queen's Pawn Game"), "{name}");
        // A node that is not named (4.h3 in the Queen's Gambit Declined) inherits the
        // name of its path.
        let tree = Tree::build(&games, &[0, 1, 2, 4], 20, &mut set);
        let h3 = tree.nodes.iter().find(|n| n.san == "h3").unwrap();
        assert!(!h3.named);
        assert_eq!(h3.opening, tree.nodes[h3.parent.unwrap() as usize].opening);
        assert!(set.list[h3.opening.unwrap()].name.starts_with("Queen's Gambit Declined"));
        // Depth limit.
        let short = Tree::build(&games, &[0, 1, 2], 1, &mut set);
        assert_eq!(short.nodes.len(), 3);
    }
}
