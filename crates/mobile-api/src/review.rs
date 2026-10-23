//! The game report for the Android app: the analysis of a game's main line by the
//! `game-review` crate (the same evaluations, judgements, accuracy and cache as on the
//! desktop), with its engine job on a thread of its own. The app starts it, asks for
//! the state while it runs, and gets everything the graph, the report and the marks in
//! the notation need, in the node numbers of [`crate::GameData`].
//!
//! The game is never changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chess_core::{Game, NodeId};
use game_review::{
    Eval, GameAnalysis, JobEvent, JobFailure, JobSettings, Judgement, ReviewMeta, SideStats,
};
use shakmaty::Color;

use crate::EngineLine;
use crate::engine::{line_with, pawns};

/// How bad a move was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MoveJudgement {
    Inaccuracy,
    Mistake,
    Blunder,
}

impl From<Judgement> for MoveJudgement {
    fn from(j: Judgement) -> Self {
        match j {
            Judgement::Inaccuracy => MoveJudgement::Inaccuracy,
            Judgement::Mistake => MoveJudgement::Mistake,
            Judgement::Blunder => MoveJudgement::Blunder,
        }
    }
}

/// A position of the main line: the start, then the one after each move.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ReviewPoint {
    /// The node (of [`crate::GameData`]) of this position.
    pub node: u32,
    /// White's chance of winning, 0..100, when the position is evaluated.
    pub white_win: Option<f32>,
    /// "+0.35", "−1.20", "#3", "#−2", "1-0" ("" while not evaluated).
    pub eval: String,
    /// The judgement of the move that led here.
    pub judgement: Option<MoveJudgement>,
}

/// One player's numbers.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SideReview {
    pub inaccuracies: u32,
    pub mistakes: u32,
    pub blunders: u32,
    /// Average centipawn loss.
    pub acpl: Option<u32>,
    /// Accuracy 0..100 (only when every position is evaluated).
    pub accuracy: Option<f64>,
    pub opening: Option<f64>,
    pub middlegame: Option<f64>,
    pub endgame: Option<f64>,
}

/// An inaccuracy, mistake or blunder of the main line.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ReviewMove {
    /// The node of the move.
    pub node: u32,
    pub judgement: MoveJudgement,
    /// White made the move.
    pub white: bool,
    /// The engine's line from the position before the move (its first move was
    /// better), to show on the board; `None` when the evaluation came from the PGN.
    pub best: Option<EngineLine>,
    /// The better move with its number, Latin letters: "23…Nf6" ("" when unknown).
    pub best_label: String,
    /// How the opponent punishes the move: the engine's reply, for an arrow.
    pub reply_from: Option<String>,
    pub reply_to: Option<String>,
}

/// Everything about the analysis of the game at this moment.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ReviewState {
    /// The engine job is running.
    pub running: bool,
    /// Positions evaluated by this run, of `total`.
    pub done: u32,
    pub total: u32,
    /// Why the last run stopped early (the engine crashed, did not start).
    pub failure: Option<String>,
    /// Some evaluations exist (from the engine, the cache or the PGN).
    pub has_data: bool,
    /// The engine's name; "PGN" when the evaluations come from `[%eval]` comments.
    pub engine: String,
    /// Nodes per position; 0 for evaluations from the PGN.
    pub nodes: u64,
    /// Positions of the main line without an evaluation.
    pub missing: u32,
    /// Positions a full analysis looks at (for the time estimate).
    pub positions: u32,
    /// The engine's speed in the last run, per thread (0 when unknown).
    pub nps_per_thread: u64,
    /// The start position and the position after every main-line move.
    pub points: Vec<ReviewPoint>,
    /// Where the middlegame and the endgame begin, as indices of `points`.
    pub middlegame: Option<u32>,
    pub endgame: Option<u32>,
    pub white: SideReview,
    pub black: SideReview,
    /// The errors, in the order of the game.
    pub moves: Vec<ReviewMove>,
}

enum Msg {
    Event(JobEvent),
    Done,
    Failed(String),
}

