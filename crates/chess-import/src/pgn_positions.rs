//! Position search in a PGN database (books, collections: up to some hundred thousand
//! games): the positions of every game, variations included, are kept in memory once
//! built, so a search is a binary search in a sorted index plus a walk over the games
//! that reach the position.
//!
//! Layout. Every attached node of every game is a [`Node`] (8 bytes: parent, the move
//! that led to it, ply), games one after another, each in the order of its PGN text.
//! The index holds one entry per move of the tree (the key of the position before the
//! move, the node the move leads to) and one per line end (the key of the last
//! position, that node), sorted by key and node, so the entries of a game are
//! contiguous inside the range of a key. Per game: the first node and the tag values
//! the statistics need. About 20 bytes per node.
//!
//! Statistics. A game counts once among the matches, at its main line occurrence if
//! there is one, else (preferably reached by the moves of the query) at the first in
//! text order. Each distinct move the game plays from the position, in any branch,
//! adds the game once to that move; a game "ends here" when its main line ends in the
//! position (variation ends are not counted). See `docs/dev/import.md`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chess_core::{Game, MoveKind, PgnDatabase};
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Role, Square};

use crate::position::{self, Collector, GameResult, Match, PositionOutcome, PositionQuery};

/// `Node::parent` of a game's root.
const NO_PARENT: u32 = u32::MAX >> 1;
/// Flag in `Node::parent`: the node is in a variation (not the main line).
const VARIATION: u32 = 1 << 31;
/// Encoded teaching null move `--` (and anything that is not a normal move).
const NULL_MOVE: u16 = u16::MAX;
/// Nodes are numbered with 31 bits (an index entry keeps a flag in the lowest bit).
const MAX_NODES: usize = (1 << 31) - 1;
/// Games handed to a build thread at a time.
const CHUNK: usize = 32;
/// Index size from which it is sorted in parallel.
const PARALLEL_SORT: usize = 1 << 16;

/// A position of a game: the node of the move tree that reaches it.
#[derive(Clone, Copy, Debug)]
struct Node {
    /// Index of the parent node (`NO_PARENT` for the root), with the `VARIATION` flag.
    parent: u32,
    /// The move from the parent (see [`encode`]); unused for the root.
    mv: u16,
    /// Half-moves from the game's start (saturating).
    ply: u16,
}

impl Node {
    fn parent(self) -> Option<usize> {
        let p = self.parent & !VARIATION;
        (p != NO_PARENT).then_some(p as usize)
    }

    fn in_variation(self) -> bool {
        self.parent & VARIATION != 0
    }
}

/// What the statistics need from a game's tags, and whether it starts from the usual
/// position (only such a game can follow the query's move order).
#[derive(Clone, Copy, Debug)]
struct Meta {
    result: GameResult,
    white_elo: u16,
    black_elo: u16,
    year: u16,
    standard_start: bool,
}

/// The positions of all games of a database.
pub struct PositionTable {
    nodes: Vec<Node>,
    /// First node of each game, and the total at the end (`games + 1` values).
    starts: Vec<u32>,
    meta: Vec<Meta>,
    /// Index: position keys in ascending order ...
    keys: Vec<u64>,
    /// ... and per key `node << 1 | end`: the node a move from the position leads to,
    /// or (`end` = 1) the node of the position itself where its line ends.
    refs: Vec<u32>,
}

/// Part of the table built by one thread: consecutive games from `first`, with node
/// numbers local to the chunk.
struct Chunk {
    first: usize,
    nodes: Vec<Node>,
    starts: Vec<u32>,
    meta: Vec<Meta>,
    entries: Vec<(u64, u32)>,
}

