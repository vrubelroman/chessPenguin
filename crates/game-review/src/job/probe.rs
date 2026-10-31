//! Whether an engine is the Stockfish 19 of the game analysis: its search gives the same
//! evaluations as ChessPenguin's, so a user's own Stockfish 19 can do the analysis and
//! the marks stay the same on every computer.
//!
//! Its name is not enough (a build of another commit, another network or a patched one
//! says the same). A few short searches in the fixed mode of the analysis — one
//! thread, a hash of [`SEARCH_HASH_MB`], `ucinewgame` before each, [`PROBE_NODES`]
//! nodes — are compared with those of ChessPenguin's Stockfish 19 ([`REFERENCE`]): the
//! best move, the score and the nodes of the line taken must all be the same. Stockfish
//! with one thread and a node limit is deterministic, whatever the CPU's instruction
//! set: its own check of the universal build (`scripts/check_universal.sh` of the
//! release) requires the same `bench` node count from every per-CPU build.

use std::time::{Duration, Instant};

use uci::{EnginePosition, Score};

use super::engines::search_once;
use super::{JobFailure, SEARCH_HASH_MB};

/// Nodes of each probe search (about 0.1 s each).
pub const PROBE_NODES: u64 = 100_000;

/// The name Stockfish 19 gives itself (`id name`).
pub const STOCKFISH_19: &str = "Stockfish 19";

/// What a probe search gives: the best move, the score of the line taken (the side to
/// move's) and its nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeAnswer {
    pub best: String,
    pub score: Score,
    pub nodes: u64,
}

/// The positions of the probe: an opening, a middlegame full of tactics, an endgame.
pub const PROBE_POSITIONS: [&str; 3] = [
    "r1bqkbnr/pppp1ppp/2n5/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
];

/// ChessPenguin's Stockfish 19 (the official `stockfish-linux-x86-64-universal`, sha256
/// of its archive in `engine_download`) on [`PROBE_POSITIONS`], taken 2026-10-31. The
/// same from its x86-64-avxvnni slice and from builds of its source for x86-64 (SSE2)
/// and x86-64-bmi2.
pub const REFERENCE: [(&str, Score, u64); 3] = [
    ("a7a6", Score::Cp(-36), 76_994),
    ("e2a6", Score::Cp(-196), 49_328),
    ("b4f4", Score::Cp(176), 38_955),
];

/// What the probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The engine's name (`id name`).
    pub name: Option<String>,
    /// The answers, one per position of [`PROBE_POSITIONS`] (fewer when one gave none,
    /// took too long, or the engine is not called Stockfish 19: then none is searched).
    pub answers: Vec<ProbeAnswer>,
    /// A search took longer than allowed (an engine that ignores `go nodes`).
    pub timed_out: bool,
}

impl Probe {
    /// Whether the engine is the Stockfish 19 of the analysis: its name and every answer.
    pub fn same(&self) -> bool {
        self.name.as_deref() == Some(STOCKFISH_19)
            && self.answers.len() == REFERENCE.len()
            && self
                .answers
                .iter()
                .zip(REFERENCE)
                .all(|(a, (best, score, nodes))| {
                    a.best == best && a.score == score && a.nodes == nodes
                })
    }
}

/// How long the probe may take: a search (Stockfish needs about 0.1 s) and all of it.
#[derive(Clone, Copy, Debug)]
pub struct ProbeLimits {
    pub search: Duration,
    pub total: Duration,
}

impl Default for ProbeLimits {
    fn default() -> Self {
        ProbeLimits {
            search: Duration::from_secs(10),
            total: Duration::from_secs(30),
        }
    }
}

