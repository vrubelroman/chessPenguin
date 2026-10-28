//! The "Reference" tab of a game («Справочник»): what was played in the current
//! position in a database of the shelf (Mega for theory, an opening collection, a
//! pupil's games), following the game's moves. Two tables, as in ChessBase: the moves
//! played from the position with their statistics, and the strongest games that
//! reached it. The database is picked in the panel and remembered; by default the
//! largest one on the shelf. The same position search as the library's search board
//! (`docs/dev/import.md`), run in the background for each new position. It also tells
//! how long the game's main line stays in the database ("theory") and whether the game
//! itself is there. Layout: `docs/design/reference.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chess_core::{Game, NodeId};
use chess_import::Database;
use chess_import::position::{Match, MoveStat, PositionOutcome, PositionQuery, Tally, theory_end};
use egui::{RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Move, Position};

use crate::board_search::{self, Background};
use crate::i18n::{tr, trf};
use crate::large_db::grouped;

/// A database of the shelf as the panel offers it.
pub struct RefDb {
    pub path: PathBuf,
    pub name: String,
    /// Loaded (with its number of records), or not yet.
    pub db: Option<Arc<Database>>,
    /// It could not be opened (why).
    pub failed: Option<String>,
    /// Size of its file, to pick the largest one before any is opened.
    pub bytes: u64,
}

/// What the panel asks of the game view and the application.
pub enum RefAction {
    /// Play this move in the game (as a move on the board: added, undoable).
    Play(Move),
    /// Use this database from now on.
    Choose(PathBuf),
    /// Load the chosen database (it is on the shelf but not open yet).
    Load(PathBuf),
    /// Open a game of the reference database at this position.
    OpenGame(PathBuf, usize, Chess),
    /// Go to this node of the game (the last move still in the database).
    Goto(NodeId),
    /// Show the position on the library's search board of this database.
    FindInLibrary(PathBuf),
}

/// A game of the games table.
#[derive(Clone, Debug)]
struct GameRow {
    index: usize,
    white: String,
    white_elo: u16,
    black: String,
    black_elo: u16,
    result: String,
    year: u16,
    event: String,
    /// The move the game played from the position in the line it is listed for
    /// (`None`: it ends here).
    next: Option<UciMove>,
    /// The three half-moves from the position, "5.Nc3 a6 6.Be2".
    then: String,
}

#[cfg(test)]
impl GameRow {
    fn average(&self) -> u32 {
        average(self.white_elo, self.black_elo)
    }
}

fn average(w: u16, b: u16) -> u32 {
    if w > 0 && b > 0 {
        (u32::from(w) + u32::from(b)) / 2
    } else {
        0
    }
}

/// A search result with what the tables show already worked out (picking the strongest
/// of millions of matches, reading the games' moves: work for the background).
pub struct Found {
    out: PositionOutcome,
    /// The strongest games, the newest games; per move the strongest that played it.
    strongest: Vec<GameRow>,
    newest: Vec<GameRow>,
    by_move: HashMap<UciMove, Vec<GameRow>>,
    by_move_newest: HashMap<UciMove, Vec<GameRow>>,
    /// Per move: the strongest players who played it (their own rating).
    players: HashMap<UciMove, String>,
    /// Most games have no result: a book of chapters (counts are chapters, no results).
    books: bool,
}

/// Identity of a request: database, position and the moves that reached it.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    position: u64,
    line: Option<Vec<String>>,
}

/// How far a game's main line stays in the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookEnd {
    /// The last node of the main line still found in other games (`None`: not even
    /// the start position).
    pub last: Option<NodeId>,
    /// The whole main line is in the database (a game with moves).
    pub to_the_end: bool,
    /// Copies of this very game in the database (same players and year, same final
    /// position), and the game itself when it comes from it: left out of "theory" and
    /// of the games table.
    pub copies: Vec<usize>,
}

/// The theory of one game: the main line it was worked out for (UCI moves) and the
/// answer. A longer line that starts with it keeps the answer when the theory ended
/// inside it.
struct Book {
    path: PathBuf,
    line: Vec<String>,
    end: BookEnd,
}

/// Database and main line (UCI) of a theory request.
type BookKey = (PathBuf, Vec<String>);

/// Results kept per database and position, at most this many matches in all; a single
/// bigger result is shown but not kept.
const CACHE_MATCHES: usize = 5_000_000;
/// Games listed: overall and per move; "then" is read for the most played moves.
const GAMES_SHOWN: usize = 100;
const GAMES_PER_MOVE: usize = 40;
const MOVES_WITH_GAMES: usize = 12;
/// A PGN database up to this many matches has the continuations of every game read
/// from its tree (a game can reach the position in several variations).
const PGN_TREE_MATCHES: usize = 5_000;
/// Results bigger than the cache kept apart (the start position, 1.e4).
const BIG_KEPT: usize = 2;

/// Sort of the games table.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum GameSort {
    #[default]
    Strongest,
    Newest,
}

#[derive(Default)]
pub struct Reference {
    /// Databases of the shelf (set by the application every frame).
    pub dbs: Vec<RefDb>,
    job: Option<(Key, Background<Found>)>,
    shown: Option<(Key, Arc<Found>)>,
    cache: HashMap<Key, Arc<Found>>,
    /// The fast-search files of a large base, built when it is first used here.
    prepare: Option<(PathBuf, Background<bool>)>,
    prepared: Vec<PathBuf>,
    /// The theory of the game shown, the job working it out, and a request that failed
    /// (not retried until the game or the database changes).
    book: Option<Book>,
    book_job: Option<(BookKey, Background<BookEnd>)>,
    book_failed: Option<BookKey>,
    /// The move of the moves table under the pointer; the selected one (it filters the
    /// games), for its database and position.
    hover: Option<UciMove>,
    selected: Option<(PathBuf, u64, UciMove)>,
    sort: GameSort,
    /// A request that could not be answered (no memory for a PGN table…): not retried.
    failed: Option<Key>,
    /// The last results too big for the cache (the start position of Mega: 11.7
    /// million matches), kept apart so going back to them is instant.
    big: Vec<(Key, Arc<Found>)>,
}

/// The position of `node` and the moves that reach it from the usual start (`None`
/// for a game from a set-up position, or a line with a null move).
fn query_for(game: &Game, node: NodeId) -> Option<PositionQuery> {
    let pos = game.position(node)?.clone();
    let line = board_search::line_to(game, node).and_then(|(start, line)| {
        chess_import::position::same_position(&start, &Chess::default()).then_some(line)
    });
    Some(PositionQuery::new(pos, line))
}