impl PositionTable {
    /// Parses every game (`progress` counts the games done). `None` when cancelled.
    pub fn build(
        db: &PgnDatabase,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<PositionTable> {
        let games = db.len();
        let next = AtomicUsize::new(0);
        let total = AtomicUsize::new(0);
        let work = || -> Option<Vec<Chunk>> {
            let mut chunks = Vec::new();
            loop {
                let first = next.fetch_add(CHUNK, Ordering::Relaxed);
                if first >= games {
                    return Some(chunks);
                }
                let mut chunk = Chunk {
                    first,
                    nodes: Vec::new(),
                    starts: Vec::new(),
                    meta: Vec::new(),
                    entries: Vec::new(),
                };
                for index in first..(first + CHUNK).min(games) {
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    chunk.add_game(db, index, &total);
                    progress.fetch_add(1, Ordering::Relaxed);
                }
                chunks.push(chunk);
            }
        };
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(games.div_ceil(CHUNK))
            .max(1);
        let mut chunks = if threads == 1 {
            work()?
        } else {
            let parts: Vec<Option<Vec<Chunk>>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..threads).map(|_| s.spawn(work)).collect();
                handles
                    .into_iter()
                    .map(|h| h.join().ok().flatten())
                    .collect()
            });
            let mut all = Vec::new();
            for part in parts {
                all.extend(part?);
            }
            all
        };
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        chunks.sort_unstable_by_key(|c| c.first);
        Self::assemble(chunks, cancel)
    }

    /// Joins the chunks (in game order) into one table and sorts the index.
    fn assemble(chunks: Vec<Chunk>, cancel: &AtomicBool) -> Option<PositionTable> {
        let total_nodes: usize = chunks.iter().map(|c| c.nodes.len()).sum();
        let total_entries: usize = chunks.iter().map(|c| c.entries.len()).sum();
        let games: usize = chunks.iter().map(|c| c.meta.len()).sum();
        let mut table = PositionTable {
            nodes: Vec::with_capacity(total_nodes),
            starts: Vec::with_capacity(games + 1),
            meta: Vec::with_capacity(games),
            keys: Vec::new(),
            refs: Vec::new(),
        };
        let mut parts = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let offset = table.nodes.len() as u32;
            table.nodes.extend(chunk.nodes.into_iter().map(|mut n| {
                if n.parent().is_some() {
                    n.parent += offset;
                }
                n
            }));
            table.starts.extend(chunk.starts.iter().map(|s| s + offset));
            table.meta.extend(chunk.meta);
            let mut entries = chunk.entries;
            for e in &mut entries {
                e.1 += offset << 1;
            }
            parts.push(entries);
        }
        table.starts.push(table.nodes.len() as u32);
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let entries = sort_entries(parts, total_entries);
        table.keys = entries.iter().map(|e| e.0).collect();
        table.refs = entries.iter().map(|e| e.1).collect();
        Some(table)
    }

    /// Number of games.
    pub fn games(&self) -> usize {
        self.meta.len()
    }

    /// Number of positions (nodes of all game trees, roots included).
    pub fn positions(&self) -> usize {
        self.nodes.len()
    }

    /// Memory held by the table, in bytes.
    pub fn memory(&self) -> usize {
        self.nodes.capacity() * size_of::<Node>()
            + self.starts.capacity() * size_of::<u32>()
            + self.meta.capacity() * size_of::<Meta>()
            + self.keys.capacity() * size_of::<u64>()
            + self.refs.capacity() * size_of::<u32>()
    }

    /// Games among `candidates` (record indices in ascending order; `None`: all)
    /// that reach the position, in the main line or a variation.
    pub fn search(
        &self,
        db: &PgnDatabase,
        candidates: Option<&[u32]>,
        query: &PositionQuery,
        cancel: &AtomicBool,
    ) -> Option<PositionOutcome> {
        let _ = db;
        let key = query.key();
        let lo = self.keys.partition_point(|&k| k < key);
        let hi = lo + self.keys[lo..].partition_point(|&k| k == key);
        let line: Option<Vec<u16>> = query
            .line
            .as_ref()
            .map(|l| l.iter().map(|m| encode(position::uci(m))).collect());
        let mut collector = Collector::default();
        let mut cand = 0;
        let mut game = 0;
        let mut i = lo;
        while i < hi {
            if (i - lo) % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                return None;
            }
            let node = self.refs[i] >> 1;
            if node >= self.starts[game + 1] {
                game += self.starts[game..].partition_point(|&s| s <= node) - 1;
            }
            let end = self.starts[game + 1];
            let mut j = i + 1;
            while j < hi && self.refs[j] >> 1 < end {
                j += 1;
            }
            let wanted = match candidates {
                None => true,
                Some(c) => {
                    cand += c[cand..].partition_point(|&g| (g as usize) < game);
                    c.get(cand).is_some_and(|&g| g as usize == game)
                }
            };
            if wanted {
                self.collect(game, &self.refs[i..j], line.as_deref(), &mut collector);
            }
            i = j;
        }
        Some(collector.finish(&query.position))
    }

    /// Adds game `game` whose entries for the position are `refs`.
    fn collect(&self, game: usize, refs: &[u32], line: Option<&[u16]>, out: &mut Collector) {
        let meta = self.meta[game];
        // Best occurrence: main line first, then reached by the query's moves, then
        // the first in text order (smallest node).
        let mut best: Option<(bool, bool, usize)> = None;
        // Distinct moves played from the position, with "in the main line".
        let mut moves: Vec<(u16, bool)> = Vec::new();
        let mut ended = false;
        for &r in refs {
            let node = (r >> 1) as usize;
            let at = if r & 1 == 1 {
                if !self.nodes[node].in_variation() {
                    ended = true;
                }
                node
            } else {
                let n = self.nodes[node];
                let main = !n.in_variation();
                match moves.iter_mut().find(|m| m.0 == n.mv) {
                    Some(m) => m.1 |= main,
                    None => moves.push((n.mv, main)),
                }
                n.parent().unwrap_or(node)
            };
            let rank = (
                self.nodes[at].in_variation(),
                !self.same_order(at, meta, line),
                at,
            );
            if best.is_none_or(|b| rank < b) {
                best = Some(rank);
            }
        }
        let Some((in_variation, other_order, at)) = best else {
            return;
        };
        let index = game as u32;
        out.add_match(Match {
            index,
            ply: u32::from(self.nodes[at].ply),
            same_order: !other_order,
            in_variation,
            next: None,
        });
        // The move after the chosen occurrence goes first: it is the game's `next`.
        let first = self.first_child_move(at, refs);
        if let Some(pos) = moves.iter().position(|m| Some(m.0) == first) {
            moves.swap(0, pos);
        }
        let result = meta.result;
        for (mv, main) in moves {
            out.add_continuation(
                index,
                Some(decode(mv)),
                main,
                result,
                meta.white_elo,
                meta.black_elo,
                meta.year,
            );
        }
        if ended {
            out.add_continuation(
                index,
                None,
                true,
                result,
                meta.white_elo,
                meta.black_elo,
                meta.year,
            );
        }
    }

    /// The move of the first child of node `at` among the entries `refs` of its game
    /// (children come in text order, so the smallest node is the main continuation).
    fn first_child_move(&self, at: usize, refs: &[u32]) -> Option<u16> {
        refs.iter()
            .filter(|&&r| r & 1 == 0)
            .map(|&r| (r >> 1) as usize)
            .find(|&n| self.nodes[n].parent() == Some(at))
            .map(|n| self.nodes[n].mv)
    }

    /// Whether node `at` is reached from the usual start position by exactly `line`.
    fn same_order(&self, at: usize, meta: Meta, line: Option<&[u16]>) -> bool {
        let Some(line) = line else {
            return false;
        };
        if !meta.standard_start || usize::from(self.nodes[at].ply) != line.len() {
            return false;
        }
        let mut cur = at;
        for &mv in line.iter().rev() {
            let n = self.nodes[cur];
            if n.mv != mv {
                return false;
            }
            match n.parent() {
                Some(p) => cur = p,
                None => return false,
            }
        }
        self.nodes[cur].parent().is_none()
    }
}

