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

use std::sync::atomic::AtomicBool;

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
    /// The answers, one per position of [`PROBE_POSITIONS`] (fewer when one gave none).
    pub answers: Vec<ProbeAnswer>,
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

/// Runs the probe searches in a process of the engine. Takes about a second (the
/// engine's start included); the engine is quit at the end.
pub fn probe(config: uci::EngineConfig) -> Result<Probe, JobFailure> {
    let handle = uci::Engine::spawn_with(config, Box::new(|| {}))
        .map_err(|e| JobFailure::Start(e.to_string()))?;
    let name = handle.info().name.clone();
    let _ = handle.set_threads(1);
    let _ = handle.set_hash_mb(SEARCH_HASH_MB);
    let _ = handle.set_multipv(1);
    let never = AtomicBool::new(false);
    let halted = || never.load(std::sync::atomic::Ordering::Relaxed);
    let mut answers = Vec::new();
    for (i, fen) in PROBE_POSITIONS.iter().enumerate() {
        let _ = handle.new_game();
        let pos = EnginePosition::fen(*fen, Vec::<String>::new());
        let id = uci::SearchId(i as u64 + 1);
        let answer = search_once(&handle, &pos, id, Vec::new(), PROBE_NODES, &halted)?;
        let Some((info, best)) = answer else { break };
        let (Some(best), Some(score), Some(nodes)) = (
            best.or_else(|| info.pv.first().cloned()),
            info.score,
            info.nodes,
        ) else {
            break;
        };
        answers.push(ProbeAnswer {
            best,
            score: score.score,
            nodes,
        });
    }
    handle.quit();
    Ok(Probe { name, answers })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let p = probe(engine(STOCKFISH_19, &REFERENCE)).expect("the probe runs");
        assert_eq!(p.name.as_deref(), Some(STOCKFISH_19));
        assert_eq!(p.answers.len(), 3);
        assert!(p.same(), "{p:?}");
    }

    #[test]
    fn other_answers_or_names_do_not() {
        // Another network or version: one score differs.
        let mut other = REFERENCE;
        other[1].1 = Score::Cp(-117);
        let p = probe(engine(STOCKFISH_19, &other)).expect("the probe runs");
        assert!(!p.same());
        // The nodes differ (another search), the move and score the same.
        let mut other = REFERENCE;
        other[2].2 += 1;
        assert!(!probe(engine(STOCKFISH_19, &other)).unwrap().same());
        // Another best move.
        let mut other = REFERENCE;
        other[0].0 = "g8f6";
        assert!(!probe(engine(STOCKFISH_19, &other)).unwrap().same());
        // The same answers under another name.
        let p = probe(engine("Stockfish 18", &REFERENCE)).unwrap();
        assert_eq!(p.name.as_deref(), Some("Stockfish 18"));
        assert!(!p.same());
        // An engine that answers nothing of the kind (the fake's own lines).
        let mut plain = uci::EngineConfig::new(fake());
        plain.args = vec!["--name".into(), STOCKFISH_19.into()];
        assert!(!probe(plain).unwrap().same());
    }

    /// A real Stockfish 19 gives [`REFERENCE`] (run when the reference is taken again
    /// or another build is checked):
    /// `CP_PROBE_ENGINE=/path/to/stockfish cargo test -p game-review probe -- --ignored`.
    #[test]
    #[ignore = "needs a Stockfish 19 in CP_PROBE_ENGINE"]
    fn a_real_stockfish_19_gives_the_reference() {
        let path = std::env::var("CP_PROBE_ENGINE").expect("CP_PROBE_ENGINE");
        let p = probe(uci::EngineConfig::new(path)).expect("the probe runs");
        println!("{p:?}");
        assert!(p.same(), "{p:?}");
    }

    #[test]
    fn an_engine_that_does_not_start_fails() {
        assert!(matches!(
            probe(uci::EngineConfig::new("/nonexistent/engine")),
            Err(JobFailure::Start(_))
        ));
    }
}