fn key(path: &Path, q: &PositionQuery) -> Key {
    Key {
        path: path.to_owned(),
        position: q.key(),
        line: q.line.as_ref().map(|l| {
            l.iter()
                .map(|m| chess_import::position::uci(m).to_string())
                .collect()
        }),
    }
}

/// The main line as UCI moves (a null move as "--").
fn main_line(game: &Game) -> Vec<String> {
    let mut out = Vec::new();
    let mut node = game.root();
    while let Some(next) = game.main_child(node) {
        out.push(
            game.game_move(next)
                .and_then(|m| m.uci())
                .unwrap_or_else(|| "--".into()),
        );
        node = next;
    }
    out
}

/// "40…Ke5": a move of the game with the number and side of the position before it
/// (games from a set-up position count from their FEN).
fn node_label(game: &Game, node: NodeId) -> Option<String> {
    let m = game.game_move(node)?;
    let before = game.position(game.parent(node)?)?;
    let n = before.fullmoves().get();
    let san = crate::san::show(&m.san);
    Some(match before.turn() {
        Color::White => format!("{n}.{san}"),
        Color::Black => format!("{n}…{san}"),
    })
}

/// "5.Nc3" / "5…Nc6".
fn move_label(pos: &Chess, m: Move) -> String {
    let san = shakmaty::san::SanPlus::from_move(pos.clone(), m).to_string();
    let san = crate::san::show(&san);
    let n = pos.fullmoves().get();
    match pos.turn() {
        Color::White => format!("{n}.{san}"),
        Color::Black => format!("{n}…{san}"),
    }
}

/// The surname of "Carlsen, Magnus" / "Alekhine Alexander (RUS)" (the first word, as
/// databases write names surname first); empty for an unknown player.
fn surname(name: &str) -> String {
    let name = match name.find('(') {
        Some(i) => &name[..i],
        None => name,
    }
    .trim();
    // The first word before a comma: "Carlsen, Magnus", "Duke of Brunswick, Count"
    // and "Duke Karl / Count Isouard" all compare by it.
    let before = name.split_once(',').map_or(name, |(s, _)| s);
    let s = before.split_whitespace().next().unwrap_or("");
    if s == "?" || s == "NN" {
        String::new()
    } else {
        s.to_owned()
    }
}

/// Ratings and year of a match without reading names (cheap for millions).
fn ratings(db: &Database, index: usize) -> (u16, u16, u16) {
    match db {
        Database::ChessBase(b) => {
            let r = b.record(index);
            let u = |at: usize, n: usize| -> u32 {
                r.get(at..at + n)
                    .map_or(0, |s| s.iter().fold(0, |a, x| (a << 8) | u32::from(*x)))
            };
            (
                u(0x1f, 2) as u16,
                u(0x21, 2) as u16,
                ((u(0x18, 3) >> 9) & 0xfff) as u16,
            )
        }
        Database::Pgn(p) => {
            let e = &p.entries()[index];
            let elo = |k: &str| e.tag(k).and_then(|v| v.parse().ok()).unwrap_or(0);
            let year = e
                .tag("Date")
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok())
                .unwrap_or(0);
            (elo("WhiteElo"), elo("BlackElo"), year)
        }
    }
}

/// The header fields of a game of the database (no moves).
fn row_of(db: &Database, index: usize, next: Option<UciMove>) -> GameRow {
    let (white_elo, black_elo, year) = ratings(db, index);
    let (white, black, result, event) = match db {
        Database::ChessBase(b) => {
            let r = b.row(index);
            (r.white, r.black, r.result.to_owned(), r.event)
        }
        Database::Pgn(p) => {
            let e = &p.entries()[index];
            let t = |k: &str| e.tag(k).unwrap_or("").to_owned();
            (t("White"), t("Black"), t("Result"), t("Event"))
        }
    };
    GameRow {
        index,
        white,
        white_elo,
        black,
        black_elo,
        result,
        year,
        event,
        next,
        then: String::new(),
    }
}

/// "5.Nc3 a6 6.Be2" from `node` on: its child playing `next`, then the main line.
fn then_from(game: &Game, node: NodeId, next: Option<UciMove>) -> String {
    let Some(next) = next else {
        return String::new();
    };
    let next = next.to_string();
    let Some(mut child) = game.children(node).iter().copied().find(|c| {
        game.game_move(*c)
            .and_then(|m| m.uci())
            .is_some_and(|u| u == next)
    }) else {
        return String::new();
    };
    let mut out = Vec::new();
    for k in 0..3 {
        let Some(m) = game.game_move(child) else {
            break;
        };
        let Some(before) = game.parent(child).and_then(|p| game.position(p)) else {
            break;
        };
        let n = before.fullmoves().get();
        // Latin here (made in the background, kept): shown through `san::show_line`.
        out.push(match before.turn() {
            Color::White => format!("{n}.{}", m.san),
            Color::Black if k == 0 => format!("{n}…{}", m.san),
            Color::Black => m.san.clone(),
        });
        match game.main_child(child) {
            Some(c) => child = c,
            None => break,
        }
    }
    out.join(" ")
}

/// The nodes of `game` in the position `key`, in tree order (main line first).
fn occurrences(game: &Game, key: u64) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![game.root()];
    while let Some(n) = stack.pop() {
        if game
            .position(n)
            .is_some_and(|p| chess_import::position::key(p) == key)
        {
            out.push(n);
        }
        stack.extend(game.children(n).iter().rev().copied());
    }
    out
}

/// What a game of a PGN database played from the position, in every branch, each
/// with its "then": `(next, then)`; the game's own `Match::next` first.
fn pgn_continuations(db: &Database, m: &Match, key: u64) -> Vec<(Option<UciMove>, String)> {
    let Some(parsed) = db.load_game(m.index as usize) else {
        return vec![(m.next, String::new())];
    };
    let g = &parsed.game;
    let mut out: Vec<(Option<UciMove>, String)> = Vec::new();
    for node in occurrences(g, key) {
        let children = g.children(node);
        if children.is_empty() {
            if !out.iter().any(|(n, _)| n.is_none()) {
                out.push((None, String::new()));
            }
            continue;
        }
        for c in children {
            let Some(u) = g
                .game_move(*c)
                .and_then(|x| x.uci())
                .and_then(|u| u.parse::<UciMove>().ok())
            else {
                continue;
            };
            if !out.iter().any(|(n, _)| *n == Some(u)) {
                out.push((Some(u), then_from(g, node, Some(u))));
            }
        }
    }
    // The match's own continuation leads (it is the one counted for the game first).
    if let Some(i) = out.iter().position(|(n, _)| *n == m.next) {
        out.swap(0, i);
    }
    if out.is_empty() {
        out.push((m.next, String::new()));
    }
    out
}