impl Chunk {
    /// Parses record `index` and appends its positions (a record that does not parse,
    /// or one past `MAX_NODES` nodes counted in `total` over all chunks, adds a game
    /// without nodes).
    fn add_game(&mut self, db: &PgnDatabase, index: usize, total: &AtomicUsize) {
        let base = self.nodes.len();
        self.starts.push(base as u32);
        let entry = &db.entries()[index];
        let tag = |name| entry.tag(name).unwrap_or("");
        let elo = |name| tag(name).trim().parse::<u16>().unwrap_or(0);
        let mut meta = Meta {
            result: GameResult::from_tag(tag("Result")),
            white_elo: elo("WhiteElo"),
            black_elo: elo("BlackElo"),
            year: tag("Date")
                .get(..4)
                .and_then(|y| y.parse::<u16>().ok())
                .unwrap_or(0),
            standard_start: false,
        };
        if let Some(parsed) = db.load_game(index) {
            let game = &parsed.game;
            meta.standard_start =
                game.position(game.root()).map(position::key) == Some(standard_key());
            let len = game.arena_len();
            if total.fetch_add(len, Ordering::Relaxed) + len < MAX_NODES {
                self.add_tree(game);
            }
        }
        self.meta.push(meta);
    }

    /// Appends the attached nodes of `game` in text order (the order of the node
    /// arena) and their index entries.
    fn add_tree(&mut self, game: &Game) {
        let base = self.nodes.len();
        let preorder = game.attached_nodes();
        // Variation flags (parents come first in preorder).
        let mut variation = vec![false; game.arena_len()];
        for &id in &preorder {
            if let Some(p) = game.parent(id) {
                variation[id.index()] = variation[p.index()] || game.main_child(p) != Some(id);
            }
        }
        let mut order = preorder;
        order.sort_unstable();
        let mut local = vec![NO_PARENT; game.arena_len()];
        for (k, id) in order.iter().enumerate() {
            local[id.index()] = (base + k) as u32;
        }
        for &id in &order {
            let Some(node) = game.node(id) else { continue };
            let parent = node
                .parent()
                .map_or(NO_PARENT, |p| local[p.index()].min(NO_PARENT));
            let mv = match node.game_move().map(|m| m.kind) {
                Some(MoveKind::Normal(m)) => encode(position::uci(&m)),
                _ => NULL_MOVE,
            };
            self.nodes.push(Node {
                parent: if variation[id.index()] {
                    parent | VARIATION
                } else {
                    parent
                },
                mv,
                ply: u16::try_from(node.ply()).unwrap_or(u16::MAX),
            });
            let key = position::key(node.position());
            let children = node.children();
            if children.is_empty() {
                self.entries.push((key, (local[id.index()] << 1) | 1));
            }
            for c in children {
                self.entries.push((key, local[c.index()] << 1));
            }
        }
    }
}

