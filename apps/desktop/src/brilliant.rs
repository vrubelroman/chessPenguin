//! The search for brilliant and great moves (!!, !) in the games of a filtered list
//! (task 27, docs/design/task-27-brilliant-search-2026-11-27.md): a background pass of
//! the game analysis's engine over at most [`LIMIT`] games, the index of what each
//! analysed record holds (kept beside the analysis cache, one file per database), and
//! the totals of a pass by category. Games already analysed come from the analysis
//! cache, the engine is not asked again; the source database is only read.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chess_core::Game;
use chess_import::Database;
use game_review::{CacheStore, GameAnalysis, JobEvent, JobFailure, JobSettings, Judgement};
use serde::{Deserialize, Serialize};

/// A pass takes at most this many games (the owner's limit, inclusive).
pub const LIMIT: usize = 100;

/// Why a pass over `n` listed games cannot start (the button is disabled with this).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    NoGames,
    TooMany,
}

/// Whether a pass over a list of `n` games may start: 1 to [`LIMIT`] inclusive.
pub fn can_start(n: usize) -> Result<(), Blocked> {
    match n {
        0 => Err(Blocked::NoGames),
        n if n > LIMIT => Err(Blocked::TooMany),
        _ => Ok(()),
    }
}

/// Engine judgements of one side of a game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub brilliant: u32,
    pub great: u32,
    pub inaccuracies: u32,
    pub mistakes: u32,
    pub blunders: u32,
}

/// One side of an analysed game: its judgements, average centipawn loss and accuracy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SideMarks {
    #[serde(flatten)]
    pub counts: Counts,
    #[serde(default)]
    pub acpl: Option<u32>,
    #[serde(default)]
    pub accuracy: Option<f64>,
}

impl SideMarks {
    fn of(s: &game_review::SideStats) -> SideMarks {
        SideMarks {
            counts: Counts {
                brilliant: s.brilliant,
                great: s.great,
                inaccuracies: s.inaccuracies,
                mistakes: s.mistakes,
                blunders: s.blunders,
            },
            acpl: s.acpl,
            accuracy: s.accuracy,
        }
    }
}

impl Counts {
    pub fn add(&mut self, o: &Counts) {
        self.brilliant += o.brilliant;
        self.great += o.great;
        self.inaccuracies += o.inaccuracies;
        self.mistakes += o.mistakes;
        self.blunders += o.blunders;
    }
}

/// A move of note: its ply in the main line (0: the first move), who made it, its
/// number and SAN (Latin letters, as in PGN), and whether it is brilliant (else great).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    pub ply: u16,
    pub white: bool,
    pub number: u32,
    pub san: String,
    pub brilliant: bool,
}

impl Found {
    /// «21.Rxf7!!», «34…Nf3+!!» in the notation of the settings.
    pub fn label(&self) -> String {
        let dots = if self.white { "." } else { "…" };
        let mark = if self.brilliant { "!!" } else { "!" };
        format!("{}{dots}{}{mark}", self.number, crate::san::show(&self.san))
    }
}

/// Why a game is not analysed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Skip {
    NoMoves,
    Chess960,
    /// A training null move (`--`) in the main line.
    NullMove,
    Unreadable,
}

impl Skip {
    pub fn reason(self) -> &'static str {
        crate::i18n::tr(match self {
            Skip::NoMoves => "no moves",
            Skip::Chess960 => "Chess960",
            Skip::NullMove => "a null move in the main line",
            Skip::Unreadable => "the game cannot be read",
        })
    }
}

/// What the index knows of a record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The main line's name in the analysis cache: the entry holds only while the record
    /// still has this main line.
    pub line: String,
    #[serde(default)]
    pub white: SideMarks,
    #[serde(default)]
    pub black: SideMarks,
    /// Every !! and the first three !.
    #[serde(default)]
    pub found: Vec<Found>,
    /// Analysed with the engine settings of before (counted as analysed).
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub skip: Option<Skip>,
}

impl Entry {
    /// The entry with only the marks of the sides that count (the filter's player): the
    /// row's sign and hint show only them.
    pub fn for_sides(&self, white: bool, black: bool) -> Entry {
        let keep = |on: bool, s: &SideMarks| if on { *s } else { SideMarks::default() };
        Entry {
            white: keep(white, &self.white),
            black: keep(black, &self.black),
            found: self
                .found
                .iter()
                .filter(|f| if f.white { white } else { black })
                .cloned()
                .collect(),
            ..self.clone()
        }
    }