/// "Then" of a game whose main line reaches the position at `m.ply` (a ChessBase
/// search looks at main lines only).
fn main_line_then(db: &Database, m: &Match) -> String {
    let Some(parsed) = db.load_game(m.index as usize) else {
        return String::new();
    };
    let g = &parsed.game;
    let mut node = g.root();
    for _ in 0..m.ply {
        match g.main_child(node) {
            Some(n) => node = n,
            None => return String::new(),
        }
    }
    then_from(g, node, m.next)
}

/// The `n` best of `items` by `score` (highest first), without sorting all of them.
fn best<T: Copy, K: Ord>(mut items: Vec<T>, n: usize, score: impl Fn(&T) -> K) -> Vec<T> {
    if items.len() > n {
        items.select_nth_unstable_by(n, |a, b| score(b).cmp(&score(a)));
        items.truncate(n);
    }
    items.sort_by_key(|x| std::cmp::Reverse(score(x)));
    items
}

/// One match for picking: its number among the matches, average rating, year and the
/// rating of the player to move.
type Fact = (usize, u32, u16, u16);

/// Works out what the tables show for an outcome. `None` when cancelled (a newer
/// position was asked for).
fn tables(
    db: &Database,
    position: &Chess,
    out: PositionOutcome,
    cancel: &AtomicBool,
) -> Option<Found> {
    use std::sync::atomic::Ordering;
    let stopped = || cancel.load(Ordering::Relaxed);
    let key = chess_import::position::key(position);
    let mover_white = position.turn() == Color::White;
    let facts: Vec<Fact> = out
        .matches
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let (w, b, y) = ratings(db, m.index as usize);
            (k, average(w, b), y, if mover_white { w } else { b })
        })
        .collect();
    if stopped() {
        return None;
    }
    let by_strength = |x: &Fact| (x.1, x.2, std::cmp::Reverse(x.0));
    let by_year = |x: &Fact| (x.2, x.1, std::cmp::Reverse(x.0));
    let pgn = matches!(db, Database::Pgn(_));
    // What each match played from the position: every branch of a PGN game when the
    // result is small enough to read them all, else the chosen occurrence's move.
    let mut continuations: HashMap<usize, Vec<(Option<UciMove>, String)>> = HashMap::new();
    if pgn && out.matches.len() <= PGN_TREE_MATCHES {
        for (k, m) in out.matches.iter().enumerate() {
            if k % 256 == 0 && stopped() {
                return None;
            }
            continuations.insert(k, pgn_continuations(db, m, key));
        }
    }
    let moves_of = |k: usize| -> Vec<Option<UciMove>> {
        match continuations.get(&k) {
            Some(c) => c.iter().map(|(n, _)| *n).collect(),
            None => vec![out.matches[k].next],
        }
    };
    // A row of the games table; "then" (reading the game's moves) only when asked.
    let row = |k: usize, next: Option<UciMove>, then: bool| -> GameRow {
        let m = &out.matches[k];
        let mut r = row_of(db, m.index as usize, next);
        if !then {
            return r;
        }
        r.then = match continuations.get(&k) {
            Some(c) => c
                .iter()
                .find(|(n, _)| *n == next)
                .map(|(_, t)| t.clone())
                .unwrap_or_default(),
            None if pgn => pgn_continuations(db, m, key)
                .into_iter()
                .find(|(n, _)| *n == next)
                .map(|(_, t)| t)
                .unwrap_or_default(),
            None => main_line_then(db, m),
        };
        r
    };
    let strongest = best(facts.clone(), GAMES_SHOWN, by_strength)
        .into_iter()
        .map(|f| row(f.0, out.matches[f.0].next, true))
        .collect();
    if stopped() {
        return None;
    }
    let newest = best(facts.clone(), GAMES_SHOWN, by_year)
        .into_iter()
        .map(|f| row(f.0, out.matches[f.0].next, true))
        .collect();
    // Per move of the table: its strongest and newest games and its strongest players
    // ("then" for the most played moves; the rest are rarely looked at).
    let main_moves: Vec<UciMove> = out
        .moves
        .iter()
        .take(MOVES_WITH_GAMES)
        .map(|m| m.uci)
        .collect();
    let mut groups: HashMap<UciMove, Vec<Fact>> = HashMap::new();
    for f in &facts {
        for n in moves_of(f.0).into_iter().flatten() {
            groups.entry(n).or_default().push(*f);
        }
    }
    let mut by_move = HashMap::new();
    let mut by_move_newest = HashMap::new();
    let mut players = HashMap::new();
    for (mv, list) in groups {
        if stopped() {
            return None;
        }
        let then = main_moves.contains(&mv);
        let mut names: Vec<String> = Vec::new();
        for f in best(list.clone(), 400, |x: &Fact| {
            (x.3, x.2, std::cmp::Reverse(x.0))
        }) {
            let r = row_of(db, out.matches[f.0].index as usize, None);
            let s = surname(if mover_white { &r.white } else { &r.black });
            if !s.is_empty() && !names.contains(&s) {
                names.push(s);
            }
            if names.len() == 2 {
                break;
            }
        }
        players.insert(mv, names.join(", "));
        let rows: Vec<GameRow> = best(list.clone(), GAMES_PER_MOVE, by_strength)
            .into_iter()
            .map(|f| row(f.0, Some(mv), then))
            .collect();
        by_move.insert(mv, rows);
        let rows: Vec<GameRow> = best(list, GAMES_PER_MOVE, by_year)
            .into_iter()
            .map(|f| row(f.0, Some(mv), then))
            .collect();
        by_move_newest.insert(mv, rows);
    }
    let books = match db {
        Database::Pgn(p) => {
            let decided = p
                .entries()
                .iter()
                .filter(|e| matches!(e.result(), Some("1-0" | "0-1" | "1/2-1/2")))
                .count();
            decided * 2 < p.len()
        }
        Database::ChessBase(_) => false,
    };
    Some(Found {
        out,
        strongest,
        newest,
        by_move,
        by_move_newest,
        players,
        books,
    })
}

/// Games among `matches` with the players and year of `game` (the same game, maybe
/// from another source).
fn same_game(db: &Database, game: &Game, matches: &[Match]) -> Vec<usize> {
    let (w, b) = (
        surname(game.tag("White").unwrap_or("")).to_lowercase(),
        surname(game.tag("Black").unwrap_or("")).to_lowercase(),
    );
    if w.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let year: u16 = game
        .tag("Date")
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse().ok())
        .unwrap_or(0);
    matches
        .iter()
        .filter(|m| {
            let r = row_of(db, m.index as usize, None);
            surname(&r.white).to_lowercase() == w
                && surname(&r.black).to_lowercase() == b
                && (year == 0 || r.year == 0 || r.year == year)
        })
        .map(|m| m.index as usize)
        .collect()
}