struct Job {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    threads: u32,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Inner {
    game: Game,
    /// Node ids of the game → node numbers of [`crate::GameData`].
    places: HashMap<NodeId, u32>,
    analysis: GameAnalysis,
    cache_dir: Option<PathBuf>,
    job: Option<Job>,
    done: u32,
    total: u32,
    failure: Option<String>,
    nps_per_thread: u64,
    /// When the cache was last written: a long run is kept now and then, so that a
    /// crash or a killed app loses little of it.
    saved: Instant,
}

/// How often a running analysis is written to the cache.
const SAVE_EVERY: Duration = Duration::from_secs(10);

/// The analysis of one game.
#[derive(uniffi::Object)]
pub struct GameReview {
    inner: Mutex<Inner>,
}

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl GameReview {
    /// The analysis of `game`; earlier results come from `cache_dir` (empty: no cache).
    pub(crate) fn new(game: Game, cache_dir: String) -> Arc<Self> {
        let cache_dir = (!cache_dir.is_empty()).then(|| PathBuf::from(cache_dir));
        let analysis = GameAnalysis::new(&game, cache_dir.as_deref());
        Arc::new(GameReview {
            inner: Mutex::new(Inner {
                places: crate::places(&game),
                game,
                analysis,
                cache_dir,
                job: None,
                done: 0,
                total: 0,
                failure: None,
                nps_per_thread: 0,
                saved: Instant::now(),
            }),
        })
    }
}

#[uniffi::export]
impl GameReview {
    /// Starts the engine at `path` (with its network `eval_file`, if not empty) on the
    /// positions without an evaluation, or on all of them when `full`. Returns false when
    /// there is nothing to analyse. Returns at once: the engine starts on its own thread.
    pub fn start(
        &self,
        path: String,
        eval_file: String,
        threads: u32,
        hash_mb: u32,
        nodes: u64,
        full: bool,
    ) -> bool {
        let mut st = lock(&self.inner);
        stop(&mut st);
        st.failure = None;
        let meta = ReviewMeta {
            engine: "Stockfish".into(),
            nodes,
        };
        let Inner { game, analysis, .. } = &mut *st;
        let targets = analysis.begin(game, meta, full);
        if targets.is_empty() {
            return false;
        }
        // Filling in keeps the quality of the earlier run (see `GameAnalysis::begin`).
        let nodes = analysis.meta.as_ref().map_or(nodes, |m| m.nodes);
        let settings = JobSettings {
            threads: threads.max(1),
            hash_mb: hash_mb.max(1),
            nodes,
            eval_file: (!eval_file.is_empty()).then_some(eval_file),
        };
        // Below the app (nice 10), as the live engine: the phone stays smooth.
        let config = uci::EngineConfig {
            nice: 10,
            ..uci::EngineConfig::new(path)
        };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        st.total = targets.len() as u32;
        st.done = 0;
        let threads = settings.threads;
        let spawned = std::thread::Builder::new()
            .name("game-review".into())
            .spawn(move || {
                let result = game_review::run(config, &settings, targets, &stop, &mut |e| {
                    let _ = tx.send(Msg::Event(e));
                });
                let _ = tx.send(match result {
                    Ok(()) => Msg::Done,
                    Err(JobFailure::Start(e) | JobFailure::Engine(e)) => Msg::Failed(e),
                    Err(JobFailure::Terminated(reason)) => Msg::Failed(format!("{reason:?}")),
                });
            });
        if let Err(e) = spawned {
            st.failure = Some(e.to_string());
            return false;
        }
        st.job = Some(Job {
            rx,
            cancel,
            threads,
        });
        true
    }

    /// Stops the engine; what it has evaluated is kept (and cached).
    pub fn cancel(&self) {
        stop(&mut lock(&self.inner));
    }

