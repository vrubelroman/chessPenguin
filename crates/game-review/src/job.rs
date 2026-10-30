//! The engine job of a game analysis: one node-limited search per position, in engine
//! processes of its own, results reported as they come. Runs on the caller's thread
//! until it is done or cancelled; the game is never touched.
//!
//! The search is the same everywhere (decision of 2026-10-28): every position is
//! searched to [`SEARCH_NODES`] nodes by an engine with one thread and a hash of
//! [`SEARCH_HASH_MB`], cleared (`ucinewgame`) before each search. One thread and a
//! clear hash make a search depend on its position alone — not on the machine, the
//! order of the positions or what was searched before — so every computer and phone
//! gets the same evaluations, and the same marks. Speed comes from several engine
//! processes, each taking the next position; their number changes nothing but the time.

use uci::{EnginePosition, Side};

use crate::PosEval;

// What the job does for one target, whatever drives it.
mod work;
// The job in engine processes, each on a thread of its own: a computer's and a
// phone's.
#[cfg(not(target_family = "wasm"))]
mod engines;
#[cfg(not(target_family = "wasm"))]
pub use engines::run;
// The job in Web Workers: a browser's.
#[cfg(target_family = "wasm")]
mod workers;
#[cfg(target_family = "wasm")]
pub use workers::Dispatcher;

/// Nodes of every search of a game analysis (0.85 s on one thread of an i7-13700KF,
/// depth about 24).
pub const SEARCH_NODES: u64 = 1_000_000;

/// Nodes of the shallow search, for how hard a move is to find ([`crate::Measure`]):
/// made only for a move that could be brilliant or great and was the engine's best, and
/// only when the rules ask for it ([`crate::Hardness`]). At 5 000 nodes (depth about 10)
/// Stockfish already finds nearly every move of note itself; above 10 000 hardly any is
/// hard (the chess.com check set, 2026-10-29). Its time is lost in that of the full
/// searches (+0.2 % for a game).
pub const SHALLOW_NODES: u64 = 5_000;

/// The engine's hash of every search, in MB. Part of the search: another size gives
/// other evaluations.
pub const SEARCH_HASH_MB: u32 = 32;

/// The version of the fixed search: [`SEARCH_NODES`], [`SEARCH_HASH_MB`], one thread, a
/// clear hash, and the engine (ChessPenguin's Stockfish 19). Raised when any of them
/// changes: evaluations of another version are stale and are made again, never mixed.
pub const SEARCH_VERSION: u32 = 1;

/// Engine processes of a job on a computer at most.
pub const MAX_PROCESSES: usize = 8;

/// Engine processes of a job on a phone at most.
pub const PHONE_PROCESSES: usize = 4;

/// Memory of the engine's network, shared by all its processes (Stockfish 19 keeps it
/// in shared memory), in MB.
const SHARED_MB: u64 = 200;

/// Memory of one engine process of its own (the hash of [`SEARCH_HASH_MB`] included),
/// in MB.
const PROCESS_MB: u64 = 70;

/// Engine processes for a job on this machine: one per logical CPU, at most `limit`
/// (and [`MAX_PROCESSES`]), and no more than the free memory holds.
pub fn processes(limit: usize) -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let memory = available_mb().map_or(usize::MAX, |mb| {
        (mb.saturating_sub(SHARED_MB) / PROCESS_MB)
            .try_into()
            .unwrap_or(usize::MAX)
    });
    cpus.min(limit).min(memory).clamp(1, MAX_PROCESSES)
}

/// Memory available to new processes (`MemAvailable` of Linux and Android), in MB;
/// `None` where it is not known.
fn available_mb() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// Seconds `searches` take at `nps` nodes per second of one engine process (one
/// thread), in `processes` of them: the time estimate of a run.
pub fn estimate_secs(searches: usize, nps: f64, processes: usize) -> f64 {
    let rate = nps.max(1.0) * processes.clamp(1, MAX_PROCESSES) as f64;
    searches as f64 * SEARCH_NODES as f64 / rate
}

/// A position to evaluate (from [`crate::GameAnalysis::begin`]).
#[derive(Clone, Debug)]
pub struct Target {
    pub key: String,
    pub pos: EnginePosition,
    pub turn: Side,
    /// The move played here (UCI), when it could be brilliant or great: if it is the
    /// engine's best, a second search among `others` finds the best of the rest.
    pub played: Option<String>,
    /// The other legal moves (UCI).
    pub others: Vec<String>,
    /// The evaluation already there: only the second or shallow search is left.
    pub known: Option<PosEval>,
    /// Nodes of the shallow search of `played`, when the rules want it.
    pub shallow: Option<u64>,
}

/// How the job runs; the search itself is fixed (see the module).
#[derive(Clone, Debug)]
pub struct JobSettings {
    /// Engine processes working at once (see [`processes`]).
    pub processes: usize,
    /// The network file of the engine (Stockfish `EvalFile`), when it is not beside it.
    pub eval_file: Option<String>,
    /// Nodes per search: [`SEARCH_NODES`] in the programs. Only an experiment (the
    /// chess.com check, choosing the nodes) searches otherwise, never mixing its
    /// evaluations with the programs'.
    pub nodes: u64,
}