/// The theory of `game`'s main line in `db`. `exclude`: the game itself when it comes
/// from this database.
fn book_end(
    db: &Database,
    game: &Game,
    exclude: Option<usize>,
    from: usize,
    cancel: &AtomicBool,
) -> Option<BookEnd> {
    let mut nodes = vec![game.root()];
    while let Some(n) = game.main_child(*nodes.last()?) {
        nodes.push(n);
    }
    let search = |q: &PositionQuery| db.search_position(None, q, &Default::default(), cancel);
    // Copies: games with the same players and year that reach the final position.
    let mut copies: Vec<usize> = exclude.into_iter().collect();
    if nodes.len() > 1
        && let Some(q) = query_for(game, *nodes.last()?)
    {
        let out = search(&q)?;
        if out.matches.len() <= 10_000 {
            copies.extend(same_game(db, game, &out.matches));
        }
    }
    copies.sort_unstable();
    copies.dedup();
    let known = |ply: usize| -> Option<bool> {
        let Some(q) = query_for(game, nodes[ply]) else {
            return Some(false);
        };
        let out = search(&q)?;
        Some(
            out.matches
                .iter()
                .any(|m| copies.binary_search(&(m.index as usize)).is_err()),
        )
    };
    // The first plies one by one (the opening index answers them at once), then by
    // doubling and halving (`theory_end`). `from`: the plies before it are known
    // already (the game grew past a theory that reached its end).
    let end = theory_end(nodes.len(), from, known)?;
    Some(BookEnd {
        last: end.last_known.map(|p| nodes[p]),
        to_the_end: nodes.len() > 1 && end.first_unknown.is_none(),
        copies,
    })
}

/// "Mega Database 2026 · 11,7 млн партий" for the database picker.
fn db_label(d: &RefDb) -> String {
    match &d.db {
        Some(db) => format!("{} · {}", d.name, crate::i18n::games_short(db.len())),
        None => format!("{} · {}", d.name, tr("not open")),
    }
}

impl Reference {
    /// The move under the pointer, or the selected one (an arrow on the board).
    pub fn hover(&self) -> Option<UciMove> {
        self.hover.or(self.selected.as_ref().map(|(_, _, m)| *m))
    }

