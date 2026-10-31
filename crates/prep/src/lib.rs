//! Preparing against an opponent, shared by the desktop and the Android app: what a
//! player of a database plays in the opening and where in the opening he goes wrong.
//!
//! The [`PrepReport`] is built in a moment from the player's games ([`build`]): his
//! main lines up to a number of plies, the repertoire tree of each colour with
//! transpositions marked, the openings named from the Lichess `chess-openings` table
//! (family → variation → games), and, with a second database as the book, where each
//! game left it. The engine step is apart and optional: every distinct position of
//! the lines is evaluated once ([`PrepReport::targets`], run by
//! [`game_review::run`], results taken in by [`PrepReport::apply`]), the player's
//! moves are judged with the rules of the game analysis, and the errors are listed by
//! opening and, when the same error was made in several games, as repeated. The
//! report is cached by database, player and plies; evaluations by position, across
//! reports ([`PositionCache`]).
//!
//! Nothing here touches the database or the games; no user interface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::SystemTime;

use chess_import::Database;
use game_review::{JobEvent, SEARCH_NODES, SEARCH_VERSION, Target};
#[cfg(not(target_family = "wasm"))]
use game_review::{JobFailure, JobSettings};
use serde::{Deserialize, Serialize};
use uci::EnginePosition;

mod book;
mod errors;
mod games;
pub mod openings;
mod period;
mod players;
mod tree;

pub use book::BookOptions;
pub use chess_import::chessbase::base::Side;
pub use errors::{EXACT, ErrorCounts, OpeningError, PositionCache, PositionEval, RepeatedError};
pub use game_review::{Eval, Judgement};
pub use games::{BookExit, GameInfo, Outcome, Tally, move_label};
pub use openings::Opening;
pub use period::{Chosen, Period, PoolGame, newest as period_newest};
pub use players::{Candidate, Player, candidates, games_of};
pub use tree::{Node, Tree};

/// Plies of the opening a report looks at unless told otherwise.
pub const DEFAULT_PLIES: usize = 20;
/// A position with fewer games than this in the book is out of it.
pub const DEFAULT_BOOK_THRESHOLD: u32 = 5;

/// What a report covers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    /// Plies of each game's main line.
    pub plies: usize,
    /// The player's colour(s).
    pub side: Side,
    /// Which of his games.
    pub period: Period,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            plies: DEFAULT_PLIES,
            side: Side::Any,
            period: Period::All,
        }
    }
}

/// An opening as played by the player: the games with the same name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Variation {
    /// Index into [`PrepReport::openings`]; `None`: no named position in the line.
    pub opening: Option<usize>,
    pub tally: Tally,
    /// Indices into [`PrepReport::games`], in order.
    pub games: Vec<u32>,
    pub errors: ErrorCounts,
    /// Indices into [`ColorReport::errors`].
    pub error_list: Vec<u32>,
}

/// The openings with the same family name ("Sicilian Defense"), most played first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Family {
    /// Empty for games without a named position.
    pub name: String,
    pub tally: Tally,
    /// Most played first.
    pub variations: Vec<Variation>,
    pub errors: ErrorCounts,
}

/// The report for one colour of the player.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorReport {
    pub white: bool,
    /// Indices into [`PrepReport::games`], in record order.
    pub games: Vec<u32>,
    pub tree: Tree,
    pub families: Vec<Family>,
    /// The player's opening errors, by game and ply (after the engine step).
    pub errors: Vec<OpeningError>,
    /// The errors made in two or more games.
    pub repeated: Vec<RepeatedError>,
    pub counts: ErrorCounts,
}

impl ColorReport {
    /// Games that left the book, by whose move: (the player's, the opponent's);
    /// the rest stayed in the book for the whole line (or no book was looked at).
    pub fn book_exits(&self, games: &[GameInfo]) -> (u32, u32) {
        let mut out = (0, 0);
        for &g in &self.games {
            match games[g as usize].book {
                Some(b) if b.by_player => out.0 += 1,
                Some(_) => out.1 += 1,
                None => {}
            }
        }
        out
    }
}

/// The engine the evaluations came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    /// Nodes per position.
    pub nodes: u64,
}

/// The book the exits were found against.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BookInfo {
    pub fingerprint: u64,
    pub threshold: u32,
    /// Games that left the book within their line.
    pub left: u32,
}

// 4: the period and the pool of the player's games.
const REPORT_VERSION: u32 = 4;

