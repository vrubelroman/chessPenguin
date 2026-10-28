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

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use uci::{EngineEvent, EnginePosition, ScoreBound, SearchId, SearchLimits, Side};

use crate::{Eval, PosEval, Second};

/// Nodes of every search of a game analysis (0.85 s on one thread of an i7-13700KF,
/// depth about 24).
pub const SEARCH_NODES: u64 = 1_000_000;

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

/// A node-limited search should never take this long; stop it if it does.
const SEARCH_GUARD: Duration = Duration::from_secs(300);

/// Engine processes for a job on this machine: one per logical CPU, at most `limit`
/// (and [`MAX_PROCESSES`]), and no more than the free memory holds.
pub fn processes(limit: usize) -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let memory = available_mb().map_or(usize::MAX, |mb| {
        (mb.saturating_sub(SHARED_MB) / PROCESS_MB).try_into().unwrap_or(usize::MAX)
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
    /// The evaluation already there: only the second search is left.
    pub known: Option<PosEval>,
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

enum WorkerMsg {
    Event(JobEvent),
    Failed(JobFailure),
}

/// Evaluates `targets` in `settings.processes` engine processes; `on_event` gets every
/// result, on the caller's thread, in the order they come. Returns when all are done,
/// or soon after `cancel` is set (then with `Ok`). When an engine fails, the others
/// stop too and the first failure is returned; what came before it was reported.
pub fn run(
    config: uci::EngineConfig,
    settings: &JobSettings,
    targets: Vec<Target>,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<(), JobFailure> {
    if targets.is_empty() {
        return Ok(());
    }
    let workers = settings.processes.clamp(1, MAX_PROCESSES).min(targets.len());
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let halted = || cancel.load(Ordering::Relaxed) || failed.load(Ordering::Relaxed);
    let (tx, rx) = channel();
    let mut failure = None;
    std::thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (config, targets, next, halted) = (config.clone(), &targets, &next, &halted);
            s.spawn(move || {
                let mut send = |e| {
                    let _ = tx.send(WorkerMsg::Event(e));
                };
                if let Err(e) = worker(config, settings, targets, next, halted, &mut send) {
                    let _ = tx.send(WorkerMsg::Failed(e));
                }
            });
        }
        drop(tx);
        let mut names = HashSet::new();
        for msg in rx {
            match msg {
                WorkerMsg::Event(JobEvent::Engine(name)) => {
                    if names.insert(name.clone()) {
                        on_event(JobEvent::Engine(name));
                    }
                }
                WorkerMsg::Event(e) => on_event(e),
                WorkerMsg::Failed(e) => {
                    failed.store(true, Ordering::Relaxed);
                    failure.get_or_insert(e);
                }
            }
        }
    });
    match failure {
        Some(e) if !cancel.load(Ordering::Relaxed) => Err(e),
        _ => Ok(()),
    }
}

/// One engine process: takes the next target until there is none or `halted`.
fn worker(
    config: uci::EngineConfig,
    settings: &JobSettings,
    targets: &[Target],
    next: &AtomicUsize,
    halted: &dyn Fn() -> bool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<(), JobFailure> {
    let handle = uci::Engine::spawn_with(config, Box::new(|| {}))
        .map_err(|e| JobFailure::Start(e.to_string()))?;
    if let Some(name) = &handle.info().name {
        on_event(JobEvent::Engine(name.clone()));
    }
    if let Some(file) = &settings.eval_file {
        handle
            .set_option("EvalFile", Some(file))
            .map_err(|e| JobFailure::Start(e.to_string()))?;
    }
    // Unknown options are not fatal: some engines lack Threads/Hash/MultiPV.
    let _ = handle.set_threads(1);
    let _ = handle.set_hash_mb(SEARCH_HASH_MB);
    let _ = handle.set_multipv(1);
    let mut next_id = 0;
    // Every search starts from a clear hash, so that it depends on its position alone.
    let mut search = |pos: &EnginePosition, among: Vec<String>| {
        next_id += 1;
        let _ = handle.new_game();
        search_once(&handle, pos, SearchId(next_id), among, settings.nodes, halted)
    };
    while let Some(t) = targets.get(next.fetch_add(1, Ordering::Relaxed)) {
        if halted() {
            break;
        }
        let (mut eval, nps) = match &t.known {
            Some(e) => (e.clone(), 0),
            None => {
                let Some((info, best)) = search(&t.pos, Vec::new())? else {
                    continue;
                };
                let Some(score) = info.score else { continue };
                let mut pv: Vec<String> = info.pv.iter().take(12).cloned().collect();
                if let Some(b) = best
                    && pv.first() != Some(&b)
                {
                    pv = vec![b];
                }
                let eval = PosEval {
                    eval: Eval::from_white_eval(score.to_white(t.turn).eval),
                    pv,
                    depth: info.depth.unwrap_or(0),
                    second: None,
                };
                (eval, info.nps.unwrap_or(0))
            }
        };
        // The move played is the engine's best: what the best of the others gives says
        // whether it was the only good move, or a sacrifice when nothing else won.
        let mover = match t.turn {
            Side::White => chess_core::shakmaty::Color::White,
            Side::Black => chess_core::shakmaty::Color::Black,
        };
        if let Some(played) = &t.played
            && eval.pv.first() == Some(played)
            && !t.others.is_empty()
            && crate::report::worth_second(eval.eval, mover)
        {
            match search(&t.pos, t.others.clone())? {
                // An engine that ignores `searchmoves` answers with the move played:
                // no second move then.
                Some((info, best))
                    if best.as_ref() != Some(played) && info.pv.first() != Some(played) =>
                {
                    if let Some(score) = info.score {
                        eval.second = Some(Second {
                            without: played.clone(),
                            eval: Eval::from_white_eval(score.to_white(t.turn).eval),
                        });
                    }
                }
                None if halted() => break,
                _ => {}
            }
        }
        on_event(JobEvent::Eval {
            key: t.key.clone(),
            eval,
            nps,
        });
    }
    handle.quit();
    Ok(())
}

/// One node-limited search: its last full line and the best move. `None` when it gave
/// no line or the job was halted.
fn search_once(
    handle: &uci::EngineHandle,
    pos: &EnginePosition,
    id: SearchId,
    among: Vec<String>,
    nodes: u64,
    halted: &dyn Fn() -> bool,
) -> Result<Option<(uci::SearchInfo, Option<String>)>, JobFailure> {
    handle
        .search_among(pos.clone(), id, SearchLimits::Nodes(nodes), among)
        .map_err(|e| JobFailure::Engine(e.to_string()))?;
    let started = Instant::now();
    let mut stopping = false;
    let mut last: Option<uci::SearchInfo> = None;
    loop {
        if halted() {
            return Ok(None);
        }
        if !stopping && started.elapsed() > SEARCH_GUARD {
            let _ = handle.stop();
            stopping = true;
        }
        match handle.recv_timeout(Duration::from_millis(100)) {
            Some(EngineEvent::Info { search_id, info }) if search_id == id => {
                if info.multipv_index() == 1 && info.score.is_some() {
                    let exact = info.score.is_some_and(|s| s.bound == ScoreBound::Exact);
                    if exact || last.is_none() {
                        last = Some(info);
                    }
                }
            }
            Some(EngineEvent::BestMove {
                search_id, best, ..
            }) if search_id == id => {
                return Ok(last.map(|info| (info, best.map(|b| b.to_string()))));
            }
            Some(EngineEvent::Terminated(reason)) => {
                return Err(JobFailure::Terminated(reason));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameAnalysis;

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