fn standard_key() -> u64 {
    static KEY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *KEY.get_or_init(|| position::key(&Chess::default()))
}

/// Joins the index entries of the chunks (`total` in all) sorted by key and node; in
/// parallel for a large index.
fn sort_entries(parts: Vec<Vec<(u64, u32)>>, total: usize) -> Vec<(u64, u32)> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    if threads == 1 || total < PARALLEL_SORT {
        let mut entries = parts.concat();
        entries.sort_unstable();
        return entries;
    }
    // Keys are uniform: distribute by their top bits into buckets (freeing each part
    // once copied), then sort the buckets in parallel; no merge is needed.
    let buckets = (threads * 4).next_power_of_two();
    let shift = 64 - buckets.trailing_zeros();
    let bucket = |e: &(u64, u32)| (e.0 >> shift) as usize;
    let mut counts = vec![0usize; buckets];
    for e in parts.iter().flatten() {
        counts[bucket(e)] += 1;
    }
    let mut offsets: Vec<usize> = counts
        .iter()
        .scan(0, |sum, c| {
            let start = *sum;
            *sum += c;
            Some(start)
        })
        .collect();
    let mut sorted = vec![(0u64, 0u32); total];
    for part in parts {
        for e in part {
            let b = bucket(&e);
            sorted[offsets[b]] = e;
            offsets[b] += 1;
        }
    }
    let mut groups: Vec<Vec<&mut [(u64, u32)]>> = (0..threads).map(|_| Vec::new()).collect();
    let mut rest = sorted.as_mut_slice();
    for (b, c) in counts.iter().enumerate() {
        let (part, tail) = rest.split_at_mut(*c);
        groups[b % threads].push(part);
        rest = tail;
    }
    std::thread::scope(|s| {
        for group in groups {
            s.spawn(move || {
                for part in group {
                    part.sort_unstable();
                }
            });
        }
    });
    sorted
}

