//! Measures the position search of a PGN database: building the table and queries.
//!
//!     cargo run --release -p chess-import --example position_search -- "Book.pgn" [copies]
//!
//! `copies` > 1 joins that many copies of the database's games (a synthetic large
//! database).

use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use chess_core::PgnDatabase;
use chess_import::pgn_positions::PositionTable;
use chess_import::position::PositionQuery;
use shakmaty::san::San;
use shakmaty::{Chess, Position};

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<f64>().ok())
        .map_or(0.0, |pages| pages * 4096.0 / 1e6)
}

/// Peak resident memory of the process.
fn peak_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            let l = s.lines().find(|l| l.starts_with("VmHWM:"))?;
            l.split_whitespace().nth(1)?.parse::<f64>().ok()
        })
        .map_or(0.0, |kb| kb * 1024.0 / 1e6)
}

fn line(moves: &str) -> PositionQuery {
    let mut pos = Chess::default();
    let mut line = Vec::new();
    for s in moves.split_whitespace() {
        let m = s.parse::<San>().unwrap().to_move(&pos).unwrap();
        line.push(m);
        pos.play_unchecked(m);
    }
    PositionQuery::new(pos, Some(line))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: position_search <database> [copies]");
        std::process::exit(2);
    };
    let copies: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let never = AtomicBool::new(false);
    let opened =
        chess_import::open(std::path::Path::new(&path), &mut |_| {}, &never).expect("opens");
    let mut text = String::new();
    for i in 0..opened.db.len() {
        if let Some(t) = opened.db.raw_text(i) {
            text.push_str(t.trim_end());
            text.push_str("\n\n");
        }
    }
    let one = text.clone();
    for _ in 1..copies {
        text.push_str(&one);
    }
    drop(opened);
    let db = PgnDatabase::from_bytes(text.into_bytes(), None, &mut |_| {}, &never).unwrap();
    let before = rss_mb();
    let peak_before = peak_mb();
    let t0 = Instant::now();
    let table = PositionTable::build(&db, &AtomicUsize::new(0), &never).unwrap();
    println!(
        "{} games, {} positions; build {:.2?}; table {:.1} MB (RSS +{:.1} MB, peak +{:.1} MB)",
        table.games(),
        table.positions(),
        t0.elapsed(),
        table.memory() as f64 / 1e6,
        rss_mb() - before,
        peak_mb() - peak_before,
    );
    let queries = [
        "",
        "e4",
        "d4 Nf6 c4 g6",
        "e4 e5 Nf3 Nc6 Bb5 a6 Ba4 Nf6 O-O Be7",
        "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6",
    ];
    for q in queries {
        let q = line(q);
        let t0 = Instant::now();
        let runs = 20;
        let mut out = None;
        for _ in 0..runs {
            out = table.search(&db, None, &q, &never);
        }
        let out = out.unwrap();
        println!(
            "{:>4} matches, {:>2} moves: {:.2?}  [{}]",
            out.matches.len(),
            out.moves.len(),
            t0.elapsed() / runs,
            q.line.as_ref().map_or(0, |l| l.len()),
        );
    }
    // Candidates: every other game.
    let half: Vec<u32> = (0..db.len() as u32).step_by(2).collect();
    let q = line("e4");
    let t0 = Instant::now();
    let out = table.search(&db, Some(&half), &q, &never).unwrap();
    println!(
        "1.e4 among half the games: {} matches, {:.2?}",
        out.matches.len(),
        t0.elapsed()
    );
}