/// What [`REFERENCE`] is a reference of — the positions, the nodes, the hash, the
/// answers — as a short text kept with the verdicts: a verdict of another probe is
/// not trusted. (FNV-1a of their text: the same in every build.)
pub fn reference_key() -> String {
    let text = format!("{PROBE_POSITIONS:?}|{PROBE_NODES}|{SEARCH_HASH_MB}|{REFERENCE:?}");
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// Runs the probe in a process of the engine; `halted` gives it up (the program
/// quits). An engine not called Stockfish 19 is not searched at all. The engine is
/// quit at the end (killed if it does not go).
pub fn probe(
    config: uci::EngineConfig,
    limits: ProbeLimits,
    halted: &dyn Fn() -> bool,
) -> Result<Probe, JobFailure> {
    let started = Instant::now();
    let handle = uci::Engine::spawn_with(config, Box::new(|| {}))
        .map_err(|e| JobFailure::Start(e.to_string()))?;
    let mut probe = Probe {
        name: handle.info().name.clone(),
        answers: Vec::new(),
        timed_out: false,
    };
    if probe.name.as_deref() != Some(STOCKFISH_19) {
        handle.quit();
        return Ok(probe);
    }
    let _ = handle.set_threads(1);
    let _ = handle.set_hash_mb(SEARCH_HASH_MB);
    let _ = handle.set_multipv(1);
    let over = || halted() || started.elapsed() > limits.total;
    for (i, fen) in PROBE_POSITIONS.iter().enumerate() {
        let _ = handle.new_game();
        let pos = EnginePosition::fen(*fen, Vec::<String>::new());
        let id = uci::SearchId(i as u64 + 1);
        let search_started = Instant::now();
        let answer = search_once(
            &handle,
            &pos,
            id,
            Vec::new(),
            PROBE_NODES,
            limits.search,
            &over,
        )?;
        if search_started.elapsed() > limits.search || started.elapsed() > limits.total {
            probe.timed_out = true;
            break;
        }
        let Some((info, best)) = answer else { break };
        let (Some(best), Some(score), Some(nodes)) = (
            best.or_else(|| info.pv.first().cloned()),
            info.score,
            info.nodes,
        ) else {
            break;
        };
        probe.answers.push(ProbeAnswer {
            best,
            score: score.score,
            nodes,
        });
    }
    handle.quit();
    Ok(probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(config: uci::EngineConfig) -> Result<Probe, JobFailure> {
        probe(config, ProbeLimits::default(), &|| false)
    }

    fn fake() -> &'static str {
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/debug/fake_uci");
        assert!(
            std::path::Path::new(fake).exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        fake
    }

    /// A fake engine answering every probe position as `answers` say.
    fn engine(name: &str, answers: &[(&str, Score, u64)]) -> uci::EngineConfig {
        let mut config = uci::EngineConfig::new(fake());
        config.args = vec!["--name".into(), name.into()];
        for (fen, (best, score, nodes)) in PROBE_POSITIONS.iter().zip(answers) {
            let score = match score {
                Score::Cp(cp) => format!("cp {cp}"),
                Score::Mate(n) => format!("mate {n}"),
            };
            let marker = fen.split(' ').next().unwrap_or_default();
            config.args.extend([
                "--answer".into(),
                marker.into(),
                format!("{score} nodes {nodes} pv {best}").into(),
            ]);
        }
        config
    }

    #[test]
    fn the_reference_answers_pass() {
        let p = run(engine(STOCKFISH_19, &REFERENCE)).expect("the probe runs");
        assert_eq!(p.name.as_deref(), Some(STOCKFISH_19));
        assert_eq!(p.answers.len(), 3);
        assert!(p.same(), "{p:?}");
    }

    #[test]
    fn other_answers_or_names_do_not() {
        // Another network or version: one score differs.
        let mut other = REFERENCE;
        other[1].1 = Score::Cp(-117);
        let p = run(engine(STOCKFISH_19, &other)).expect("the probe runs");
        assert!(!p.same());
        // The nodes differ (another search), the move and score the same.
        let mut other = REFERENCE;
        other[2].2 += 1;
        assert!(!run(engine(STOCKFISH_19, &other)).unwrap().same());
        // Another best move.
        let mut other = REFERENCE;
        other[0].0 = "g8f6";
        assert!(!run(engine(STOCKFISH_19, &other)).unwrap().same());
        // The same answers under another name.
        let p = run(engine("Stockfish 18", &REFERENCE)).unwrap();
        assert_eq!(p.name.as_deref(), Some("Stockfish 18"));
        assert!(!p.same());
        // An engine that answers nothing of the kind (the fake's own lines).
        let mut plain = uci::EngineConfig::new(fake());
        plain.args = vec!["--name".into(), STOCKFISH_19.into()];
        assert!(!run(plain).unwrap().same());
    }

    /// A real Stockfish 19 gives [`REFERENCE`] (run when the reference is taken again
    /// or another build is checked):
    /// `CP_PROBE_ENGINE=/path/to/stockfish cargo test -p game-review probe -- --ignored`.
    #[test]
    #[ignore = "needs a Stockfish 19 in CP_PROBE_ENGINE"]
    fn a_real_stockfish_19_gives_the_reference() {
        let path = std::env::var("CP_PROBE_ENGINE").expect("CP_PROBE_ENGINE");
        let p = run(uci::EngineConfig::new(path)).expect("the probe runs");
        println!("{p:?}");
        assert!(p.same(), "{p:?}");
    }

    /// An engine not called Stockfish 19 is not searched (Rodent searches on past
    /// `go nodes`): its name is the verdict, at once.
    #[test]
    fn another_engine_is_not_searched() {
        let mut config = uci::EngineConfig::new(fake());
        config.args = vec![
            "--name".into(),
            "Rodent V 1.2".into(),
            "--ignore-nodes".into(),
        ];
        let started = Instant::now();
        let p = run(config).expect("the probe runs");
        assert_eq!(p.name.as_deref(), Some("Rodent V 1.2"));
        assert!(p.answers.is_empty() && !p.timed_out && !p.same());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A «Stockfish 19» that ignores `go nodes`: stopped by the guard, not the same.
    #[test]
    fn an_endless_search_is_cut_short() {
        let mut config = uci::EngineConfig::new(fake());
        config.args = vec![
            "--name".into(),
            STOCKFISH_19.into(),
            "--ignore-nodes".into(),
        ];
        let limits = ProbeLimits {
            search: Duration::from_millis(300),
            total: Duration::from_secs(2),
        };
        let started = Instant::now();
        let p = probe(config, limits, &|| false).expect("the probe runs");
        assert!(p.timed_out && !p.same(), "{p:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        // Halted from outside (the program quits): at once.
        let mut config = uci::EngineConfig::new(fake());
        config.args = vec![
            "--name".into(),
            STOCKFISH_19.into(),
            "--ignore-nodes".into(),
        ];
        let started = Instant::now();
        let p = probe(config, ProbeLimits::default(), &|| true).expect("the probe runs");
        assert!(!p.same());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_reference_key_is_stable() {
        assert_eq!(reference_key(), reference_key());
        assert_eq!(reference_key().len(), 16);
    }

    #[test]
    fn an_engine_that_does_not_start_fails() {
        assert!(matches!(
            run(uci::EngineConfig::new("/nonexistent/engine")),
            Err(JobFailure::Start(_))
        ));
    }
}
