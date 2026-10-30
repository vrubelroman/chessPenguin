//! The engine job in engine processes, each driven from a thread of its own.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use uci::{EngineEvent, EnginePosition, ScoreBound, SearchId, SearchLimits, Side};

use super::{JobEvent, JobFailure, JobSettings, MAX_PROCESSES, SEARCH_HASH_MB, Target};
use crate::{Eval, PosEval, Second, Shallow};

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
    while let Some(t) = targets.get(next.fetch_add(1, Ordering::Relaxed)) {
        if halted() {
            break;
        }
        let (mut eval, nps) = match &t.known {
            Some(e) => (e.clone(), 0),
            None => {
                let Some((info, best)) = search(&t.pos, Vec::new(), settings.nodes)? else {
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
                    shallow: None,
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
        let played = t.played.as_ref().filter(|p| {
            eval.pv.first() == Some(*p)
                && !t.others.is_empty()
                && crate::report::worth_second(eval.eval, mover)
        });
        if let Some(played) = played
            && eval.second.as_ref().map(|s| &s.without) != Some(played)
        {
            match search(&t.pos, t.others.clone(), settings.nodes)? {
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
        // How hard it was to find: what a shallow search makes of it.
        if let (Some(played), Some(nodes)) = (played, t.shallow)
            && !eval
                .shallow
                .as_ref()
                .is_some_and(|s| &s.played == played && s.nodes == nodes)
        {
            match shallow(&mut search, t, played, nodes)? {
                Some(s) => eval.shallow = Some(s),
                None if halted() => break,
                None => {}
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

/// What a search gave: its last full line and the best move, `None` when there is none
/// (see [`search_once`]).
type SearchResult = Result<Option<(uci::SearchInfo, Option<String>)>, JobFailure>;

/// The shallow search of `played` at `t`: the best move at `nodes` and, when it is
/// another, the move played alone. `None` when the engine gave no answer (or ignored
/// `searchmoves`) or the job was halted.
fn shallow(
    search: &mut dyn FnMut(&EnginePosition, Vec<String>, u64) -> SearchResult,
    t: &Target,
    played: &str,
    nodes: u64,
) -> Result<Option<Shallow>, JobFailure> {
    let eval_of = |info: &uci::SearchInfo| {
        info.score
            .map(|s| Eval::from_white_eval(s.to_white(t.turn).eval))
    };
    let Some((info, best)) = search(&t.pos, Vec::new(), nodes)? else {
        return Ok(None);
    };
    let (Some(best), Some(best_eval)) = (best.or_else(|| info.pv.first().cloned()), eval_of(&info))
    else {
        return Ok(None);
    };
    let played_eval = if best == played {
        best_eval
    } else {
        match search(&t.pos, vec![played.to_owned()], nodes)? {
            Some((info, b)) if b.as_deref().is_none_or(|b| b == played) => {
                let Some(e) = eval_of(&info) else {
                    return Ok(None);
                };
                e
            }
            _ => return Ok(None),
        }
    };
    Ok(Some(Shallow {
        nodes,
        played: played.to_owned(),
        best,
        best_eval,
        played_eval,
    }))
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
) -> SearchResult {
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
