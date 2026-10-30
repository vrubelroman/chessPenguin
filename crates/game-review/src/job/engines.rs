//! The engine job in engine processes, each driven from a thread of its own.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use uci::{EngineEvent, EnginePosition, ScoreBound, SearchId, SearchLimits};

use super::work::{Answer, Next, Work};
use super::{JobEvent, JobFailure, JobSettings, MAX_PROCESSES, SEARCH_HASH_MB, Target};

/// A node-limited search should never take this long; stop it if it does.
const SEARCH_GUARD: Duration = Duration::from_secs(300);

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
    let workers = settings
        .processes
        .clamp(1, MAX_PROCESSES)
        .min(targets.len());
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
    let mut search = |pos: &EnginePosition, among: Vec<String>, nodes: u64| {
        next_id += 1;
        let _ = handle.new_game();
        search_once(&handle, pos, SearchId(next_id), among, nodes, halted)
    };
    'targets: while let Some(t) = targets.get(next.fetch_add(1, Ordering::Relaxed)) {
        if halted() {
            break;
        }
        let mut work = Work::new(t.clone(), settings.nodes);
        loop {
            match work.next() {
                Next::Search { pos, among, nodes } => {
                    let answer = search(&pos, among, nodes)?;
                    if answer.is_none() && halted() {
                        break 'targets;
                    }
                    work.answer(answer);
                }
                Next::Done(event) => {
                    if let Some(e) = event {
                        on_event(e);
                    }
                    break;
                }
            }
        }
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
) -> Result<Answer, JobFailure> {
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