    /// The !! and ! of the sides that count.
    pub fn of_sides(&self, white: bool, black: bool) -> Counts {
        let mut c = Counts::default();
        if white {
            c.add(&self.white.counts);
        }
        if black {
            c.add(&self.black.counts);
        }
        c
    }
}

/// The record of an analysis of `game`: its marks per side and its moves of note.
pub fn entry_of(analysis: &GameAnalysis, game: &Game) -> Entry {
    let report = &analysis.report;
    let line = analysis.line();
    let mut found = Vec::new();
    let mut great = 0;
    for (ply, j) in report.judgements.iter().enumerate() {
        let brilliant = match j {
            Some(Judgement::Brilliant) => true,
            Some(Judgement::Great) if great < 3 => {
                great += 1;
                false
            }
            _ => continue,
        };
        let Some(&node) = line.get(ply + 1) else {
            continue;
        };
        let (Some(m), Some((number, color))) = (game.game_move(node), game.move_number(node))
        else {
            continue;
        };
        found.push(Found {
            ply: ply as u16,
            white: color == chess_core::shakmaty::Color::White,
            number,
            san: m.san.clone(),
            brilliant,
        });
    }
    Entry {
        line: analysis.cache_key(),
        white: SideMarks::of(&report.white),
        black: SideMarks::of(&report.black),
        found,
        stale: analysis.stale(),
        skip: None,
    }
}

/// Why `game` is not analysed, if it is not.
pub fn skip_of(game: &Game, chess960: bool) -> Option<Skip> {
    if chess960 {
        return Some(Skip::Chess960);
    }
    let line = game.mainline();
    if line.is_empty() {
        return Some(Skip::NoMoves);
    }
    if line
        .iter()
        .any(|id| game.game_move(*id).is_none_or(|m| m.is_null()))
    {
        return Some(Skip::NullMove);
    }
    None
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    records: Vec<(u32, Entry)>,
}

const INDEX_VERSION: u32 = 1;

/// The marks of a database's records, by record number, in a file of their own. No
/// file (or one of another version) is an empty index: nothing found yet.
#[derive(Debug, Default)]
pub struct Index {
    file: Option<PathBuf>,
    records: BTreeMap<u32, Entry>,
    /// Changes since it was read (the table re-checks what it shows).
    pub generation: u64,
}

/// The folder of the indexes: beside the analysis cache.
fn index_dir() -> Option<PathBuf> {
    crate::settings::Settings::cache_dir().map(|d| d.join("marks"))
}

/// The index file of the database at `db`: named by a hash of its real path.
pub fn index_file(dir: &Path, db: &Path) -> PathBuf {
    let real = std::fs::canonicalize(db).unwrap_or_else(|_| db.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    real.hash(&mut h);
    dir.join(format!("{:016x}.json", h.finish()))
}

impl Index {
    /// The index of the database at `db`, from the program's cache.
    pub fn of(db: &Path) -> Index {
        match index_dir() {
            Some(dir) => Index::load(index_file(&dir, db)),
            None => Index::default(),
        }
    }

    /// The index kept in `file`.
    pub fn load(file: PathBuf) -> Index {
        let records = match crate::platform::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice::<IndexFile>(&bytes) {
                Ok(f) if f.version == INDEX_VERSION => f.records.into_iter().collect(),
                Ok(f) => {
                    eprintln!("marks: {}: version {}", file.display(), f.version);
                    BTreeMap::new()
                }
                Err(e) => {
                    eprintln!("marks: {}: {e}", file.display());
                    BTreeMap::new()
                }
            },
            Err(_) => BTreeMap::new(),
        };
        Index {
            file: Some(file),
            records,
            generation: 0,
        }
    }

    pub fn get(&self, record: u32) -> Option<&Entry> {
        self.records.get(&record)
    }

    pub fn set(&mut self, record: u32, entry: Entry) {
        self.records.insert(record, entry);
        self.generation += 1;
    }

    /// Where a record goes when the table is sorted by its marks: !! first, then !,
    /// analysed without them, not analysable, not analysed.
    pub fn rank(&self, record: u32, sides: &Sides) -> u8 {
        match self.records.get(&record) {
            None => 4,
            Some(e) if e.skip.is_some() => 3,
            Some(e) => {
                let (w, b) = sides.of(record);
                let c = e.of_sides(w, b);
                if c.brilliant > 0 {
                    0
                } else if c.great > 0 {
                    1
                } else {
                    2
                }
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Written whole into a temporary file and renamed over the old one.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let out = IndexFile {
            version: INDEX_VERSION,
            records: self.records.iter().map(|(r, e)| (*r, e.clone())).collect(),
        };
        let bytes = serde_json::to_vec(&out).map_err(std::io::Error::other)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::platform::fs::write_atomic(file, &bytes)
    }
}

/// How far a pass is: games done, and the record being analysed (its number + 1; 0:
/// none).
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub current: AtomicUsize,
}

/// How a pass ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Stopped,
    EngineFailed(String),
    SourceChanged,
}