    /// The state now, with what the engine has said since the last call.
    pub fn state(&self) -> ReviewState {
        let mut st = lock(&self.inner);
        drain(&mut st);
        let Inner { game, analysis, .. } = &mut *st;
        analysis.refresh(game);
        snapshot(&st)
    }
}

impl Drop for GameReview {
    fn drop(&mut self) {
        // Leaving the game mid-analysis keeps what was computed.
        stop(&mut lock(&self.inner));
    }
}

/// Ends the running job, keeping every result it has sent.
fn stop(st: &mut Inner) {
    if st.job.is_none() {
        return;
    }
    drain(st);
    if st.job.take().is_some() {
        save(st);
    }
}

fn drain(st: &mut Inner) {
    let Some(job) = &st.job else { return };
    let mut events = Vec::new();
    let mut ended = None;
    while let Ok(msg) = job.rx.try_recv() {
        match msg {
            Msg::Event(e) => events.push(e),
            Msg::Done => ended = Some(None),
            Msg::Failed(e) => ended = Some(Some(e)),
        }
    }
    let threads = u64::from(job.threads.max(1));
    let got = !events.is_empty();
    for e in events {
        if let JobEvent::Eval { nps, .. } = &e {
            st.done += 1;
            if *nps > 0 {
                st.nps_per_thread = nps / threads;
            }
        }
        st.analysis.apply(e);
    }
    if let Some(failure) = ended {
        st.job = None;
        st.failure = failure;
        save(st);
    } else if got && st.saved.elapsed() >= SAVE_EVERY {
        save(st);
    }
}

fn save(st: &mut Inner) {
    st.saved = Instant::now();
    if let Some(dir) = &st.cache_dir {
        // A cache that cannot be written only means analysing again next time.
        let _ = st.analysis.save_cache(dir);
    }
}

/// "+0.35", "−1.20", "#3", "#−2", "1-0".
fn eval_text(e: Eval) -> String {
    match e {
        Eval::Cp(cp) => pawns(cp),
        Eval::Mate { white, moves: 0 } => if white { "1-0" } else { "0-1" }.into(),
        Eval::Mate { white: true, moves } => format!("#{moves}"),
        Eval::Mate {
            white: false,
            moves,
        } => format!("#\u{2212}{moves}"),
    }
}

fn side(s: &SideStats) -> SideReview {
    SideReview {
        inaccuracies: s.inaccuracies,
        mistakes: s.mistakes,
        blunders: s.blunders,
        acpl: s.acpl,
        accuracy: s.accuracy,
        opening: s.phase_accuracy[0],
        middlegame: s.phase_accuracy[1],
        endgame: s.phase_accuracy[2],
    }
}

fn snapshot(st: &Inner) -> ReviewState {
    let a = &st.analysis;
    let place = |id: NodeId| st.places.get(&id).copied().unwrap_or(0);
    let line = a.line();
    let points = line
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let v = a.values().get(i).copied().flatten();
            ReviewPoint {
                node: place(*id),
                white_win: v.map(|e| e.win_percent(Color::White) as f32),
                eval: v.map(eval_text).unwrap_or_default(),
                judgement: i
                    .checked_sub(1)
                    .and_then(|p| a.report.judgements.get(p).copied().flatten())
                    .map(Into::into),
            }
        })
        .collect();
    let mut moves = Vec::new();
    for (i, id) in line.iter().enumerate().skip(1) {
        let Some(mark) = a.marks.get(id) else {
            continue;
        };
        let best = a.eval_at(i - 1).filter(|e| !e.pv.is_empty()).and_then(|e| {
            let root = st.game.position(mark.base)?;
            let (cp, mate) = match e.eval {
                Eval::Cp(cp) => (Some(cp), None),
                Eval::Mate { white, moves } => (
                    None,
                    Some(if white { moves as i32 } else { -(moves as i32) }),
                ),
            };
            Some(line_with(root, 1, cp, mate, &e.pv))
        });
        // The reply is the first move of the engine line after the bad move.
        let reply = a
            .eval_at(i)
            .and_then(|e| e.pv.first())
            .filter(|_| !a.refutation(*id, &st.game).is_empty())
            .filter(|u| u.len() >= 4)
            .map(|u| (u[0..2].to_owned(), u[2..4].to_owned()));
        moves.push(ReviewMove {
            node: place(*id),
            judgement: mark.judgement.into(),
            white: a.movers().get(i - 1) == Some(&Color::White),
            best,
            best_label: mark.best_label.clone().unwrap_or_default(),
            reply_from: reply.as_ref().map(|r| r.0.clone()),
            reply_to: reply.map(|r| r.1),
        });
    }
    let meta = a.meta.clone().unwrap_or_default();
    ReviewState {
        running: st.job.is_some(),
        done: st.done,
        total: st.total,
        failure: st.failure.clone(),
        has_data: a.has_data(),
        engine: meta.engine,
        nodes: meta.nodes,
        missing: a.missing() as u32,
        positions: a.positions() as u32,
        nps_per_thread: st.nps_per_thread,
        points,
        middlegame: a.report.phases.middlegame.map(|i| i as u32),
        endgame: a.report.phases.endgame.map(|i| i as u32),
        white: side(&a.report.white),
        black: side(&a.report.black),
        moves,
    }
}