impl JobSettings {
    /// The fixed search in `processes` engine processes.
    pub fn new(processes: usize, eval_file: Option<String>) -> JobSettings {
        JobSettings {
            processes,
            eval_file,
            nodes: SEARCH_NODES,
        }
    }
}

/// What the job reports, for [`crate::GameAnalysis::apply`].
#[derive(Clone, Debug)]
pub enum JobEvent {
    /// The engine's own name.
    Engine(String),
    Eval {
        key: String,
        eval: PosEval,
        /// The speed of the engine process in this search (one thread; 0 if it did
        /// not say).
        nps: u64,
    },
}

/// Why the job stopped before the end.
#[derive(Debug)]
pub enum JobFailure {
    /// The engine did not start or did not accept its settings.
    Start(String),
    /// Talking to the engine failed.
    Engine(String),
    /// The engine process ended (crashed, or hung and was killed).
    Terminated(uci::TerminationReason),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Eval;
    use crate::GameAnalysis;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The fake engine of the `uci` crate (`cargo build -p uci --bin fake_uci`).
    fn fake() -> &'static str {
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/debug/fake_uci");
        assert!(
            std::path::Path::new(fake).exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        fake
    }

    fn settings(processes: usize) -> JobSettings {
        JobSettings::new(processes, None)
    }

    /// The fake engine answers like Stockfish: every position gets an evaluation.
    #[test]
    fn the_job_evaluates_every_position() {
        let game = chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 *").game;
        let mut a = GameAnalysis::new(&game, None);
        let targets = a.begin(&game, false);
        let n = targets.len();
        let mut evals = 0;
        let cancel = AtomicBool::new(false);
        run(
            uci::EngineConfig::new(fake()),
            &settings(2),
            targets,
            &cancel,
            &mut |e| {
                if matches!(e, JobEvent::Eval { .. }) {
                    evals += 1;
                }
                a.apply(e);
            },
        )
        .expect("the job runs");
        assert_eq!(evals, n);
        a.refresh(&game);
        assert_eq!(a.missing(), 0);
        assert!(a.report.complete);
        assert_eq!(a.meta.as_ref().map(|m| m.engine.as_str()), Some("Fake UCI"));

        // A cancelled job ends at once, without results.
        let mut b = GameAnalysis::new(&game, None);
        let targets = b.begin(&game, false);
        cancel.store(true, Ordering::Relaxed);
        let mut got = 0;
        run(
            uci::EngineConfig::new(fake()),
            &settings(3),
            targets,
            &cancel,
            &mut |e| {
                if matches!(e, JobEvent::Eval { .. }) {
                    got += 1;
                }
            },
        )
        .expect("cancelled cleanly");
        assert_eq!(got, 0);
    }

    /// The evaluations depend on the positions alone: not on the number of engine
    /// processes, nor on the order of the positions. The fake engine with `--hash-memory`
    /// lets earlier searches change a score, as a hash kept between searches does: only
    /// a clear hash before every search gives the same results.
    #[test]
    fn the_evaluations_do_not_depend_on_processes_or_order() {
        let game = chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 *").game;
        let evaluate = |processes: usize, reverse: bool| {
            let mut a = GameAnalysis::new(&game, None);
            let mut targets = a.begin(&game, false);
            if reverse {
                targets.reverse();
            }
            let mut config = uci::EngineConfig::new(fake());
            config.args = vec!["--hash-memory".into()];
            run(
                config,
                &settings(processes),
                targets,
                &AtomicBool::new(false),
                &mut |e| a.apply(e),
            )
            .expect("the job runs");
            a.refresh(&game);
            (0..a.line().len())
                .map(|i| a.eval_at(i).cloned())
                .collect::<Vec<_>>()
        };
        let one = evaluate(1, false);
        assert!(one.iter().all(Option::is_some));
        // The fake scores differ by position: the comparison means something.
        assert!(one.windows(2).any(|w| w[0] != w[1]));
        assert_eq!(evaluate(1, true), one);
        assert_eq!(evaluate(3, false), one);
        assert_eq!(evaluate(8, true), one);
    }

