//! The engine job of a game analysis: its own engine process, one node-limited search
//! per position, results reported as they come. Runs on the caller's thread until it
//! is done or cancelled; the game is never touched.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use uci::{EngineEvent, EnginePosition, ScoreBound, SearchId, SearchLimits, Side};

use crate::{Eval, PosEval, Second};

/// A node-limited search should never take this long; stop it if it does.
const SEARCH_GUARD: Duration = Duration::from_secs(300);

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

/// How the engine searches.
#[derive(Clone, Debug)]
pub struct JobSettings {
    pub threads: u32,
    pub hash_mb: u32,
    /// Nodes per position.
    pub nodes: u64,
    /// The network file of the engine (Stockfish `EvalFile`), when it is not beside it.
    pub eval_file: Option<String>,
}

/// What the job reports, for [`crate::GameAnalysis::apply`].
#[derive(Clone, Debug)]
pub enum JobEvent {
    /// The engine's own name.
    Engine(String),
    Eval {
        key: String,
        eval: PosEval,
        /// The engine's speed in this search (0 if it did not say).
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

/// Evaluates `targets` one by one; `on_event` gets every result. Returns when all are
/// done, or soon after `cancel` is set (then with `Ok`).
pub fn run(
    config: uci::EngineConfig,
    settings: &JobSettings,
    targets: Vec<Target>,
    cancel: &AtomicBool,
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
    let _ = handle.set_threads(settings.threads.max(1));
    let _ = handle.set_hash_mb(settings.hash_mb.max(1));
    let _ = handle.set_multipv(1);
    let _ = handle.new_game();
    let mut next_id = 0;
    let mut search = |pos: &EnginePosition, among: Vec<String>| {
        next_id += 1;
        search_once(
            &handle,
            pos,
            SearchId(next_id),
            among,
            settings.nodes,
            cancel,
        )
    };
    for t in targets {
        let (mut eval, nps) = match t.known {
            Some(e) => (e, 0),
            None => {
                let Some((info, best)) = search(&t.pos, Vec::new())? else {
                    if cancel.load(Ordering::Relaxed) {
                        handle.quit();
                        return Ok(());
                    }
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
                None if cancel.load(Ordering::Relaxed) => {
                    handle.quit();
                    return Ok(());
                }
                _ => {}
            }
        }
        on_event(JobEvent::Eval {
            key: t.key,
            eval,
            nps,
        });
    }
    handle.quit();
    Ok(())
}

/// One node-limited search: its last full line and the best move. `None` when it gave
/// no line or `cancel` was set.
fn search_once(
    handle: &uci::EngineHandle,
    pos: &EnginePosition,
    id: SearchId,
    among: Vec<String>,
    nodes: u64,
    cancel: &AtomicBool,
) -> Result<Option<(uci::SearchInfo, Option<String>)>, JobFailure> {
    handle
        .search_among(pos.clone(), id, SearchLimits::Nodes(nodes), among)
        .map_err(|e| JobFailure::Engine(e.to_string()))?;
    let started = Instant::now();
    let mut stopping = false;
    let mut last: Option<uci::SearchInfo> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
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
    use crate::{GameAnalysis, ReviewMeta};

    /// The fake engine of the `uci` crate (`cargo build -p uci --bin fake_uci`) answers
    /// like Stockfish: every position gets an evaluation, in order.
    #[test]
    fn the_job_evaluates_every_position() {
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/debug/fake_uci");
        assert!(
            std::path::Path::new(fake).exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        let game = chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 *").game;
        let mut a = GameAnalysis::new(&game, None);
        let meta = ReviewMeta {
            engine: String::new(),
            nodes: 1000,
        };
        let targets = a.begin(&game, meta, false);
        let n = targets.len();
        let settings = JobSettings {
            threads: 1,
            hash_mb: 16,
            nodes: 1000,
            eval_file: None,
        };
        let mut evals = 0;
        let cancel = AtomicBool::new(false);
        run(
            uci::EngineConfig::new(fake),
            &settings,
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

        // A cancelled job ends at once, without results.
        let mut b = GameAnalysis::new(&game, None);
        let targets = b.begin(&game, ReviewMeta::default(), false);
        cancel.store(true, Ordering::Relaxed);
        let mut got = 0;
        run(
            uci::EngineConfig::new(fake),
            &settings,
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
}