/// What a pass did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassEnd {
    pub outcome: Outcome,
    /// Records analysed (by the engine now, or from the cache), in the pass's order.
    pub reviewed: Vec<u32>,
    /// Of them, taken from the cache without the engine.
    pub from_cache: usize,
    pub skipped: Vec<(u32, Skip)>,
}

/// The engine of a pass: started for each game that needs it.
pub struct Engine {
    pub config: uci::EngineConfig,
    pub settings: JobSettings,
    /// Whether the engine, by the name it gives, is still the one checked.
    pub fits: Box<dyn Fn(&str) -> bool + Send + Sync>,
    /// A pause before each game (`CP_BRILLIANT_SLOW`: the progress can be photographed).
    pub pause: std::time::Duration,
}

/// Analyses `records` of `db` in this order: each from the analysis cache in `cache`
/// when it is there (also one made with the engine settings of before), else with
/// `engine`; after each game the cache and `index` are saved, so a stop or a crash
/// keeps what was done. `cancel` stops it within the current search.
pub fn run_pass(
    db: &Database,
    records: &[u32],
    index: &Mutex<Index>,
    cache: &dyn CacheStore,
    engine: &Engine,
    progress: &Progress,
    cancel: &AtomicBool,
) -> PassEnd {
    let mut end = PassEnd {
        outcome: Outcome::Finished,
        reviewed: Vec::new(),
        from_cache: 0,
        skipped: Vec::new(),
    };
    for &record in records {
        if cancel.load(Ordering::Relaxed) {
            end.outcome = Outcome::Stopped;
            break;
        }
        if let Some(pgn) = db.as_pgn()
            && pgn.source_changed().unwrap_or(true)
        {
            end.outcome = Outcome::SourceChanged;
            break;
        }
        progress
            .current
            .store(record as usize + 1, Ordering::Relaxed);
        if !engine.pause.is_zero() {
            std::thread::sleep(engine.pause);
            if cancel.load(Ordering::Relaxed) {
                end.outcome = Outcome::Stopped;
                break;
            }
        }
        let index_skip = |skip: Skip, line: String| {
            if let Ok(mut i) = index.lock() {
                i.set(
                    record,
                    Entry {
                        line,
                        white: SideMarks::default(),
                        black: SideMarks::default(),
                        found: Vec::new(),
                        stale: false,
                        skip: Some(skip),
                    },
                );
                let _ = i.save();
            }
        };
        let Some(parsed) = db.load_game(record as usize) else {
            end.skipped.push((record, Skip::Unreadable));
            index_skip(Skip::Unreadable, String::new());
            progress.done.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let game = parsed.game;
        let chess960 = db.row(record as usize).chess960;
        let mut analysis = GameAnalysis::with_store(&game, Some(cache));
        if let Some(skip) = skip_of(&game, chess960) {
            end.skipped.push((record, skip));
            index_skip(skip, analysis.cache_key());
            progress.done.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let by_engine = analysis.has_data() && analysis.meta.as_ref().is_some_and(|m| m.nodes > 0);
        // An analysis with every position evaluated is one, also of the engine settings
        // of before (the owner's decision; `begin` would drop that one): it is taken as
        // it is, the engine is not asked.
        let targets = if by_engine && (analysis.stale() || analysis.missing() == 0) {
            Vec::new()
        } else {
            analysis.begin(&game, !by_engine)
        };
        if targets.is_empty() && by_engine {
            end.from_cache += 1;
        } else {
            let mut changed = false;
            let result = game_review::run(
                engine.config.clone(),
                &engine.settings,
                targets,
                cancel,
                &mut |e| {
                    if changed {
                        return;
                    }
                    if let JobEvent::Engine(name) = &e
                        && !(engine.fits)(name)
                    {
                        changed = true;
                        return;
                    }
                    analysis.apply(e);
                },
            );
            if changed {
                end.outcome = Outcome::EngineFailed(
                    crate::i18n::tr("the engine is no longer the Stockfish 19 that was checked")
                        .to_owned(),
                );
                break;
            }
            if let Err(e) = result {
                end.outcome = Outcome::EngineFailed(match e {
                    JobFailure::Start(e) | JobFailure::Engine(e) => e,
                    JobFailure::Terminated(r) => crate::analysis::termination_message(&r),
                });
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                // The evaluations made are kept for later; the game is not counted.
                let _ = analysis.save_to(cache);
                end.outcome = Outcome::Stopped;
                break;
            }
            if let Err(e) = analysis.save_to(cache) {
                eprintln!("review cache: {e}");
            }
        }
        analysis.refresh(&game);
        if let Ok(mut i) = index.lock() {
            i.set(record, entry_of(&analysis, &game));
            if let Err(e) = i.save() {
                eprintln!("marks: {e}");
            }
        }
        end.reviewed.push(record);
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    progress.current.store(0, Ordering::Relaxed);
    end
}

/// Whose moves count in the totals of each record: the player of the filter's sides
/// (`white`, `black`), or both when the filter names no player.
#[derive(Clone, Debug, Default)]
pub struct Sides {
    /// Records where the filter's player has White, and Black; `None`: no player.
    pub player: Option<(Vec<u32>, Vec<u32>)>,
}

impl Sides {
    /// Whether White's and Black's moves of `record` count.
    pub fn of(&self, record: u32) -> (bool, bool) {
        match &self.player {
            None => (true, true),
            Some((w, b)) => (
                w.binary_search(&record).is_ok(),
                b.binary_search(&record).is_ok(),
            ),
        }
    }
}

/// The totals of a pass: games analysed, those with !! and with !, every category
/// summed over the sides that count, the averages over those sides, and the !! moves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub games: usize,
    pub with_brilliant: usize,
    pub with_great: usize,
    pub counts: Counts,
    /// Sides counted (a game counts twice when both of its sides do).
    pub sides: usize,
    /// Average over the sides of their average centipawn loss, and of their accuracy,
    /// with the least and the greatest accuracy.
    pub acpl: Option<f64>,
    pub accuracy: Option<f64>,
    pub accuracy_range: Option<(f64, f64)>,
    /// Of them, analysed with the engine settings of before.
    pub stale: usize,
    /// Records where the filter's player was found on neither side.
    pub player_missing: Vec<u32>,
    /// Every !! of the sides that count: record and move, in the records' order.
    pub brilliant: Vec<(u32, Found)>,
}

/// Sums the index's entries of `records` over the sides that count (`sides`).
pub fn totals(index: &Index, records: &[u32], sides: &Sides) -> Totals {
    let mut t = Totals::default();
    let (mut acpl, mut acc) = (Vec::new(), Vec::new());
    for &r in records {
        let Some(e) = index.get(r).filter(|e| e.skip.is_none()) else {
            continue;
        };
        let (white, black) = sides.of(r);
        if !white && !black {
            t.player_missing.push(r);
        }
        let c = e.of_sides(white, black);
        t.games += 1;
        t.stale += usize::from(e.stale);
        t.with_brilliant += usize::from(c.brilliant > 0);
        t.with_great += usize::from(c.great > 0);
        t.counts.add(&c);
        for (counted, side) in [(white, &e.white), (black, &e.black)] {
            if counted {
                t.sides += 1;
                acpl.extend(side.acpl.map(f64::from));
                acc.extend(side.accuracy);
            }
        }
        t.brilliant.extend(
            e.found
                .iter()
                .filter(|f| f.brilliant && if f.white { white } else { black })
                .map(|f| (r, f.clone())),
        );
    }
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    t.acpl = mean(&acpl);
    t.accuracy = mean(&acc);
    t.accuracy_range = acc.iter().copied().fold(None, |r: Option<(f64, f64)>, a| {
        Some(r.map_or((a, a), |(lo, hi)| (lo.min(a), hi.max(a))))
    });
    t
}

/// The records of `view` (a pass's list) where the filter's player has White and Black:
/// the same filter with the side fixed, for `Sides`.
pub fn player_sides(
    db: &Database,
    filter: &chess_import::chessbase::base::Filter,
    cancel: &AtomicBool,
) -> Sides {
    use chess_import::chessbase::base::{Side, SortKey};
    if !filter.player.is_set() {
        return Sides::default();
    }
    let side = |s: Side| {
        let f = chess_import::chessbase::base::Filter {
            side: s,
            ..filter.clone()
        };
        let mut v = db
            .query(&f, SortKey::Number, true, &AtomicUsize::new(0), cancel)
            .unwrap_or_default();
        v.sort_unstable();
        v
    };
    let (w, b) = match filter.side {
        Side::White => (side(Side::White), Vec::new()),
        Side::Black => (Vec::new(), side(Side::Black)),
        _ => (side(Side::White), side(Side::Black)),
    };
    Sides {
        player: Some((w, b)),
    }
}

/// What the app keeps of a pass for its summary: the filter, the words for its
/// conditions, and the games that could not be analysed.
pub type PassMeta = (
    chess_import::chessbase::base::Filter,
    String,
    Vec<(u32, Skip)>,
);

/// A pass under way, on a thread of its own: its source, its list, how far it is, and
/// how it ended once it did.
pub struct PassJob {
    pub source: PathBuf,
    pub records: Vec<u32>,
    progress: std::sync::Arc<Progress>,
    cancel: std::sync::Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<(PassEnd, Sides)>,
    pub stopping: bool,
    pub started: std::time::Instant,
}

impl PassJob {
    /// Starts the pass over `records` of `db` (the database at `source`); the sides of
    /// the filter's player are found first, in the same thread.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        ctx: &egui::Context,
        source: PathBuf,
        db: std::sync::Arc<Database>,
        records: Vec<u32>,
        filter: chess_import::chessbase::base::Filter,
        index: std::sync::Arc<Mutex<Index>>,
        cache: PathBuf,
        engine: Engine,
    ) -> std::io::Result<PassJob> {
        let progress = std::sync::Arc::new(Progress::default());
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (p, c, list, ctx) = (
            progress.clone(),
            cancel.clone(),
            records.clone(),
            ctx.clone(),
        );
        std::thread::Builder::new()
            .name("brilliant-search".into())
            .spawn(move || {
                let sides = player_sides(&db, &filter, &c);
                let store = crate::review::Cache(cache);
                let end = run_pass(&db, &list, &index, &store, &engine, &p, &c);
                let _ = tx.send((end, sides));
                ctx.request_repaint();
            })?;
        Ok(PassJob {
            source,
            records,
            progress,
            cancel,
            rx,
            stopping: false,
            started: std::time::Instant::now(),
        })
    }

    pub fn done(&self) -> usize {
        self.progress.done.load(Ordering::Relaxed)
    }

    /// The record being analysed.
    pub fn current(&self) -> Option<u32> {
        match self.progress.current.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n as u32 - 1),
        }
    }

    /// «Stop»: the engines stop within their search; what was analysed stays.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.stopping = true;
    }

    /// How it ended, once it did.
    pub fn poll(&self) -> Option<(PassEnd, Sides)> {
        self.rx.try_recv().ok()
    }

    /// Stops and waits up to `timeout` (closing the program).
    pub fn shutdown(&mut self, timeout: std::time::Duration) {
        self.stop();
        let _ = self.rx.recv_timeout(timeout);
    }

    /// The time left, from the pace so far after three games, else from the moves.
    pub fn left_secs(&self, estimate: impl Fn(&[u32]) -> f64) -> f64 {
        let done = self.done();
        let rest = &self.records[done.min(self.records.len())..];
        if done >= 3 {
            self.started.elapsed().as_secs_f64() / done as f64 * rest.len() as f64
        } else {
            estimate(rest)
        }
    }
}

impl Drop for PassJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Seconds an engine takes for games of these many plies: one search per position and a
/// second one for some, about 0.94 s per position in one process
/// (docs/decisions.md, «Analysis with fixed parameters»), over `processes`.
pub fn estimate_secs(plies: impl Iterator<Item = usize>, processes: usize) -> f64 {
    plies.map(|p| (p + 1) as f64 * 0.94).sum::<f64>() / processes.max(1) as f64
}

#[cfg(test)]
#[path = "brilliant_tests.rs"]
mod tests;