/// A move in 16 bits: from (6), to (6), promotion role (3). Anything else (the null
/// move) is `NULL_MOVE`, which no normal move encodes to.
fn encode(m: UciMove) -> u16 {
    match m {
        UciMove::Normal {
            from,
            to,
            promotion,
        } => {
            u16::from(u8::from(from))
                | u16::from(u8::from(to)) << 6
                | u16::from(promotion.map_or(0, |r| r as u8)) << 12
        }
        _ => NULL_MOVE,
    }
}

fn decode(v: u16) -> UciMove {
    if v == NULL_MOVE {
        return UciMove::Null;
    }
    UciMove::Normal {
        from: Square::new(u32::from(v & 63)),
        to: Square::new(u32::from((v >> 6) & 63)),
        promotion: Role::try_from(v >> 12).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::san::San;
    use shakmaty::{CastlingMode, Move, Position};

    const PGN: &str = r#"[Event "0: other order"]
[Result "1-0"]
[WhiteElo "2600"]
[BlackElo "2400"]
[Date "2020.01.01"]

1. Nf3 d5 2. d4 Nf6 1-0

[Event "1: same order"]
[Result "0-1"]
[Date "2024.??.??"]

1. d4 d5 2. Nf3 e6 0-1

[Event "2: only in a variation"]
[Result "*"]

1. e4 (1. d4 d5 2. Nf3 c5) 1... e5 *

[Event "3: ends here"]
[Result "1/2-1/2"]

1. d4 d5 2. Nf3 1/2-1/2

[Event "4: set up"]
[SetUp "1"]
[FEN "rnbqkbnr/ppp1pppp/8/3p4/3P4/5N2/PPP1PPPP/RNBQKB1R b KQkq - 1 2"]
[Result "1-0"]

2... Nf6 1-0

[Event "5: null move"]
[Result "*"]

1. e4 -- 2. d4 *

[Event "6: two branches"]
[Result "0-1"]

1. d4 d5 2. Nf3 (2. c4 e6) 2... Nf6 (2... c6 3. e3) 3. e3 0-1
"#;

    fn db(text: &str) -> PgnDatabase {
        let never = AtomicBool::new(false);
        PgnDatabase::from_bytes(text.as_bytes().to_vec(), None, &mut |_| {}, &never).unwrap()
    }

    fn table(db: &PgnDatabase) -> PositionTable {
        let never = AtomicBool::new(false);
        PositionTable::build(db, &AtomicUsize::new(0), &never).unwrap()
    }

    fn line(moves: &[&str]) -> (Chess, Vec<Move>) {
        let mut pos = Chess::default();
        let mut line = Vec::new();
        for s in moves {
            let m = s.parse::<San>().unwrap().to_move(&pos).unwrap();
            line.push(m);
            pos.play_unchecked(m);
        }
        (pos, line)
    }

    fn search(t: &PositionTable, db: &PgnDatabase, q: &PositionQuery) -> PositionOutcome {
        t.search(db, None, q, &AtomicBool::new(false)).unwrap()
    }

    fn query(moves: &[&str]) -> PositionQuery {
        let (pos, l) = line(moves);
        PositionQuery::new(pos, Some(l))
    }

    #[test]
    fn finds_transpositions_variations_and_set_up_games() {
        let db = db(PGN);
        let t = table(&db);
        assert_eq!(t.games(), 7);
        let out = search(&t, &db, &query(&["d4", "d5", "Nf3"]));
        let m = |index, ply, same_order, in_variation| Match {
            index,
            ply,
            same_order,
            in_variation,
            next: None,
        };
        // Compared without the next moves (checked in `counts_each_move_of_a_game_once`).
        let found: Vec<Match> = out
            .matches
            .iter()
            .map(|x| Match { next: None, ..*x })
            .collect();
        assert_eq!(
            found,
            vec![
                m(0, 3, false, false),
                m(1, 3, true, false),
                m(2, 3, true, true),
                m(3, 3, true, false),
                m(4, 0, false, false),
                m(6, 3, true, false),
            ]
        );
    }

    #[test]
    fn counts_each_move_of_a_game_once() {
        let db = db(PGN);
        let t = table(&db);
        let out = search(&t, &db, &query(&["d4", "d5", "Nf3"]));
        let stat = |san: &str| {
            out.moves
                .iter()
                .find(|s| s.san == san)
                .unwrap_or_else(|| panic!("{san} in {:?}", out.moves))
        };
        let nf6 = stat("Nf6");
        assert_eq!(
            (nf6.tally.games, nf6.main_line_games),
            (3, 3),
            "games 0, 4, 6"
        );
        assert_eq!(
            (nf6.tally.white_wins, nf6.tally.black_wins, nf6.tally.draws),
            (2, 1, 0)
        );
        assert_eq!(nf6.tally.average_elo(), Some(2500));
        assert_eq!(nf6.tally.last_year, 2020);
        let e6 = stat("e6");
        assert_eq!((e6.tally.games, e6.tally.last_year), (1, 2024));
        assert_eq!((stat("c5").tally.games, stat("c5").main_line_games), (1, 0));
        assert_eq!((stat("c6").tally.games, stat("c6").main_line_games), (1, 0));
        assert_eq!(out.moves[0].san, "Nf6");
        assert_eq!(out.moves.len(), 4);
        assert_eq!(out.ended.games, 1);
        assert_eq!(out.ended.draws, 1);
        // The move after the chosen occurrence is the game's next move.
        let nf6_uci = stat("Nf6").uci;
        let next = |index: u32| {
            out.matches
                .iter()
                .find(|m| m.index == index)
                .and_then(|m| m.next)
        };
        assert_eq!(next(6), Some(nf6_uci));
        assert_eq!(next(2), Some(stat("c5").uci));
        assert_eq!(next(3), None);
    }

    #[test]
    fn a_variation_end_is_not_a_game_end() {
        let db = db(PGN);
        let t = table(&db);
        // After 2...c6 3.e3 in game 6 (a variation end) and 2...Nf6 3.e3 (main line
        // end): one game, which ends here in its main line.
        let out = search(&t, &db, &query(&["d4", "d5", "Nf3", "c6", "e3"]));
        assert_eq!(out.matches.len(), 1);
        assert!(out.matches[0].in_variation && out.matches[0].same_order);
        assert_eq!(out.ended.games, 0);
        let out = search(&t, &db, &query(&["d4", "d5", "Nf3", "Nf6", "e3"]));
        assert_eq!(out.ended.games, 1);
        assert!(!out.matches[0].in_variation);
    }

    #[test]
    fn positions_after_a_null_move_are_found() {
        let db = db(PGN);
        let t = table(&db);
        let fen: Fen = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2"
            .parse()
            .unwrap();
        let pos: Chess = fen.into_position(CastlingMode::Standard).unwrap();
        let out = search(&t, &db, &PositionQuery::new(pos, None));
        assert_eq!(out.matches.len(), 1);
        assert_eq!(out.matches[0].index, 5);
        assert_eq!(out.matches[0].ply, 2);
        assert!(!out.matches[0].same_order);
        assert_eq!(out.moves.len(), 1);
        assert_eq!(out.moves[0].san, "d4");
        // The null move itself is not a move of the position before it.
        let out = search(&t, &db, &query(&["e4"]));
        assert_eq!(
            out.matches.iter().map(|m| m.index).collect::<Vec<_>>(),
            vec![2, 5]
        );
        assert_eq!(out.moves.len(), 1, "{:?}", out.moves);
        assert_eq!(out.moves[0].san, "e5");
    }

    #[test]
    fn start_position_and_candidates() {
        let db = db(PGN);
        let t = table(&db);
        let out = search(&t, &db, &query(&[]));
        assert_eq!(
            out.matches.iter().map(|m| m.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 5, 6]
        );
        assert!(out.matches.iter().all(|m| m.same_order && m.ply == 0));
        let e4 = out.moves.iter().find(|s| s.san == "e4").unwrap();
        assert_eq!((e4.tally.games, e4.main_line_games), (2, 2));
        let d4 = out.moves.iter().find(|s| s.san == "d4").unwrap();
        assert_eq!((d4.tally.games, d4.main_line_games), (4, 3));

        let q = query(&["d4", "d5", "Nf3"]);
        let never = AtomicBool::new(false);
        let out = t.search(&db, Some(&[1, 2, 5, 9]), &q, &never).unwrap();
        assert_eq!(
            out.matches.iter().map(|m| m.index).collect::<Vec<_>>(),
            vec![1, 2]
        );
        let out = t.search(&db, Some(&[]), &q, &never).unwrap();
        assert!(out.matches.is_empty() && out.moves.is_empty());
        let out = search(&t, &db, &query(&["h4", "h5"]));
        assert!(out.matches.is_empty());
    }

    #[test]
    fn cancel_stops_build_and_search() {
        let db = db(PGN);
        let t = table(&db);
        let cancel = AtomicBool::new(true);
        let progress = AtomicUsize::new(0);
        assert!(PositionTable::build(&db, &progress, &cancel).is_none());
        assert!(t.search(&db, None, &query(&[]), &cancel).is_none());
        let progress = AtomicUsize::new(0);
        PositionTable::build(&db, &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn many_games_build_in_parallel_like_one() {
        // Enough games for several threads and the bucket sort.
        let mut text = String::new();
        for _ in 0..3000 {
            text.push_str(PGN);
            text.push('\n');
        }
        let db = db(&text);
        let t = table(&db);
        assert_eq!(t.games(), 21_000);
        assert!(t.keys.len() >= PARALLEL_SORT, "the bucket sort is used");
        assert!(
            t.keys
                .iter()
                .zip(&t.refs)
                .collect::<Vec<_>>()
                .windows(2)
                .all(|w| w[0] <= w[1])
        );
        let out = search(&t, &db, &query(&["d4", "d5", "Nf3"]));
        assert_eq!(out.matches.len(), 6 * 3000);
        assert_eq!(out.moves[0].tally.games, 3 * 3000);
        assert_eq!(out.matches[6 * 1000 + 2].index, 7 * 1000 + 2);
        assert!(out.matches[6 * 1000 + 2].in_variation);
    }

    #[test]
    fn moves_encode_reversibly() {
        for uci in ["e2e4", "e1g1", "a7a8q", "h2h1n", "b7c8r", "g7h8b"] {
            let m: UciMove = uci.parse().unwrap();
            assert_eq!(decode(encode(m)), m);
            assert_ne!(encode(m), NULL_MOVE);
        }
        assert_eq!(decode(encode(UciMove::Null)), UciMove::Null);
    }
}