    /// The database in use: the chosen one if it is on the shelf, else the largest
    /// file on the shelf.
    fn current<'a>(&'a self, chosen: Option<&Path>) -> Option<&'a RefDb> {
        chosen
            .and_then(|c| self.dbs.iter().find(|d| d.path == c))
            .or_else(|| self.dbs.iter().max_by_key(|d| d.bytes))
    }

    /// Collects finished work.
    fn poll(&mut self) {
        if let Some((k, job)) = &self.job
            && let Some(r) = job.poll()
        {
            let k = k.clone();
            self.job = None;
            match r {
                Some(found) => {
                    let found = Arc::new(found);
                    let n = found.out.matches.len();
                    if n <= CACHE_MATCHES {
                        let kept: usize = self.cache.values().map(|o| o.out.matches.len()).sum();
                        if kept + n > CACHE_MATCHES {
                            self.cache.clear();
                        }
                        self.cache.insert(k.clone(), found.clone());
                    } else {
                        self.big.retain(|(bk, _)| *bk != k);
                        self.big.push((k.clone(), found.clone()));
                        if self.big.len() > BIG_KEPT {
                            self.big.remove(0);
                        }
                    }
                    self.shown = Some((k, found));
                }
                None => self.failed = Some(k),
            }
        }
        if let Some((path, job)) = &self.prepare
            && job.poll().is_some()
        {
            self.prepared.push(path.clone());
            self.prepare = None;
        }
        if let Some(((path, line), job)) = &self.book_job
            && let Some(r) = job.poll()
        {
            let (path, line) = (path.clone(), line.clone());
            self.book_job = None;
            match r {
                Some(end) => self.book = Some(Book { path, line, end }),
                None => self.book_failed = Some((path, line)),
            }
        }
    }

    /// The theory for this game and database, if worked out: also for a longer line
    /// when the theory ended inside the one it was worked out for.
    fn book_for(&self, path: &Path, line: &[String]) -> Option<&BookEnd> {
        let b = self.book.as_ref().filter(|b| b.path == path)?;
        if b.line == line {
            return Some(&b.end);
        }
        (!b.line.is_empty() && line.starts_with(&b.line) && !b.end.to_the_end).then_some(&b.end)
    }

    /// Starts what the position and the database need.
    fn request(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        db: &Arc<Database>,
        game: &Game,
        query: &PositionQuery,
        exclude: Option<usize>,
    ) {
        let now = ctx.input(|i| i.time);
        // A large base gets its search index once (as the search board does).
        if let Some(b) = db.as_chessbase()
            && self.prepare.is_none()
            && !self.prepared.iter().any(|p| p == path)
            && b.prepare_wanted()
        {
            let db2 = db.clone();
            if let Some(job) = Background::spawn(ctx, now, move |progress, cancel| {
                db2.as_chessbase()?.prepare(progress, cancel).ok()
            }) {
                self.prepare = Some((path.to_owned(), job));
            }
        }
        let k = key(path, query);
        let kept = self
            .cache
            .get(&k)
            .or_else(|| self.big.iter().find(|(bk, _)| *bk == k).map(|(_, f)| f));
        if let Some(found) = kept {
            self.shown = Some((k.clone(), found.clone()));
            self.job = None;
        } else if self.failed.as_ref() != Some(&k)
            && self.shown.as_ref().is_none_or(|(sk, _)| *sk != k)
            && self.job.as_ref().is_none_or(|(jk, _)| *jk != k)
        {
            let (db2, q) = (db.clone(), query.clone());
            self.job = Background::spawn(ctx, now, move |progress, cancel| {
                let out = db2.search_position(None, &q, progress, cancel)?;
                tables(&db2, &q.position, out, cancel)
            })
            .map(|j| (k, j));
        }
        // The theory of the main line, once per game and database (kept while the
        // game grows past its end).
        let line = main_line(game);
        let bk = (path.to_owned(), line.clone());
        let known = self.book_for(path, &line).is_some()
            || self.book_job.as_ref().is_some_and(|(k, _)| *k == bk)
            || self.book_failed.as_ref() == Some(&bk);
        if !known && line.is_empty() {
            // An empty game: no theory and no copies to look for.
            self.book = Some(Book {
                path: path.to_owned(),
                line,
                end: BookEnd {
                    last: None,
                    to_the_end: false,
                    copies: exclude.into_iter().collect(),
                },
            });
        } else if !known && self.job.is_none() {
            // After the tables (both searching at once would compete for the machine).
            // A game grown past a theory that reached its end checks the new moves only.
            let from = self
                .book
                .as_ref()
                .filter(|b| b.path == path && b.end.to_the_end && line.starts_with(&b.line))
                .map_or(0, |b| b.line.len() + 1);
            let (db2, g) = (db.clone(), game.clone());
            self.book_job = Background::spawn(ctx, now, move |_, cancel| {
                book_end(&db2, &g, exclude, from, cancel)
            })
            .map(|j| (bk, j));
        }
    }

    /// The panel. `chosen`: the database picked last time; `source`: where the game
    /// comes from (it is left out of "theory" when it is the same database).
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        game: &Game,
        cursor: NodeId,
        chosen: Option<&Path>,
        source: Option<(&Path, usize)>,
    ) -> Option<RefAction> {
        self.poll();
        self.hover = None;
        let mut action = None;
        let Some(current) = self.current(chosen) else {
            ui.add_space(8.0);
            ui.label(
                RichText::new(tr(
                    "Add a database to the library to see what was played here.",
                ))
                .weak(),
            );
            return None;
        };
        let path = current.path.clone();
        let db = current.db.clone();
        let failed = current.failed.clone();
        let current_label = db_label(current);
        let query = query_for(game, cursor);
        let exclude = source.filter(|(p, _)| *p == path).map(|(_, i)| i);
        if let (Some(db), Some(q)) = (&db, &query) {
            self.request(ui.ctx(), &path, db, game, q, exclude);
            if self.job.is_some() || self.prepare.is_some() || self.book_job.is_some() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        // The novelty (the first move of the game no other game of the database played)
        // and a copy of the game in the database, once worked out.
        let line = main_line(game);
        let book = db
            .is_some()
            .then(|| self.book_for(&path, &line).cloned())
            .flatten();
        let novelty = book.as_ref().and_then(|b| match b.last {
            _ if b.to_the_end || line.is_empty() => None,
            Some(last) => game.main_child(last),
            None => game.main_child(game.root()),
        });
        let copy = book
            .as_ref()
            .and_then(|b| b.copies.iter().copied().find(|c| Some(*c) != exclude));
        // The first line: the database, then the novelty and the copy beside it (they
        // come a moment later; on this line they take no room of their own).
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Database"));
            // What is left of the row after the label, and the box shortens its text to
            // that: in a wrapping row `available_width` is the whole row (with it a long
            // name pushed the side panel out under the board).
            let width = ui.available_size_before_wrap().x.min(340.0);
            ui.allocate_ui(vec2(width, ui.spacing().interact_size.y), |ui| {
                egui::ComboBox::from_id_salt("reference_db")
                    .selected_text(current_label)
                    .width(width)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for d in &self.dbs {
                            if ui.selectable_label(d.path == path, db_label(d)).clicked()
                                && d.path != path
                            {
                                action = Some(RefAction::Choose(d.path.clone()));
                            }
                        }
                    });
            });
            if let Some(node) = novelty {
                row_gap(ui, 8.0);
                ui.label(RichText::new(tr("Novelty:")).weak());
                if ui
                    .link(node_label(game, node).unwrap_or_default())
                    .on_hover_text(tr(
                        "The first move of this game that no other game of this database played; before it the game followed games of the database",
                    ))
                    .clicked()
                {
                    action = Some(RefAction::Goto(node));
                }
            }
            if let (Some(c), Some(q)) = (copy, &query) {
                row_gap(ui, 8.0);
                ui.label(RichText::new(tr("This game is in the database")).weak());
                if ui
                    .link(tr("open it"))
                    .on_hover_text(tr(
                        "Open the copy of this game from the database at this position",
                    ))
                    .clicked()
                {
                    action = Some(RefAction::OpenGame(path.clone(), c, q.position.clone()));
                }
            }
        });
        if let Some(e) = &failed {
            ui.add_space(8.0);
            ui.label(
                RichText::new(trf("This database cannot be opened: {e}", &[("e", e)]))
                    .color(ui.visuals().warn_fg_color),
            );
            return action;
        }
        let Some(db) = db else {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(tr("Opening the database…")).weak());
            });
            return action.or(Some(RefAction::Load(path)));
        };
        let Some(query) = query else {
            return action;
        };
        let pos = query.position.clone();
        if let Some((p, job)) = &self.prepare
            && *p == path
        {
            let work = db.as_chessbase().map_or(1, |b| b.prepare_work()).max(1);
            let pct = 100.0 * job.progress() as f32 / work as f32;
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(
                    RichText::new(trf(
                        "Speeding up search in this database (once)… {p} %",
                        &[("p", &format!("{:.0}", pct.min(100.0)))],
                    ))
                    .weak(),
                )
                .on_hover_text(tr(
                    "An index for fast position search is built once and kept in the cache. Searching works meanwhile, only slower.",
                ));
            });
        }
        // Where we are: the position and what the game played from it.
        let side = if pos.turn() == Color::White {
            tr("White to move")
        } else {
            tr("Black to move")
        };
        let mut context = match node_label(game, cursor) {
            Some(m) => format!("{side} · {}", trf("after {move}", &[("move", &m)])),
            None => side.to_owned(),
        };
        let played = game
            .main_child(cursor)
            .and_then(|n| game.game_move(n))
            .and_then(|m| m.uci())
            .and_then(|u| u.parse::<UciMove>().ok());
        if let Some(m) = played.and_then(|u| u.to_move(&pos).ok()) {
            context.push_str(&format!(
                " · {}",
                trf("in the game {m}", &[("m", &move_label(&pos, m))])
            ));
        }
        ui.label(RichText::new(context).weak());
        ui.separator();
        let current_key = key(&path, &query);
        let shown = self
            .shown
            .as_ref()
            .filter(|(k, _)| *k == current_key)
            .map(|(_, o)| o.clone());
        let Some(found) = shown else {
            if self.failed.as_ref() == Some(&current_key) {
                let text = match db.positions_blocked() {
                    Some(need) => {
                        let free = chess_import::memory::available().unwrap_or(0);
                        let missing = (need + (256 << 20)).saturating_sub(free) / 1_000_000;
                        trf(
                            "Not enough memory for position search in this database: about {n} MB more is needed. Close other programs and try again.",
                            &[("n", &missing.max(1))],
                        )
                    }
                    None => tr("This position could not be counted").to_owned(),
                };
                ui.label(RichText::new(text).color(ui.visuals().warn_fg_color));
                if ui.small_button(tr("Try again")).clicked() {
                    self.failed = None;
                    self.book_failed = None;
                }
                return action;
            }
            // A short search shows nothing at all rather than a blinking spinner.
            let started = self.job.as_ref().map_or(0.0, |(_, j)| j.started);
            if ui.input(|i| i.time) - started > 0.3 {
                let total = db.len().max(1);
                let p = self.job.as_ref().map_or(0, |(_, j)| j.progress());
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    let text = if p > 0 {
                        trf(
                            "Counting… {p} %",
                            &[("p", &format!("{:.0}", (100 * p / total).min(100)))],
                        )
                    } else {
                        tr("Counting…").to_owned()
                    };
                    ui.label(RichText::new(text).weak());
                });
            }
            return action;
        };
        if found.out.matches.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(tr("This position is not in this database")).weak());
            return action;
        }
        // A selection belongs to its database and position.
        let pos_key = query.key();
        if self
            .selected
            .as_ref()
            .is_some_and(|(p, k, _)| *p != path || *k != pos_key)
        {
            self.selected = None;
        }
        if let Some(a) = self.moves_table(ui, &path, &pos, &found, pos_key, played) {
            action = Some(a);
        }
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
        let copies = book.map(|b| b.copies).unwrap_or_default();
        if let Some(a) = self.games_table(ui, &path, &pos, &found, &copies) {
            action = Some(a);
        }
        action
    }

    /// The moves from the position: statistics, click selects, double click plays.
    /// `played`: the move the game played here (marked).
    fn moves_table(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        pos: &Chess,
        found: &Found,
        pos_key: u64,
        played: Option<UciMove>,
    ) -> Option<RefAction> {
        let out = &found.out;
        let books = found.books;
        let total: u32 = out.moves.iter().map(|m| m.tally.games).sum::<u32>() + out.ended.games;
        let mut action = None;
        let start = chess_import::position::same_position(pos, &Chess::default());
        let rows = out.moves.len() + usize::from(out.ended.games > 0);
        if rows == 0 {
            // The games go on with a null move (a teaching "--"), which is not a move.
            ui.label(RichText::new(tr("Nothing was played here in this database")).weak());
            return None;
        }
        // As tall as its rows, but at most 40 % of the panel (at least 120 px of it).
        let natural = 26.0 * (rows as f32 + 1.0) + 8.0;
        let height = natural.min((ui.available_height() * 0.4).max(120.0));
        // Columns that give way in a narrow panel: players, year, White's score, rating,
        // the result bar. Each is shown only while the table still fits the panel (in
        // this order): a wider table would push the whole side panel under the board.
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let mut used = 96.0 + 64.0 + 44.0 + 2.0 * gap;
        let mut fits = |w: f32, wanted: bool| {
            let ok = wanted && used + w + gap <= width;
            if ok {
                used += w + gap;
            }
            ok
        };
        let show_bar = fits(86.0, !books);
        let show_elo = fits(56.0, !books);
        let show_score = fits(48.0, !books);
        let show_year = fits(44.0, true);
        let show_players = fits(80.0, !books);
        // Text in the rows must not take the clicks meant for the row.
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("reference_moves")
            .striped(true)
            .sense(Sense::click())
            .max_scroll_height(height)
            .min_scrolled_height(height)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            // "12…Крxe2+" and figurines are wider than Latin letters.
            .column(Column::exact(96.0).clip(true))
            .column(Column::exact(64.0))
            .column(Column::exact(44.0));
        if show_bar {
            table = table.column(Column::exact(86.0));
        }
        if show_score {
            table = table.column(Column::exact(48.0));
        }
        if show_elo {
            table = table.column(Column::exact(56.0));
        }
        if show_year {
            table = table.column(Column::exact(44.0));
        }
        if show_players {
            table = table.column(Column::remainder().at_least(80.0).clip(true));
        }
        let selected = self
            .selected
            .as_ref()
            .filter(|(p, k, _)| p == path && *k == pos_key)
            .map(|(_, _, m)| *m);
        let mut hovered = None;
        let mut clicked = None;
        let mut play = None;
        table
            .header(24.0, |mut h| {
                let mut head = |text: &str, hint: &str| {
                    h.col(|ui| {
                        ui.label(RichText::new(text).strong()).on_hover_text(hint);
                    });
                };
                head(
                    tr("Move"),
                    tr("Click: show its games and an arrow. Double click: play the move (it is added to the game as a variation if the game has another move; Ctrl+Z undoes it)"),
                );
                if books {
                    head(tr("Chapters"), tr("Chapters that play the move"));
                } else {
                    head(tr("Games"), tr("Games that played the move"));
                }
                head("%", tr("Share of the games in this position"));
                if show_bar {
                    head(tr("Result"), tr("White wins · draws · Black wins"));
                }
                if show_score {
                    head(tr("White"), tr("White's score"));
                }
                if show_elo {
                    head(tr("Avg Elo"), tr("Average rating, games where both are known"));
                }
                if show_year {
                    head(tr("Year"), tr("The latest game"));
                }
                if show_players {
                    head(tr("Best players"), tr("The strongest players who played the move"));
                }
            })
            .body(|body| {
                body.rows(24.0, rows, |mut row| {
                    let i = row.index();
                    let stat: Option<&MoveStat> = out.moves.get(i);
                    let tally: &Tally = stat.map_or(&out.ended, |s| &s.tally);
                    row.set_selected(stat.is_some_and(|s| Some(s.uci) == selected));
                    let in_game = stat.is_some_and(|s| Some(s.uci) == played);
                    let label = match stat {
                        Some(s) => s
                            .uci
                            .to_move(pos)
                            .map(|m| move_label(pos, m))
                            .unwrap_or_else(|_| crate::san::show(&s.san).into_owned()),
                        // In the start position "ended" are games recorded without moves.
                        None if start => tr("Without moves").to_owned(),
                        None => tr("Game over").to_owned(),
                    };
                    row.col(|ui| {
                        let text = if in_game {
                            RichText::new(format!("⏵ {label}")).strong()
                        } else if stat.is_some() {
                            RichText::new(format!("  {label}"))
                        } else {
                            RichText::new(format!("  {label}")).weak()
                        };
                        let r = ui.add(egui::Label::new(text).selectable(false));
                        if in_game {
                            r.on_hover_text(tr("The move played in this game"));
                        } else if stat.is_none() && start {
                            r.on_hover_text(tr(
                                "Games of the database with only the players and the result, no moves",
                            ));
                        } else if stat.is_none() {
                            r.on_hover_text(tr(
                                "Games that ended in this position (resignation, draw, mate, time)",
                            ));
                        }
                    });
                    row.col(|ui| {
                        ui.label(grouped(tally.games as usize));
                    });
                    row.col(|ui| {
                        let pct = 100.0 * tally.games as f32 / total.max(1) as f32;
                        ui.label(RichText::new(format!("{pct:.0}%")).weak());
                    });
                    if show_bar {
                        row.col(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(78.0, 14.0), Sense::hover());
                            board_search::results_bar(ui, rect, tally, ui.visuals().text_color());
                        });
                    }
                    if show_score {
                        row.col(|ui| {
                            let decided = tally.white_wins + tally.draws + tally.black_wins;
                            if decided > 0 {
                                let score = (tally.white_wins as f32 + tally.draws as f32 / 2.0)
                                    / decided as f32;
                                ui.label(format!("{:.0}%", 100.0 * score));
                            }
                        });
                    }
                    if show_elo {
                        row.col(|ui| {
                            if let Some(avg) = tally.average_elo() {
                                ui.label(avg.to_string());
                            }
                        });
                    }
                    if show_year {
                        row.col(|ui| {
                            if tally.last_year > 0 {
                                ui.label(RichText::new(tally.last_year.to_string()).weak());
                            }
                        });
                    }
                    if show_players {
                        row.col(|ui| {
                            if let Some(p) = stat.and_then(|s| found.players.get(&s.uci)) {
                                ui.label(RichText::new(p).weak());
                            }
                        });
                    }
                    let r = row.response();
                    if let Some(s) = stat {
                        if r.hovered() {
                            hovered = Some(s.uci);
                        }
                        if r.double_clicked() {
                            play = Some(s.uci);
                        } else if r.clicked() {
                            clicked = Some(s.uci);
                        }
                    }
                });
            });
        self.hover = hovered;
        if let Some(m) = clicked {
            self.selected = if selected == Some(m) {
                None
            } else {
                Some((path.to_owned(), pos_key, m))
            };
        }
        if let Some(m) = play.and_then(|u| u.to_move(pos).ok()) {
            self.selected = None;
            action = Some(RefAction::Play(m));
        }
        action
    }

    /// The strongest (or newest) games that reached the position, of the selected move
    /// if one is; without this game and its copies.
    fn games_table(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        pos: &Chess,
        found: &Found,
        copies: &[usize],
    ) -> Option<RefAction> {
        let selected = self
            .selected
            .as_ref()
            .filter(|(p, _, _)| p == path)
            .map(|(_, _, m)| *m);
        let is_copy = |i: usize| copies.binary_search(&i).is_ok();
        // The games counted, without this game and its copies.
        let (list, count): (Vec<&GameRow>, usize) = match selected {
            Some(m) => {
                let lists = match self.sort {
                    GameSort::Strongest => &found.by_move,
                    GameSort::Newest => &found.by_move_newest,
                };
                let rows: Vec<&GameRow> = lists
                    .get(&m)
                    .map(|v| v.iter().collect())
                    .unwrap_or_default();
                let n = found
                    .out
                    .moves
                    .iter()
                    .find(|s| s.uci == m)
                    .map_or(0, |s| s.tally.games as usize);
                let own = found
                    .out
                    .matches
                    .iter()
                    .filter(|x| x.next == Some(m) && is_copy(x.index as usize))
                    .count();
                (rows, n.saturating_sub(own))
            }
            None => {
                let rows = match self.sort {
                    GameSort::Strongest => &found.strongest,
                    GameSort::Newest => &found.newest,
                };
                let own = found
                    .out
                    .matches
                    .iter()
                    .filter(|x| is_copy(x.index as usize))
                    .count();
                (
                    rows.iter().collect(),
                    found.out.matches.len().saturating_sub(own),
                )
            }
        };
        let rows: Vec<&GameRow> = list.into_iter().filter(|g| !is_copy(g.index)).collect();
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            match selected.and_then(|m| m.to_move(pos).ok()) {
                Some(m) => {
                    ui.label(
                        RichText::new(trf("Games with {m}", &[("m", &move_label(pos, m))]))
                            .strong(),
                    );
                    ui.label(RichText::new(format!("· {}", crate::i18n::games(count))).weak());
                    if ui
                        .small_button("🗙")
                        .on_hover_text(tr("All moves"))
                        .clicked()
                    {
                        self.selected = None;
                    }
                }
                None if count == 0 => {
                    ui.label(RichText::new(tr("No other games reached this position")).weak());
                }
                None => {
                    ui.label(RichText::new(crate::i18n::games(count)).strong());
                }
            }
            if count > rows.len() && !rows.is_empty() {
                let text = match self.sort {
                    GameSort::Strongest => trf("· the {n} strongest shown", &[("n", &rows.len())]),
                    GameSort::Newest => trf("· the {n} newest shown", &[("n", &rows.len())]),
                };
                ui.label(RichText::new(text).weak());
            }
            if ui
                .link(tr("all in the library ➡"))
                .on_hover_text(tr(
                    "Open this position on the search board of this database",
                ))
                .clicked()
            {
                action = Some(RefAction::FindInLibrary(path.to_owned()));
            }
        });
        if rows.is_empty() {
            return action;
        }
        let width = ui.available_width();
        let show_event = width > 720.0;
        let show_then = width > 520.0;
        let show_year = width > 380.0;
        // The table never gets wider than the panel: wider content pushes the whole side
        // panel out to the left, under the board (a 1440-point-wide laptop screen).
        let columns = 5 + usize::from(show_year) + usize::from(show_then) + usize::from(show_event);
        let fixed = 3.0 * 44.0
            + if show_year { 52.0 } else { 0.0 }
            + match (show_then, show_event) {
                (true, true) => 150.0,
                (true, false) => 80.0,
                _ => 0.0,
            }
            + if show_event { 80.0 } else { 0.0 }
            + ui.spacing().item_spacing.x * columns as f32;
        let player = ((width - fixed) / 2.0).clamp(60.0, 150.0);
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = TableBuilder::new(ui)
            .id_salt("reference_games")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(player).clip(true))
            .column(Column::exact(44.0))
            .column(Column::exact(player).clip(true))
            .column(Column::exact(44.0))
            .column(Column::exact(44.0));
        if show_year {
            table = table.column(Column::exact(52.0));
        }
        if show_then && show_event {
            table = table.column(Column::exact(150.0).clip(true));
        } else if show_then {
            table = table.column(Column::remainder().at_least(80.0).clip(true));
        }
        if show_event {
            table = table.column(Column::remainder().at_least(80.0).clip(true));
        }
        let mut hovered = None;
        let mut opened = None;
        let mut sort_click = None;
        let sort = self.sort;
        table
            .header(24.0, |mut h| {
                h.col(|ui| {
                    ui.label(RichText::new(tr("White")).strong());
                });
                h.col(|ui| {
                    let arrow = if sort == GameSort::Strongest {
                        " ⏷"
                    } else {
                        ""
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(format!("Elo{arrow}")).strong())
                                .frame(false),
                        )
                        .on_hover_text(tr("Strongest first"))
                        .clicked()
                    {
                        sort_click = Some(GameSort::Strongest);
                    }
                });
                h.col(|ui| {
                    ui.label(RichText::new(tr("Black")).strong());
                });
                h.col(|ui| {
                    ui.label(RichText::new("Elo").strong());
                });
                h.col(|ui| {
                    ui.label(RichText::new(tr("Res.")).strong());
                });
                if show_year {
                    h.col(|ui| {
                        let arrow = if sort == GameSort::Newest { " ⏷" } else { "" };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("{}{arrow}", tr("Year"))).strong(),
                                )
                                .frame(false),
                            )
                            .on_hover_text(tr("Newest first"))
                            .clicked()
                        {
                            sort_click = Some(GameSort::Newest);
                        }
                    });
                }
                if show_then {
                    h.col(|ui| {
                        ui.label(RichText::new(tr("Then")).strong())
                            .on_hover_text(tr("How the game went on from this position"));
                    });
                }
                if show_event {
                    h.col(|ui| {
                        ui.label(RichText::new(tr("Event")).strong());
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, rows.len(), |mut row| {
                    let g = rows[row.index()];
                    let elo = |e: u16| if e > 0 { e.to_string() } else { String::new() };
                    row.col(|ui| {
                        ui.label(&g.white);
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(elo(g.white_elo)).weak());
                    });
                    row.col(|ui| {
                        ui.label(&g.black);
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(elo(g.black_elo)).weak());
                    });
                    row.col(|ui| {
                        ui.label(if g.result == "1/2-1/2" {
                            "½-½"
                        } else {
                            g.result.as_str()
                        });
                    });
                    if show_year {
                        row.col(|ui| {
                            if g.year > 0 {
                                ui.label(RichText::new(g.year.to_string()).weak());
                            }
                        });
                    }
                    if show_then {
                        row.col(|ui| {
                            ui.label(crate::san::show_line(&g.then));
                        });
                    }
                    if show_event {
                        row.col(|ui| {
                            ui.label(RichText::new(&g.event).italics().weak());
                        });
                    }
                    let r = row
                        .response()
                        .on_hover_text(tr("Double click: open this game at this position"));
                    if r.hovered() {
                        hovered = g.next;
                    }
                    if r.double_clicked() {
                        opened = Some(g.index);
                    }
                });
            });
        if let Some(s) = sort_click {
            self.sort = s;
        }
        if hovered.is_some() {
            self.hover = hovered;
        }
        if let Some(i) = opened {
            action = Some(RefAction::OpenGame(path.to_owned(), i, pos.clone()));
        }
        action
    }
}