    /// The shallow search is made only when the rules ask how hard a move was to find,
    /// for the engine's best move that could be brilliant or great; an analysis without
    /// it is filled in, without evaluating anything again. The fake engine (from a FEN
    /// it always says e2e4) honours `searchmoves`, and below the nodes given prefers
    /// d2d4 (+0.40) to e2e4 (+0.10 searched alone).
    #[test]
    fn the_shallow_search_says_how_hard_the_move_was() {
        let game = chess_core::parse_str(
            "[FEN \"rnbqkbnr/1ppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1\"] 1. e4 e5 *",
        )
        .game;
        let engine = |shallow_best: bool| {
            let mut config = uci::EngineConfig::new(fake());
            config.args = vec!["--searchmoves".into()];
            if shallow_best {
                config.args.extend([
                    "--shallow".into(),
                    (SHALLOW_NODES + 1).to_string().into(),
                    "d2d4".into(),
                ]);
            }
            config
        };
        let job = |a: &mut GameAnalysis, shallow_best: bool| {
            let targets = a.begin(&game, false);
            let n = targets.len();
            run(
                engine(shallow_best),
                &settings(2),
                targets,
                &AtomicBool::new(false),
                &mut |e| a.apply(e),
            )
            .expect("the job runs");
            a.refresh(&game);
            n
        };
        let mut a = GameAnalysis::new(&game, None);
        // Rules that do not ask how hard a move was: no shallow search.
        a.set_hardness(crate::Hardness {
            brilliant: None,
            ..Default::default()
        });
        assert_eq!(job(&mut a, true), 3);
        let e = a.eval_at(0).expect("evaluated").clone();
        assert_eq!(e.pv.first().map(String::as_str), Some("e2e4"));
        assert_eq!(e.second.as_ref().map(|s| s.eval), Some(Eval::Cp(10)));
        assert!(e.shallow.is_none(), "the rules do not ask for it");
        assert_eq!(a.seconds_missing(), 0);

        // Rules that ask for it: the earlier analysis wants the shallow search only.
        let hard = crate::Hardness {
            great: Some(1.0),
            gap: 1.0,
            ..Default::default()
        };
        a.set_hardness(hard);
        a.refresh(&game);
        assert_eq!(a.seconds_missing(), 1);
        assert_eq!(a.report.judgements[0], None, "not known to be hard");
        assert_eq!(job(&mut a, true), 1);
        assert_eq!(a.seconds_missing(), 0);
        let e = a.eval_at(0).expect("evaluated");
        let s = e.shallow.as_ref().expect("the shallow search");
        assert_eq!(
            (s.nodes, s.played.as_str(), s.best.as_str()),
            (SHALLOW_NODES, "e2e4", "d2d4")
        );
        assert_eq!((s.best_eval, s.played_eval), (Eval::Cp(40), Eval::Cp(10)));
        assert_eq!(e.second.as_ref().map(|s| s.eval), Some(Eval::Cp(10)));
        // 2.8 % harder than the shallow best: great at 1 %, not at 5 %, brilliant at 2 %.
        let d = crate::Measure::Choice.difficulty(
            e.eval,
            s.best_eval,
            s.played_eval,
            chess_core::shakmaty::Color::White,
        );
        assert!((2.0..5.0).contains(&d), "{d}");
        assert_eq!(a.report.judgements[0], Some(crate::Judgement::Great));
        a.set_hardness(crate::Hardness {
            great: Some(5.0),
            ..hard
        });
        a.refresh(&game);
        assert_eq!(a.report.judgements[0], None);
        a.set_hardness(crate::Hardness {
            brilliant: Some(2.0),
            brilliant_gap: 1.0,
            ..hard
        });
        a.refresh(&game);
        assert_eq!(a.report.judgements[0], Some(crate::Judgement::Brilliant));
        // Shallow searches of other nodes are made again.
        a.set_shallow_nodes(SHALLOW_NODES / 2);
        a.refresh(&game);
        assert_eq!(a.seconds_missing(), 1);
        assert_eq!(a.report.judgements[0], None);

        // The shallow search finding the move itself: not hard at all.
        let mut b = GameAnalysis::new(&game, None);
        b.set_hardness(hard);
        assert_eq!(job(&mut b, false), 3);
        let s = b
            .eval_at(0)
            .and_then(|e| e.shallow.clone())
            .expect("shallow");
        assert_eq!(
            (s.best.as_str(), s.best_eval, s.played_eval),
            ("e2e4", Eval::Cp(40), Eval::Cp(40))
        );
        assert_eq!(b.report.judgements[0], None, "0 % is below 1 %");
    }

    #[test]
    fn a_failing_engine_fails_the_job() {
        let game = chess_core::parse_str("1. e4 e5 *").game;
        let mut a = GameAnalysis::new(&game, None);
        let targets = a.begin(&game, false);
        let result = run(
            uci::EngineConfig::new("/nonexistent/engine"),
            &settings(2),
            targets,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(result, Err(JobFailure::Start(_))));
    }

    #[test]
    fn processes_are_bounded() {
        assert_eq!(processes(1), 1);
        assert!((1..=MAX_PROCESSES).contains(&processes(usize::MAX)));
        assert_eq!(processes(0), 1);
        assert!(processes(PHONE_PROCESSES) <= PHONE_PROCESSES);
    }

    #[test]
    fn the_estimate_counts_every_process() {
        assert_eq!(estimate_secs(10, 1_000_000.0, 1), 10.0);
        assert_eq!(estimate_secs(10, 1_000_000.0, 4), 2.5);
    }
}
