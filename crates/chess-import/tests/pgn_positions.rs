//! Position search over the real databases in `pgnFilesforExample/` (not in the
//! repository: the tests skip what is missing), checked against a brute force walk
//! of every game tree.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use chess_core::NodeId;
use chess_import::pgn_positions::PositionTable;
use chess_import::position::{PositionQuery, key};

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pgnFilesforExample")
}

const FILES: [&str; 9] = [
    "The Ruy Lopez - Move by Move.pgn",
    "Дворецкий. Учебник жндшпиля.pgn",
    "10 Great Ways to Get Better at Chess [Everyman 2010] - Davies Nigel.7z",
    "A Ferocious Opening Repertoire [Everyman 2011] - Lakdawala Cyrus.7z",
    "Attacking Chess - The King's Indian - Volume 1 [Everyman 2010] - Vigorito David.7z",
    "Chess Developments - The Modern Benoni [Everyman 2011] - Palliser Richard.7z",
    "Rubinstein russian.cbv",
    "Anand 01-14.cbv",
    "Endgame Manual.cbv",
];

#[test]
fn search_agrees_with_walking_every_game() {
    let never = AtomicBool::new(false);
    for name in FILES {
        let path = samples().join(name);
        if !path.exists() {
            eprintln!("skipped: {} not found", path.display());
            continue;
        }
        let opened = chess_import::open(&path, &mut |_| {}, &never).expect("opens");
        let db = opened.db.as_pgn().expect("a small database is PGN");
        let t0 = Instant::now();
        let table = PositionTable::build(db, &AtomicUsize::new(0), &never).expect("built");
        let built = t0.elapsed();

        // Per game: keys of all positions and of the main line positions; some
        // positions to ask for (main line and variation nodes).
        let mut all: Vec<HashSet<u64>> = Vec::new();
        let mut main: Vec<HashSet<u64>> = Vec::new();
        let mut queries = Vec::new();
        for i in 0..db.len() {
            let game = db.load_game(i).expect("game").game;
            let nodes = game.attached_nodes();
            all.push(
                nodes
                    .iter()
                    .map(|&n| key(game.position(n).unwrap()))
                    .collect(),
            );
            let mut line = vec![NodeId::ROOT];
            line.extend(game.mainline());
            main.push(
                line.iter()
                    .map(|&n| key(game.position(n).unwrap()))
                    .collect(),
            );
            if i % 7 == 0 {
                for n in nodes.iter().step_by(23) {
                    queries.push(game.position(*n).unwrap().clone());
                }
            }
        }
        let mut searching = std::time::Duration::ZERO;
        let mut found = 0;
        for pos in &queries {
            let q = PositionQuery::new(pos.clone(), None);
            let t0 = Instant::now();
            let out = table.search(db, None, &q, &never).expect("searched");
            searching += t0.elapsed();
            let k = key(pos);
            let expected: Vec<u32> = (0..all.len() as u32)
                .filter(|&i| all[i as usize].contains(&k))
                .collect();
            let got: Vec<u32> = out.matches.iter().map(|m| m.index).collect();
            assert_eq!(got, expected, "{name}: {pos:?}");
            for m in &out.matches {
                assert_eq!(m.in_variation, !main[m.index as usize].contains(&k));
            }
            // Every move of the statistics is played by at most the matching games.
            for s in &out.moves {
                assert!(s.tally.games as usize <= got.len());
                assert!(s.main_line_games <= s.tally.games);
            }
            found += got.len();
        }
        let per_query = searching / queries.len().max(1) as u32;
        eprintln!(
            "{name}: {} games, {} positions, {:.1} MB; build {built:.1?}; \
             {} queries, {per_query:.1?} each, {found} matches",
            table.games(),
            table.positions(),
            table.memory() as f64 / 1e6,
            queries.len(),
        );
        assert_eq!(table.games(), db.len());
    }
}
