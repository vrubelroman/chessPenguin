//! Our !! and ! against chess.com's on every game of `chesscom/` (see its README): run it
//! after each change of the rules, to see that a fix for one game broke no other.
//!
//! `cargo run --release -p game-review --example chesscom_check -- <engine>`
//!
//! The engine searches as in the program: the fixed search of the game analysis
//! (`game_review::SEARCH_NODES` nodes, one thread, a clear hash), in one engine process
//! per CPU. `<engine>` is ChessPenguin's Stockfish 19.
//!
//! `CP_REVIEW_NODES=N` searches N nodes instead, for choosing the program's nodes (the
//! program itself never does): its evaluations are cached apart.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chess_core::shakmaty::Color;
use chess_core::{Game, Nag};
use game_review::{GameAnalysis, JobSettings, Judgement, SEARCH_NODES, SEARCH_VERSION};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(engine), 2) = (args.get(1), args.len()) else {
        eprintln!("usage: chesscom_check <engine>");
        std::process::exit(2);
    };
    let nodes: u64 = std::env::var("CP_REVIEW_NODES")
        .map_or(SEARCH_NODES, |n| n.parse().expect("CP_REVIEW_NODES: a number"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Evaluations of one version of the fixed search are the same on every machine;
    // those of other nodes go apart.
    let folder = if nodes == SEARCH_NODES {
        format!("v{SEARCH_VERSION}")
    } else {
        format!("v{SEARCH_VERSION}-{nodes}")
    };
    let cache = root.join("../../target/chesscom-check").join(folder);
    println!("{nodes} nodes per search");
    std::fs::create_dir_all(&cache).expect("cache folder");

    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("chesscom"))
        .expect("chesscom/")
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "pgn"))
        .collect();
    files.sort();

    let (mut agree, mut differ) = (0, 0);
    // Per mark ("!!", "!"): chess.com's, found by us, ours chess.com has not.
    let mut tally = [Tally::default(), Tally::default()];
    for file in files {
        let text = std::fs::read_to_string(&file).expect("pgn");
        let game = chess_core::parse_str(&text).game;
        let mut a = GameAnalysis::new(&game, Some(&cache));
        review(&mut a, &game, engine, nodes);
        a.save_cache(&cache).expect("cache");
        let compared = game.tag("ChessComMarks").unwrap_or("!! !").to_owned();
        println!(
            "## {} — {} ({}; compared: {compared})",
            game.tag("White").unwrap_or("?"),
            game.tag("Black").unwrap_or("?"),
            file.file_name().unwrap().to_string_lossy(),
        );
        let line = a.line().to_vec();
        // Only chess.com's totals are known ("white !! 0 ! 1, black !! 2 ! 1"): compare
        // counts, list our marks, leave the game out of the shares.
        if let Some(counts) = game.tag("ChessComCounts") {
            let ours = |white: bool, j: Judgement| {
                a.report
                    .judgements
                    .iter()
                    .enumerate()
                    .filter(|(i, x)| **x == Some(j) && (a.movers()[*i] == Color::White) == white)
                    .count()
            };
            println!("   chess.com: {counts}");
            println!(
                "   ours:      white !! {} ! {}, black !! {} ! {}",
                ours(true, Judgement::Brilliant),
                ours(true, Judgement::Great),
                ours(false, Judgement::Brilliant),
                ours(false, Judgement::Great),
            );
            for (i, j) in a.report.judgements.iter().enumerate() {
                if let Some(j @ (Judgement::Brilliant | Judgement::Great)) = j {
                    let mark = if *j == Judgement::Brilliant { "!!" } else { "!" };
                    let label = game.move_label(line[i + 1]).unwrap_or_default();
                    println!("   ours  {label:<14} {mark}");
                }
            }
            continue;
        }
        for i in 0..line.len() - 1 {
            let node = line[i + 1];
            let theirs = game.node(node).map_or("", |n| mark_of_nags(n.nags()));
            let ours = match a.report.judgements[i] {
                Some(Judgement::Brilliant) => "!!",
                Some(Judgement::Great) => "!",
                _ => "",
            };
            // Only the marks chess.com was compared on: with "!!" alone our ! says nothing.
            let ours_compared = if ours == "!" && !compared.split(' ').any(|m| m == "!") {
                ""
            } else {
                ours
            };
            if theirs.is_empty() && ours_compared.is_empty() {
                continue;
            }
            for (k, mark) in ["!!", "!"].into_iter().enumerate() {
                let t = &mut tally[k];
                t.theirs += u32::from(theirs == mark);
                t.found += u32::from(theirs == mark && ours_compared == mark);
                t.extra += u32::from(ours_compared == mark && theirs != mark);
            }
            let label = game.move_label(node).unwrap_or_default();
            if theirs == ours_compared {
                agree += 1;
                println!("   ok    {label:<14} {theirs}");
            } else {
                differ += 1;
                println!(
                    "   DIFF  {label:<14} chess.com {:<3} ours {}",
                    or_dash(theirs),
                    or_dash(ours_compared)
                );
            }
        }
    }
    println!("\nagree {agree}, differ {differ}");
    for (t, mark) in tally.iter().zip(["!!", "!"]) {
        println!(
            "{mark:<2}  chess.com {:>3}  found {:>3} ({})  ours not chess.com's {:>3}  (ours {}, {} of them chess.com's)",
            t.theirs,
            t.found,
            percent(t.found, t.theirs),
            t.extra,
            t.found + t.extra,
            percent(t.found, t.found + t.extra),
        );
    }
    if differ > 0 {
        std::process::exit(1);
    }
}

/// Fills in what the cache lacks (all of it the first time) at `nodes`.
fn review(a: &mut GameAnalysis, game: &Game, engine: &str, nodes: u64) {
    let targets = a.begin(game, false);
    if let Some(m) = &mut a.meta {
        m.nodes = nodes;
    }
    if !targets.is_empty() {
        let settings = JobSettings {
            nodes,
            ..JobSettings::new(game_review::processes(game_review::MAX_PROCESSES), None)
        };
        let cancel = AtomicBool::new(false);
        game_review::run(
            uci::EngineConfig::new(engine),
            &settings,
            targets,
            &cancel,
            &mut |e| a.apply(e),
        )
        .expect("engine");
    }
    a.refresh(game);
}

fn mark_of_nags(nags: &[Nag]) -> &'static str {
    if nags.contains(&Nag::BRILLIANT_MOVE) {
        "!!"
    } else if nags.contains(&Nag::GOOD_MOVE) {
        "!"
    } else {
        ""
    }
}

#[derive(Default)]
struct Tally {
    theirs: u32,
    found: u32,
    extra: u32,
}

fn percent(part: u32, whole: u32) -> String {
    if whole == 0 {
        "—".into()
    } else {
        format!("{:.0} %", 100.0 * f64::from(part) / f64::from(whole))
    }
}

fn or_dash(mark: &str) -> &str {
    if mark.is_empty() { "—" } else { mark }
}
