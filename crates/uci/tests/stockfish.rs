//! Integration test with a real Stockfish binary. Ignored by default.
//!
//! Run: UCI_STOCKFISH=/path/to/stockfish cargo test -p uci --test stockfish -- --ignored

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use uci::{
    Engine, EngineEvent, EngineHandle, EnginePosition, Score, SearchId, SearchInfo, SearchLimits,
    Side, WhiteEval,
};

fn engine() -> EngineHandle {
    let path = std::env::var_os("UCI_STOCKFISH").expect("set UCI_STOCKFISH to the binary path");
    Engine::spawn(path, Vec::<String>::new(), Box::new(|| {})).expect("spawn stockfish")
}

/// Runs a finite search and returns (infos, bestmove).
fn run(
    h: &EngineHandle,
    pos: EnginePosition,
    id: SearchId,
    limits: SearchLimits,
) -> (Vec<SearchInfo>, Option<String>) {
    h.search(pos, id, limits).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut infos = Vec::new();
    while let Some(ev) = h.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match ev {
            EngineEvent::Info { search_id, info } => {
                assert_eq!(search_id, id);
                infos.push(info);
            }
            EngineEvent::BestMove {
                search_id, best, ..
            } => {
                assert_eq!(search_id, id);
                return (infos, best);
            }
            EngineEvent::Terminated(r) => panic!("terminated: {r:?}"),
            EngineEvent::Warning(w) => panic!("warning: {w}"),
            _ => {}
        }
    }
    panic!("no bestmove");
}

#[test]
#[ignore = "needs a real Stockfish binary in UCI_STOCKFISH"]
fn stockfish_multipv_depth_mate_and_position_switch() {
    let h = engine();
    let name = h.info().name.clone().unwrap_or_default();
    println!("engine: {name}; options: {}", h.info().options.len());
    assert!(name.starts_with("Stockfish"), "{name}");
    h.set_threads(1).unwrap();
    h.set_hash_mb(16).unwrap();
    h.set_multipv(3).unwrap();

    // Depth 10, MultiPV 3, Black to move after 1.e4.
    let t = Instant::now();
    let (infos, best) = run(
        &h,
        EnginePosition::startpos(["e2e4"]),
        SearchId(1),
        SearchLimits::Depth(10),
    );
    let mut last: BTreeMap<u32, &SearchInfo> = BTreeMap::new();
    for i in infos.iter().filter(|i| i.depth == Some(10)) {
        last.insert(i.multipv_index(), i);
    }
    assert_eq!(last.keys().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    for (k, i) in &last {
        let s = i.score.unwrap();
        println!(
            "multipv {k}: depth {:?} score {:?} white {:?} nodes {:?} pv {}",
            i.depth,
            s.score,
            s.to_white(Side::Black).eval,
            i.nodes,
            i.pv.join(" ")
        );
        assert!(!i.pv.is_empty());
        assert!(matches!(s.score, Score::Cp(_)));
    }
    assert!(best.is_some());
    assert_eq!(best.as_deref(), Some(last[&1].pv[0].as_str()));
    println!("depth 10 in {:?}, bestmove {best:?}", t.elapsed());

    // Mate in 1 for White (Rd8#).
    h.set_multipv(1).unwrap();
    let (infos, best) = run(
        &h,
        EnginePosition::fen(
            "6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1",
            Vec::<String>::new(),
        ),
        SearchId(2),
        SearchLimits::Depth(5),
    );
    let s = infos.last().unwrap().score.unwrap();
    assert_eq!(s.score, Score::Mate(1));
    assert_eq!(
        s.to_white(Side::White).eval,
        WhiteEval::Mate {
            winner: Side::White,
            moves: 1
        }
    );
    assert_eq!(best.as_deref(), Some("d1d8"));

    // Checkmated side to move: no legal moves.
    let (infos, best) = run(
        &h,
        EnginePosition::fen(
            "3R2k1/5ppp/8/8/8/8/5PPP/6K1 b - - 1 1",
            Vec::<String>::new(),
        ),
        SearchId(3),
        SearchLimits::Depth(5),
    );
    println!("mated position infos: {infos:?}, best {best:?}");
    assert_eq!(best, None);
    if let Some(s) = infos.last().and_then(|i| i.score) {
        assert_eq!(s.score, Score::Mate(0));
    }

    // Infinite analysis with rapid position switches.
    h.set_multipv(2).unwrap();
    let positions = [
        vec!["e2e4"],
        vec!["e2e4", "e7e5"],
        vec!["d2d4"],
        vec!["d2d4", "g8f6"],
        vec!["c2c4"],
    ];
    for (i, moves) in positions.iter().enumerate() {
        h.analyze(
            EnginePosition::startpos(moves.clone()),
            SearchId(10 + i as u64),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(30));
    }
    let last_id = SearchId(10 + positions.len() as u64 - 1);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut current = None;
    let mut last_infos = 0;
    while let Some(ev) = h.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match ev {
            EngineEvent::SearchStarted { search_id } => current = Some(search_id),
            EngineEvent::Info { search_id, info } => {
                assert_eq!(Some(search_id), current);
                // After 1.c4 Black is to move; PV must start with a Black move
                // (rank 7/8 origin) - a crude cross-position check.
                if search_id == last_id {
                    let from_rank = info.pv[0].as_bytes()[1];
                    assert!(from_rank == b'7' || from_rank == b'8', "{:?}", info.pv);
                    last_infos += 1;
                    if last_infos >= 20 {
                        break;
                    }
                }
            }
            EngineEvent::BestMove { search_id, .. } => {
                assert_eq!(Some(search_id), current);
                current = None;
            }
            EngineEvent::Terminated(r) => panic!("{r:?}"),
            _ => {}
        }
    }
    assert!(last_infos >= 20, "too little output for the last position");
    let pid = h.pid();
    let status = h.quit().expect("reaped");
    println!("quit status: {status:?}");
    assert!(status.success());
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}