#[cfg(test)]
mod tests {
    use crate::open_bytes;

    #[test]
    fn evaluations_of_the_pgn_make_a_report() {
        let pgn = b"[Event \"t\"]\n\n1. e4 {[%eval 0.3]} e5 {[%eval 0.32]} 2. Qh5 {[%eval -0.2]} \
            Ke7 {[%eval 6.5]} 3. Qxe5# 1-0\n"
            .to_vec();
        let db = open_bytes("t.pgn".into(), pgn).unwrap();
        let data = db.game(0).unwrap();
        let review = db.review(0, String::new()).unwrap();
        let s = review.state();
        assert!(s.has_data && !s.running);
        assert_eq!((s.engine.as_str(), s.nodes, s.missing), ("PGN", 0, 0));
        assert_eq!(s.points.len(), 6);
        assert_eq!(s.points[0].node, 0);
        assert_eq!(s.points.last().unwrap().eval, "1-0");
        assert_eq!(s.black.blunders, 1);
        assert_eq!(s.moves.len(), 1);
        let ke7 = &s.moves[0];
        assert!(!ke7.white);
        assert_eq!(data.nodes[ke7.node as usize].san, "Ke7");
        // Evaluations from the PGN have no lines: nothing to show on the board.
        assert!(ke7.best.is_none() && ke7.reply_from.is_none());
        assert!(s.white.accuracy.is_some());
        // Nothing to analyse: every position has an evaluation.
        assert!(!review.start("/nonexistent".into(), String::new(), 1, 16, 1000, false));
    }

    /// With the fake engine of the `uci` crate (`cargo build -p uci --bin fake_uci`).
    #[test]
    fn the_engine_job_runs_and_is_cached() {
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/debug/fake_uci");
        assert!(
            std::path::Path::new(fake).exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        let dir = std::env::temp_dir().join(format!("mobile-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.to_string_lossy().into_owned();
        let db = open_bytes("t.pgn".into(), b"1. e4 e5 2. Nf3 Nc6 *\n".to_vec()).unwrap();
        let review = db.review(0, cache.clone()).unwrap();
        assert!(!review.state().has_data);
        assert!(review.start(fake.into(), String::new(), 1, 16, 1000, false));
        let mut s = review.state();
        for _ in 0..200 {
            if !s.running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            s = review.state();
        }
        assert!(!s.running, "the job did not end");
        assert_eq!(s.failure, None);
        assert_eq!((s.done, s.total, s.missing), (5, 5, 0));
        assert!(s.points.iter().all(|p| p.white_win.is_some()));
        drop(review);
        // The next time the game opens, the results are there without the engine.
        let again = db.review(0, cache).unwrap().state();
        assert!(again.has_data && again.missing == 0 && again.nodes == 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_engine_is_reported() {
        let db = open_bytes("t.pgn".into(), b"1. e4 e5 *\n".to_vec()).unwrap();
        let review = db.review(0, String::new()).unwrap();
        assert!(review.start(
            "/nonexistent/engine".into(),
            String::new(),
            1,
            16,
            1000,
            false
        ));
        let mut s = review.state();
        for _ in 0..200 {
            if !s.running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            s = review.state();
        }
        assert!(!s.running);
        assert!(s.failure.is_some());
        assert!(!s.has_data);
    }
}