/// A gap between items of a wrapping row, only where the row goes on: at the end of a
/// full row a plain `add_space` does not wrap but sticks out of the panel.
fn row_gap(ui: &mut Ui, gap: f32) {
    if ui.available_size_before_wrap().x > gap + ui.spacing().item_spacing.x {
        ui.add_space(gap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surnames() {
        assert_eq!(surname("Carlsen, Magnus"), "Carlsen");
        assert_eq!(surname("Alekhine Alexander (RUS)"), "Alekhine");
        assert_eq!(surname("Kasparov"), "Kasparov");
        assert_eq!(
            surname("Duke Karl / Count Isouard"),
            surname("Duke of Brunswick, Count")
        );
        assert_eq!(surname("?"), "");
        assert_eq!(surname("NN"), "");
    }

    #[test]
    fn best_of_many() {
        let v: Vec<u32> = (0..1000).collect();
        let top = best(v, 5, |x| (*x % 97, *x));
        assert_eq!(top.len(), 5);
        assert!(top.iter().all(|x| x % 97 == 96));
        assert!(top.windows(2).all(|w| w[0] > w[1]));
    }

    fn classics() -> Arc<Database> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/classics.pgn");
        let db = chess_core::PgnDatabase::open(&path, &mut |_| {}, &AtomicBool::new(false))
            .expect("opens");
        Arc::new(Database::Pgn(chess_import::PgnDb::new(db)))
    }

    /// The tables of a small database: every listed game reaches the position, the
    /// games of a move played it, strongest first, and "then" starts with that move.
    #[test]
    fn tables_of_the_classics() {
        let db = classics();
        let mut pos = Chess::default();
        let e4: Move = "e2e4".parse::<UciMove>().unwrap().to_move(&pos).unwrap();
        pos.play_unchecked(e4);
        let q = PositionQuery::new(pos.clone(), Some(vec![e4]));
        let out = db
            .search_position(None, &q, &Default::default(), &Default::default())
            .expect("searches");
        let n = out.matches.len();
        let found = tables(&db, &pos, out, &AtomicBool::new(false)).expect("not cancelled");
        assert_eq!(found.strongest.len(), n.min(GAMES_SHOWN));
        for w in found.strongest.windows(2) {
            assert!((w[0].average(), w[0].year) >= (w[1].average(), w[1].year));
        }
        for w in found.newest.windows(2) {
            assert!(w[0].year >= w[1].year);
        }
        for (m, rows) in &found.by_move {
            let first = move_label(&pos, m.to_move(&pos).unwrap());
            let stat = found.out.moves.iter().find(|s| s.uci == *m).unwrap();
            assert_eq!(rows.len(), (stat.tally.games as usize).min(GAMES_PER_MOVE));
            for r in rows {
                assert_eq!(r.next, Some(*m));
                assert!(r.then.starts_with(&first), "{} vs {first}", r.then);
            }
        }
    }

    /// Theory and copies: a game of the database against the same database.
    #[test]
    fn book_of_a_game_of_the_database() {
        let db = classics();
        let parsed = db.load_game(0).expect("game");
        let never = AtomicBool::new(false);
        // The game itself excluded, nothing else is the same game.
        let end = book_end(&db, &parsed.game, Some(0), 0, &never).expect("theory");
        assert_eq!(end.copies, vec![0]);
        assert!(!end.to_the_end);
        // Not excluded: the game is its own copy (same players, year, final position).
        let end = book_end(&db, &parsed.game, None, 0, &never).expect("theory");
        assert_eq!(end.copies, vec![0]);
        // An empty game has no theory and no copies.
        let empty = Game::new();
        let end = book_end(&db, &empty, None, 0, &never).expect("theory");
        assert!(end.copies.is_empty() && !end.to_the_end);
    }
}
