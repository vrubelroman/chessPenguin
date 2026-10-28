//! Time of the fixed game analysis of one game, and a check that it is the same at any
//! number of engine processes: prints the time and a digest of every evaluation.
//!
//! `cargo run --release -p game-review --example review_bench -- <engine> <pgn> <processes>...`

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use game_review::{GameAnalysis, JobSettings};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: review_bench <engine> <pgn> <processes>...");
        std::process::exit(2);
    }
    let text = std::fs::read_to_string(&args[2]).expect("pgn");
    let game = chess_core::parse_str(&text).game;
    let mut digests = Vec::new();
    for p in &args[3..] {
        let processes: usize = p.parse().expect("processes");
        let mut a = GameAnalysis::new(&game, None);
        let targets = a.begin(&game, true);
        let positions = targets.len();
        let mut searches = 0;
        let started = Instant::now();
        game_review::run(
            uci::EngineConfig::new(&args[1]),
            &JobSettings::new(processes, None),
            targets,
            &AtomicBool::new(false),
            &mut |e| {
                if let game_review::JobEvent::Eval { eval, .. } = &e {
                    searches += 1 + usize::from(eval.second.is_some());
                }
                a.apply(e);
            },
        )
        .expect("engine");
        let secs = started.elapsed().as_secs_f64();
        // Every evaluation, line and second search, in the order of the game.
        let all: Vec<_> = (0..a.line().len()).map(|i| a.eval_at(i).cloned()).collect();
        let bytes = serde_json::to_vec(&all).expect("json");
        let digest = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        });
        println!(
            "processes {processes:>2}: {positions} positions, {searches} searches, {secs:.1} s, \
             evaluations {digest:016x}"
        );
        digests.push(digest);
    }
    if digests.windows(2).any(|w| w[0] != w[1]) {
        println!("DIFFERENT evaluations");
        std::process::exit(1);
    }
}