/// Everything the preparation knows about the player.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepReport {
    pub version: u32,
    /// Of the database ([`fingerprint`]).
    pub fingerprint: u64,
    pub player: Player,
    /// The player's names in the database.
    pub names: Vec<String>,
    pub plies: usize,
    pub side_white: bool,
    pub side_black: bool,
    /// Which of the player's games the report took.
    #[serde(default)]
    pub period: Period,
    /// All the player's games of the side(s), before the period's choice (for the
    /// counts of the other periods).
    #[serde(default)]
    pub pool: Vec<PoolGame>,
    pub games: Vec<GameInfo>,
    /// Games left out: from a set-up position, or unreadable.
    pub skipped: u32,
    /// The openings the games and trees refer to.
    pub openings: Vec<Opening>,
    pub white: ColorReport,
    pub black: ColorReport,
    pub book: Option<BookInfo>,
    /// The book's number of games in every position the book step looked at (by
    /// key): the first plies of every line, then the positions around each exit.
    #[serde(default)]
    pub book_counts: HashMap<u64, u32>,
    pub engine: Option<EngineInfo>,
    /// Engine evaluations by position key.
    pub evals: HashMap<u64, PositionEval>,
    /// The version of the fixed search of the game analysis the evaluations come from
    /// ([`SEARCH_VERSION`]; 0: from before it, made with the user's engine settings).
    #[serde(default)]
    pub search: u32,
}

/// Identity of a database for the report cache: the content of a PGN file, the
/// path, size and time of a ChessBase database.
pub fn fingerprint(db: &Database) -> u64 {
    match db {
        Database::Pgn(p) => {
            let f = p.fingerprint();
            fnv(f
                .content_hash
                .to_le_bytes()
                .iter()
                .chain(f.len.to_le_bytes().iter()))
        }
        Database::ChessBase(b) => {
            let meta = std::fs::metadata(b.path()).ok();
            let len = meta.as_ref().map_or(0, std::fs::Metadata::len);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            fnv(b
                .path()
                .to_string_lossy()
                .bytes()
                .chain(len.to_le_bytes())
                .chain(modified.to_le_bytes())
                .collect::<Vec<u8>>()
                .iter())
        }
    }
}

fn fnv<'a>(bytes: impl Iterator<Item = &'a u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Builds the report of `player` in `db`: his games ([`games_of`]), their first
/// plies, the trees, the names and the opening tables. `progress` counts the games
/// read. `None` when cancelled. Milliseconds for hundreds of games.
pub fn build(
    db: &Database,
    player: &Player,
    options: &Options,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<PrepReport> {
    let found = games_of(db, player, options.side, cancel)?;
    // Every game is read (its first plies: a moment), so that the pool and the counts of
    // the periods are of the games a report can take: not those from a set-up position.
    let collected = games::collect(db, &found, options.plies, progress, cancel)?;
    let pool: Vec<PoolGame> = collected
        .games
        .iter()
        .map(|g| PoolGame {
            index: g.index,
            white: g.white,
            date: g.date_number(),
        })
        .collect();
    let chosen: std::collections::HashSet<(u32, bool)> =
        period::choose(&pool, options.period).into_iter().collect();
    let mut openings = collected.openings;
    let games: Vec<GameInfo> = collected
        .games
        .into_iter()
        .filter(|g| chosen.contains(&(g.index, g.white)))
        .collect();
    let mut evals = HashMap::new();
    for (key, pos) in &collected.terminal {
        if let Some(eval) = game_review::terminal_eval(pos) {
            evals.insert(
                *key,
                PositionEval {
                    nodes: EXACT,
                    eval,
                    pv: Vec::new(),
                    depth: 0,
                },
            );
        }
    }
    let color = |white: bool, openings: &mut openings::OpeningSet| -> ColorReport {
        let which: Vec<u32> = games
            .iter()
            .enumerate()
            .filter(|(_, g)| g.white == white)
            .map(|(i, _)| i as u32)
            .collect();
        let tree = Tree::build(&games, &which, options.plies, openings);
        let families = families(&games, &which, &openings.list);
        ColorReport {
            white,
            games: which,
            tree,
            families,
            errors: Vec::new(),
            repeated: Vec::new(),
            counts: ErrorCounts::default(),
        }
    };
    let white = color(true, &mut openings);
    let black = color(false, &mut openings);
    let mut report = PrepReport {
        version: REPORT_VERSION,
        fingerprint: fingerprint(db),
        player: player.clone(),
        names: player.names(db),
        plies: options.plies,
        side_white: options.side != Side::Black,
        side_black: options.side != Side::White,
        period: options.period,
        pool,
        games,
        skipped: collected.skipped,
        openings: openings.list,
        white,
        black,
        book: None,
        book_counts: HashMap::new(),
        engine: None,
        evals,
        search: SEARCH_VERSION,
    };
    report.refresh();
    Some(report)
}

/// Groups `games[g]` for `g` in `which` by opening name: families, most played
/// first, each with its variations, most played first. A name the table gives to
/// several positions ("Sicilian Defense: Closed" after 2.Nc3 and after 2...e6) is one
/// variation, shown with its shortest line.
fn families(games: &[GameInfo], which: &[u32], openings: &[Opening]) -> Vec<Family> {
    let mut out: Vec<Family> = Vec::new();
    let mut family_of: HashMap<String, usize> = HashMap::new();
    let mut variation_of: HashMap<(usize, String), usize> = HashMap::new();
    for &g in which {
        let game = &games[g as usize];
        let opening = game.opening.map(|o| &openings[o]);
        let name = opening.map(|o| o.family().to_owned()).unwrap_or_default();
        let f = *family_of.entry(name.clone()).or_insert_with(|| {
            out.push(Family {
                name,
                ..Family::default()
            });
            out.len() - 1
        });
        let full = opening.map(|o| o.name.clone()).unwrap_or_default();
        let v = *variation_of.entry((f, full)).or_insert_with(|| {
            out[f].variations.push(Variation {
                opening: game.opening,
                ..Variation::default()
            });
            out[f].variations.len() - 1
        });
        out[f].tally.add(game.outcome);
        let var = &mut out[f].variations[v];
        if let (Some(o), Some(shown)) = (game.opening, var.opening)
            && openings[o].plies < openings[shown].plies
        {
            var.opening = Some(o);
        }
        var.tally.add(game.outcome);
        var.games.push(g);
    }
    let variation_name = |v: &Variation| -> String {
        v.opening
            .map(|o| openings[o].name.clone())
            .unwrap_or_default()
    };
    for f in &mut out {
        f.variations.sort_by(|a, b| {
            b.tally
                .games
                .cmp(&a.tally.games)
                .then_with(|| variation_name(a).cmp(&variation_name(b)))
        });
    }
    out.sort_by(|a, b| {
        b.tally
            .games
            .cmp(&a.tally.games)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

impl PrepReport {
    /// The colour(s) the report was built for.
    pub fn side(&self) -> Side {
        match (self.side_white, self.side_black) {
            (true, false) => Side::White,
            (false, true) => Side::Black,
            _ => Side::Any,
        }
    }

    /// The options the report was built with.
    pub fn options(&self) -> Options {
        Options {
            plies: self.plies,
            side: self.side(),
            period: self.period,
        }
    }

    /// How many games `period` would take of the colour(s) `side`.
    pub fn count(&self, period: Period, side: Side) -> Chosen {
        period::count(&self.pool, period, side)
    }

    pub fn color(&self, white: bool) -> &ColorReport {
        if white { &self.white } else { &self.black }
    }

    // ----- the book -------------------------------------------------------------

    /// Finds where each game left `book` (see [`book`]); `progress` counts the games
    /// done. `None` when cancelled or the book cannot be searched; the report is
    /// then left as it was.
    pub fn find_book_exits(
        &mut self,
        book: &Database,
        options: &BookOptions,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<()> {
        let mut games = self.games.clone();
        let (left, counts) = book::find_exits(&mut games, book, options, progress, cancel)?;
        self.games = games;
        self.book = Some(BookInfo {
            fingerprint: fingerprint(book),
            threshold: options.threshold,
            left,
        });
        self.book_counts = counts;
        Some(())
    }

    // ----- the engine -----------------------------------------------------------

    /// Every distinct position of the lines (the one after the last move included),
    /// first met first; those without legal moves left out.
    fn positions(&self) -> Vec<(u64, &GameInfo, usize)> {
        self.positions_for(Side::Any)
    }

    /// [`PrepReport::positions`] of the games the player had `side` in.
    fn positions_for(&self, side: Side) -> Vec<(u64, &GameInfo, usize)> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for g in &self.games {
            if (side == Side::White && !g.white) || (side == Side::Black && g.white) {
                continue;
            }
            for (i, &k) in g.keys.iter().enumerate() {
                if self.evals.get(&k).is_some_and(|e| e.nodes == EXACT) {
                    continue;
                }
                if seen.insert(k) {
                    out.push((k, g, i));
                }
            }
        }
        out
    }

    /// Some position has an engine evaluation (from a run or from the cache): the
    /// errors are judged.
    pub fn evaluated(&self) -> bool {
        self.evals.values().any(|e| e.nodes != EXACT)
    }

    /// Distinct positions an engine run looks at.
    pub fn distinct_positions(&self) -> usize {
        self.positions().len()
    }

    /// Distinct positions of the games the player had `side` in.
    pub fn distinct_positions_for(&self, side: Side) -> usize {
        self.positions_for(side).len()
    }

    /// Positions without an evaluation.
    pub fn missing(&self) -> usize {
        self.missing_for(Side::Any)
    }

    /// [`PrepReport::missing`] among the games the player had `side` in.
    pub fn missing_for(&self, side: Side) -> usize {
        self.positions_for(side)
            .iter()
            .filter(|(k, _, _)| !self.evals.contains_key(k))
            .count()
    }

    /// Takes from `cache` the evaluations the report lacks; returns how many.
    pub fn take_cached(&mut self, cache: &PositionCache) -> usize {
        let keys: Vec<u64> = self.positions().iter().map(|(k, _, _)| *k).collect();
        let mut taken = 0;
        for k in keys {
            if self.evals.contains_key(&k) {
                continue;
            }
            if let Some(e) = cache.get(k) {
                self.evals.insert(k, e.clone());
                taken += 1;
            }
        }
        if taken > 0 {
            self.refresh();
        }
        taken
    }

    /// The positions still to evaluate, for [`game_review::run`] (one search each, no
    /// second search for !! and !). Their keys are the position keys in hexadecimal, as
    /// [`PrepReport::apply`] reads them.
    pub fn targets(&self) -> Vec<Target> {
        self.targets_for(Side::Any)
    }

    /// [`PrepReport::targets`] of the games the player had `side` in (the dialog's
    /// «Black — 133 games» halves the work).
    pub fn targets_for(&self, side: Side) -> Vec<Target> {
        self.positions_for(side)
            .into_iter()
            .filter(|(k, _, _)| !self.evals.contains_key(k))
            .map(|(k, g, i)| Target {
                key: format!("{k:016x}"),
                pos: EnginePosition::startpos(g.ucis[..i].iter().cloned()),
                turn: if i.is_multiple_of(2) {
                    uci::Side::White
                } else {
                    uci::Side::Black
                },
                played: None,
                others: Vec::new(),
                known: None,
                shallow: None,
            })
            .collect()
    }

    /// Takes in what the engine job reported; the evaluation it carried (to cache), if
    /// any. The errors are re-judged on the next [`PrepReport::refresh`].
    pub fn apply(&mut self, event: &JobEvent) -> Option<(u64, PositionEval)> {
        let nodes = SEARCH_NODES;
        match event {
            JobEvent::Engine(name) => {
                self.engine = Some(EngineInfo {
                    name: name.clone(),
                    nodes,
                });
                None
            }
            JobEvent::Eval { key, eval, .. } => {
                let k = u64::from_str_radix(key, 16).ok()?;
                let e = PositionEval::from_engine(nodes, eval);
                self.engine.get_or_insert_with(|| EngineInfo {
                    name: String::new(),
                    nodes,
                });
                self.evals.insert(k, e.clone());
                Some((k, e))
            }
        }
    }

    /// Re-judges the player's moves from the evaluations: the errors by colour,
    /// opening and repetition. Cheap (the lines of the games).
    pub fn refresh(&mut self) {
        let PrepReport {
            games,
            evals,
            white,
            black,
            ..
        } = self;
        for cr in [white, black] {
            let errors = errors::judge_games(games, &cr.games, evals);
            cr.repeated = errors::repeated(&errors, games);
            cr.counts = ErrorCounts::default();
            let mut at: HashMap<u32, (usize, usize)> = HashMap::new();
            for (f, fam) in cr.families.iter_mut().enumerate() {
                fam.errors = ErrorCounts::default();
                for (v, var) in fam.variations.iter_mut().enumerate() {
                    var.errors = ErrorCounts::default();
                    var.error_list.clear();
                    for &g in &var.games {
                        at.insert(g, (f, v));
                    }
                }
            }
            for (i, e) in errors.iter().enumerate() {
                cr.counts.add(e.judgement);
                if let Some(&(f, v)) = at.get(&e.game) {
                    let fam = &mut cr.families[f];
                    fam.errors.add(e.judgement);
                    fam.variations[v].errors.add(e.judgement);
                    fam.variations[v].error_list.push(i as u32);
                }
            }
            cr.errors = errors;
        }
    }

    // ----- the report cache -----------------------------------------------------

    /// The cache file of a report in `dir`.
    pub fn cache_path(dir: &Path, fingerprint: u64, player: &Player, options: &Options) -> PathBuf {
        let mut bytes = fingerprint.to_le_bytes().to_vec();
        for id in &player.ids {
            bytes.extend(id.to_le_bytes());
        }
        bytes.extend((options.plies as u64).to_le_bytes());
        bytes.push(match options.side {
            Side::Any => 0,
            Side::White => 1,
            Side::Black => 2,
        });
        // All the games: as before periods were chosen (the reports cached then).
        match options.period {
            Period::All => {}
            Period::Months(m) => {
                bytes.push(1);
                bytes.extend(u32::from(m).to_le_bytes());
            }
            Period::Last(n) => {
                bytes.push(2);
                bytes.extend(n.to_le_bytes());
            }
        }
        dir.join(format!("{:016x}.json", fnv(bytes.iter())))
    }

    /// The report cached in `dir` for this database, player and options, if any.
    pub fn load(
        dir: &Path,
        db: &Database,
        player: &Player,
        options: &Options,
    ) -> Option<PrepReport> {
        let fp = fingerprint(db);
        let bytes = std::fs::read(Self::cache_path(dir, fp, player, options)).ok()?;
        let mut report: PrepReport = serde_json::from_slice(&bytes).ok()?;
        let same = report.version == REPORT_VERSION
            && report.fingerprint == fp
            && report.player == *player
            && report.plies == options.plies
            && report.side() == options.side
            && report.period == options.period;
        if !same {
            return None;
        }
        // Evaluations of another search are not mixed with today's: the errors are
        // found again.
        if report.search != SEARCH_VERSION {
            report.evals.retain(|_, e| e.nodes == EXACT);
            report.engine = None;
            report.search = SEARCH_VERSION;
            report.refresh();
        }
        Some(report)
    }

    /// Keeps the report in `dir` (atomically).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let options = self.options();
        let path = Self::cache_path(dir, self.fingerprint, &self.player, &options);
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        errors::write_atomic(&path, &bytes)
    }
}

/// Progress of [`analyse`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Positions evaluated in this run.
    pub done: usize,
    /// Positions the run has to evaluate.
    pub total: usize,
    /// The engine's speed in the last search (0 if it did not say).
    pub nps: u64,
}

// The engines run as processes, each driven from a thread: not in a browser.
#[cfg(not(target_family = "wasm"))]
/// Runs the engine over the positions `report` lacks (the cache
/// is consulted first and fed as results come; `cache_dir` saves it every few
/// hundred positions and at the end), updating the report as results come in.
/// Returns when done, or soon after `cancel` is set (what was evaluated stays).
pub fn analyse(
    report: &mut PrepReport,
    cache: &mut PositionCache,
    cache_dir: Option<&Path>,
    config: uci::EngineConfig,
    settings: &JobSettings,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), JobFailure> {
    report.take_cached(cache);
    let targets = report.targets();
    let total = targets.len();
    let mut progress = Progress {
        done: 0,
        total,
        nps: 0,
    };
    on_progress(progress);
    if total == 0 {
        return Ok(());
    }
    let mut since_save = 0;
    let result = game_review::run(config, settings, targets, cancel, &mut |event| {
        if let JobEvent::Eval { nps, .. } = &event {
            progress.done += 1;
            progress.nps = *nps;
        }
        if let Some((key, eval)) = report.apply(&event) {
            cache.insert(key, eval);
            since_save += 1;
            if since_save >= 200
                && let Some(dir) = cache_dir
            {
                let _ = cache.save(dir);
                since_save = 0;
            }
        }
        if matches!(event, JobEvent::Eval { .. }) {
            on_progress(progress);
        }
    });
    report.refresh();
    if let Some(dir) = cache_dir {
        let _ = cache.save(dir);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::PgnDatabase;
    use chess_import::PgnDb;

    fn database(pgn: &str) -> Database {
        let db = PgnDatabase::from_bytes(
            pgn.as_bytes().to_vec(),
            None,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .expect("pgn");
        Database::Pgn(PgnDb::new(db))
    }

    const PGN: &str = "\
[White \"Вафин Радель\"]
[Black \"Иванов Иван\"]
[Date \"2026.01.10\"]
[BlackElo \"1500\"]
[Result \"1-0\"]

1. e4 c5 2. Nf3 Nc6 3. Bb5 g6 4. O-O Bg7 1-0

[White \"Петров Пётр\"]
[Black \"Вафин Радель\"]
[Date \"2026.02.11\"]
[WhiteElo \"1700\"]
[Result \"0-1\"]

1. e4 c5 2. Nf3 d6 3. d4 cxd4 4. Nxd4 Nf6 5. Nc3 a6 0-1

[White \"Вафин Радель\"]
[Black \"Вафина Раяна\"]
[Date \"2026.03.12\"]
[Result \"1/2-1/2\"]

1. e4 c5 2. Nf3 Nc6 3. Bb5 e6 1/2-1/2

[White \"Вафин Р.\"]
[Black \"Сидоров\"]
[Date \"2026.04.01\"]
[Result \"0-1\"]

1. d4 d5 2. c4 0-1

[White \"Вафин Радель\"]
[Black \"Set Up\"]
[FEN \"rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1\"]
[SetUp \"1\"]
[Result \"*\"]

1... e5 *
";

    #[test]
    fn candidates_games_and_openings_of_a_player() {
        let db = database(PGN);
        // Every name with both words ("Вафина Раяна" has them too): the user picks.
        let (list, total) = candidates(&db, "Вафин Р.", 10);
        assert_eq!(total, 3, "{list:?}");
        assert_eq!(list[0].name, "Вафин Радель");
        assert_eq!(list[0].games, 4);
        assert_eq!(list[1].name, "Вафин Р.");
        assert_eq!(list[2].name, "Вафина Раяна");
        let (one, _) = candidates(&db, "Радель", 10);
        assert_eq!(one.len(), 1);
        // Two spellings merged.
        let player = Player::new([list[0].id, list[1].id]);
        let never = AtomicBool::new(false);
        let found = games_of(&db, &player, Side::Any, &never).expect("games");
        assert_eq!(
            found,
            vec![(0, true), (1, false), (2, true), (3, true), (4, true)]
        );
        assert_eq!(
            games_of(&db, &player, Side::Black, &never).expect("games"),
            vec![(1, false)]
        );
        let report = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        assert_eq!(report.games.len(), 4, "the set-up game is left out");
        assert_eq!(report.skipped, 1);
        assert_eq!(report.names, vec!["Вафин Радель", "Вафин Р."]);
        let g = &report.games[0];
        assert_eq!(
            (g.index, g.white, g.opponent.as_str(), g.opponent_elo),
            (0, true, "Иванов Иван", 1500)
        );
        assert_eq!(g.date, "2026.01.10");
        assert_eq!(g.outcome, Outcome::Win);
        assert_eq!(
            g.sans,
            vec!["e4", "c5", "Nf3", "Nc6", "Bb5", "g6", "O-O", "Bg7"]
        );
        assert_eq!(g.keys.len(), 9);
        let o = &report.openings[g.opening.expect("named")];
        assert!(
            o.name
                .starts_with("Sicilian Defense: Nyezhmetdinov-Rossolimo Attack"),
            "{}",
            o.name
        );
        assert_eq!(g.named, Some(6));
        assert_eq!(report.games[1].outcome, Outcome::Win);
        assert_eq!(report.games[1].opponent_elo, 1700);
        // White: two Rossolimo games and one Queen's Gambit; the family first.
        let w = &report.white;
        assert_eq!(w.games, vec![0, 2, 3]);
        assert_eq!(w.families.len(), 2);
        assert_eq!(w.families[0].name, "Sicilian Defense");
        assert_eq!(w.families[0].tally.games, 3 - 1);
        assert_eq!(
            w.families[0].variations.len(),
            2,
            "3...g6 and 3...e6 are named apart"
        );
        assert_eq!(w.families[1].name, "Queen's Gambit");
        assert_eq!(w.tree.nodes[0].tally.games, 3);
        assert_eq!(w.tree.nodes[0].last_date, "2026.04.01");
        assert_eq!(w.tree.nodes[0].average_elo(), Some(1500));
        let b = &report.black;
        assert_eq!(b.games, vec![1]);
        assert_eq!(b.families[0].name, "Sicilian Defense");
        let name = &report.openings[b.families[0].variations[0].opening.unwrap()].name;
        assert!(name.contains("Najdorf"), "{name}");
        assert!(report.evals.is_empty() && report.engine.is_none());
        assert_eq!(report.side(), Side::Any);
        // Only White.
        let white_only = build(
            &db,
            &player,
            &Options {
                plies: 4,
                side: Side::White,
                period: Period::All,
            },
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        assert_eq!(white_only.games.len(), 3);
        assert!(white_only.black.games.is_empty());
        assert!(white_only.games.iter().all(|g| g.ucis.len() <= 4));
        assert_eq!(white_only.side(), Side::White);
        // The last two months before his newest game (2026.04.01): from 2026.02.01.
        let recent = build(
            &db,
            &player,
            &Options {
                period: Period::Months(2),
                ..Options::default()
            },
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        let dates: Vec<&str> = recent.games.iter().map(|g| g.date.as_str()).collect();
        assert_eq!(dates, ["2026.02.11", "2026.03.12", "2026.04.01"]);
        // The pool: the games a report can take (not the set-up one it skips).
        let all = report.games.len();
        assert_eq!(report.skipped, 1);
        assert_eq!(recent.pool.len(), all);
        assert_eq!(recent.count(Period::All, Side::Any).games, all);
        assert_eq!(recent.count(Period::Months(2), Side::Any).games, 3);
        assert_eq!(recent.count(Period::Last(1), Side::White).games, 1);
        // Cancelled.
        let cancelled = AtomicBool::new(true);
        assert!(
            build(
                &db,
                &player,
                &Options::default(),
                &AtomicUsize::new(0),
                &cancelled
            )
            .is_none()
        );
    }

    #[test]
    fn one_name_for_several_positions_is_one_variation() {
        // "Sicilian Defense: Closed" is the table's name after 2.Nc3 and after 2...e6.
        let db = database(
            "[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 c5 2. Nc3 e6 1-0\n\n\
             [White \"A\"]\n[Black \"C\"]\n[Result \"0-1\"]\n\n1. e4 c5 2. Nc3 0-1\n\n",
        );
        let player = Player::new([candidates(&db, "A", 1).0[0].id]);
        let report = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .expect("report");
        let fam = &report.white.families[0];
        assert_eq!(fam.name, "Sicilian Defense");
        assert_eq!(fam.variations.len(), 1);
        let v = &fam.variations[0];
        assert_eq!(v.games, vec![0, 1]);
        let o = &report.openings[v.opening.unwrap()];
        assert_eq!(
            (o.name.as_str(), o.line.as_str()),
            ("Sicilian Defense: Closed", "1. e4 c5 2. Nc3")
        );
    }

    #[test]
    fn the_book_says_where_a_game_left_it() {
        let db = database(PGN);
        let (list, _) = candidates(&db, "Вафин Р.", 2);
        let player = Player::new([list[0].id, list[1].id]);
        let never = AtomicBool::new(false);
        let mut report = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        // The database itself as the book, threshold 2: 1.e4 c5 2.Nf3 is in three
        // games; game 0 (3...g6) and game 2 (3...e6) part after 3.Bb5, which is in
        // both. `ply` counts the moves to the first rare position.
        let options = BookOptions {
            threshold: 2,
            same_database: false,
        };
        report
            .find_book_exits(&db, &options, &AtomicUsize::new(0), &never)
            .expect("book");
        let exits: Vec<Option<BookExit>> = report.games.iter().map(|g| g.book).collect();
        assert_eq!(
            exits[0],
            Some(BookExit {
                ply: 6,
                by_player: false,
                count: 1
            })
        );
        assert_eq!(
            exits[1].map(|b| (b.ply, b.by_player)),
            Some((4, true)),
            "2...d6 is his"
        );
        assert_eq!(exits[2].map(|b| (b.ply, b.by_player)), Some((6, false)));
        assert_eq!(
            exits[3].map(|b| (b.ply, b.by_player)),
            Some((1, true)),
            "1.d4 is his own"
        );
        assert_eq!(
            report.book.as_ref().map(|b| (b.threshold, b.left)),
            Some((2, 4))
        );
        assert_eq!(report.white.book_exits(&report.games), (1, 2));
        // The counts of the positions looked at: the start (4 games), 1.e4 c5 2.Nf3
        // (3), the rare 3...g6 (1).
        let g = &report.games[0];
        assert_eq!(report.book_counts.get(&g.keys[0]), Some(&4));
        assert_eq!(report.book_counts.get(&g.keys[3]), Some(&3));
        assert_eq!(report.book_counts.get(&g.keys[6]), Some(&1));
        // The player's own games excluded: everything is out of the book at once.
        let own = BookOptions {
            threshold: 1,
            same_database: true,
        };
        report
            .find_book_exits(&db, &own, &AtomicUsize::new(0), &never)
            .expect("book");
        assert_eq!(report.games[0].book.map(|b| b.ply), Some(0));
        assert_eq!(report.book.as_ref().map(|b| b.left), Some(4));
    }

    #[test]
    fn evaluations_give_the_errors_by_opening_and_the_report_is_cached() {
        let db = database(PGN);
        let (list, _) = candidates(&db, "Вафин Радель", 1);
        let player = Player::new([list[0].id]);
        let never = AtomicBool::new(false);
        let mut report = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        let total = report.distinct_positions();
        // 9 + 11 + 7 positions; the start and 1.e4 c5 2.Nf3 shared by three games,
        // 2...Nc6 3.Bb5 by two.
        assert_eq!(total, 27 - 10);
        let targets = report.targets();
        assert_eq!(targets.len(), total);
        // One colour: the positions of its games only (the start position is in both).
        let black = report.targets_for(Side::Black);
        assert_eq!(black.len(), report.games[1].keys.len());
        assert_eq!(report.distinct_positions_for(Side::Black), black.len());
        assert!(report.distinct_positions_for(Side::White) < total);
        assert_eq!(report.missing_for(Side::Black), black.len());
        assert_eq!(targets[0].key, format!("{:016x}", report.games[0].keys[0]));
        assert!(targets[0].pos.moves.is_empty());
        assert_eq!(targets[3].pos.moves, vec!["e2e4", "c7c5", "g1f3"]);
        assert_eq!(targets[3].turn, uci::Side::Black);
        assert!(
            targets
                .iter()
                .all(|t| t.played.is_none() && t.known.is_none())
        );
        // The engine says: every move fine but 3.Bb5, a blunder (+0.3 → −1.5).
        let mut cache = PositionCache::default();
        for t in &targets {
            let key = u64::from_str_radix(&t.key, 16).unwrap();
            let after_bb5 = report.games[0].keys[5];
            let eval = if key == after_bb5 {
                Eval::Cp(-150)
            } else {
                Eval::Cp(30)
            };
            let before_bb5 = report.games[0].keys[4];
            let pv = if key == before_bb5 {
                vec!["d2d4".to_owned()]
            } else {
                Vec::new()
            };
            let event = JobEvent::Eval {
                key: t.key.clone(),
                eval: game_review::PosEval {
                    eval,
                    pv,
                    depth: 12,
                    second: None,
                    shallow: None,
                },
                nps: 0,
            };
            let (k, e) = report.apply(&event).expect("an evaluation");
            cache.insert(k, e);
        }
        assert_eq!(report.missing(), 0);
        assert!(report.evaluated());
        report.refresh();
        assert_eq!(report.engine.as_ref().map(|e| e.nodes), Some(SEARCH_NODES));
        let w = &report.white;
        assert_eq!(w.counts.blunders, 2);
        assert_eq!(w.errors.len(), 2);
        assert_eq!(w.errors[0].san, "Bb5");
        assert_eq!(w.errors[0].best_san, "d4");
        assert_eq!(w.errors[0].ply, 4);
        assert_eq!(
            report.games[w.errors[0].game as usize].move_label(4),
            "3. Bb5"
        );
        assert_eq!(w.families[0].errors.blunders, 2);
        assert_eq!(w.families[0].variations[0].errors.blunders, 1);
        assert_eq!(w.families[0].variations[0].error_list.len(), 1);
        assert_eq!(w.families.len(), 1);
        assert_eq!(w.repeated.len(), 1);
        assert_eq!(w.repeated[0].games, vec![0, 2]);
        assert_eq!(w.repeated[0].tally.draws, 1);
        assert!(report.black.errors.is_empty());
        // Cached: the report by database, player and options; the evaluations by
        // position, for anyone.
        let dir = std::env::temp_dir().join(format!("prep-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(PrepReport::load(&dir, &db, &player, &Options::default()).is_none());
        report.save(&dir).expect("saved");
        let again = PrepReport::load(&dir, &db, &player, &Options::default()).expect("cached");
        assert_eq!(again.white.errors, report.white.errors);
        assert_eq!(again.evals.len(), report.evals.len());
        assert!(
            PrepReport::load(
                &dir,
                &db,
                &player,
                &Options {
                    plies: 10,
                    side: Side::Any,
                    period: Period::All,
                }
            )
            .is_none()
        );
        let other = Player::new([list[0].id + 1]);
        assert!(PrepReport::load(&dir, &db, &other, &Options::default()).is_none());
        let mut fresh = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        assert!(!fresh.evaluated());
        assert_eq!(fresh.take_cached(&cache), total);
        assert!(
            fresh.evaluated(),
            "evaluations from the cache judge the errors too"
        );
        assert_eq!(fresh.white.repeated, report.white.repeated);
        assert_eq!(fresh.take_cached(&cache), 0);
        // A report of another search (from before the fixed one) keeps its games but
        // not its evaluations.
        let path = PrepReport::cache_path(&dir, report.fingerprint, &player, &Options::default());
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("report")).expect("json");
        json.as_object_mut().expect("report").remove("search");
        std::fs::write(&path, serde_json::to_vec(&json).expect("json")).expect("written");
        let old = PrepReport::load(&dir, &db, &player, &Options::default()).expect("still read");
        assert!(!old.evaluated() && old.white.errors.is_empty());
        assert_eq!(old.missing(), total);
        assert_eq!(old.search, SEARCH_VERSION);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The fake engine of the `uci` crate (`cargo build -p uci --bin fake_uci`).
    #[test]
    fn the_engine_run_fills_the_report_and_the_cache() {
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/debug/fake_uci");
        assert!(
            std::path::Path::new(fake).exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        let db = database(PGN);
        let (list, _) = candidates(&db, "Вафин Радель", 1);
        let player = Player::new([list[0].id]);
        let never = AtomicBool::new(false);
        let mut report = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        let total = report.distinct_positions();
        let settings = JobSettings::new(2, None);
        let mut cache = PositionCache::default();
        let mut last = Progress::default();
        analyse(
            &mut report,
            &mut cache,
            None,
            uci::EngineConfig::new(fake),
            &settings,
            &never,
            &mut |p| last = p,
        )
        .expect("the job runs");
        assert_eq!((last.done, last.total), (total, total));
        assert_eq!(report.missing(), 0);
        assert_eq!(cache.len(), total);
        assert_eq!(
            report.engine.as_ref().map(|e| e.name.as_str()),
            Some("Fake UCI")
        );
        // A second run has nothing to do; a cancelled one ends at once.
        let mut again = build(
            &db,
            &player,
            &Options::default(),
            &AtomicUsize::new(0),
            &never,
        )
        .expect("report");
        let cancelled = AtomicBool::new(true);
        let mut calls = 0;
        analyse(
            &mut again,
            &mut cache,
            None,
            uci::EngineConfig::new(fake),
            &settings,
            &cancelled,
            &mut |_| calls += 1,
        )
        .expect("nothing to do");
        assert_eq!(again.missing(), 0, "all from the cache");
        assert_eq!(calls, 1);
    }
}
