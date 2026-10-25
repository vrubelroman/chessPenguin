//! Agreement with the original PyTorch model (tests/reference.json, written by
//! scripts/maia-reference.py): the same top moves with close probabilities. Needs the
//! weights: `MAIA_WEIGHTS=/path/maia3-5m.bin cargo test -p maia -- --ignored`.

use maia::Maia;
use shakmaty::fen::Fen;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess};

#[test]
#[ignore = "needs the weights file (MAIA_WEIGHTS)"]
fn matches_the_original_model() {
    let path = std::env::var("MAIA_WEIGHTS").expect("MAIA_WEIGHTS");
    let maia = Maia::load(std::path::Path::new(&path)).expect("weights");
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("reference.json")).expect("json");
    for case in cases.as_array().unwrap() {
        let start = case["start"].as_str().unwrap();
        let mut pos: Chess = if start == "startpos" {
            Chess::default()
        } else {
            start[4..]
                .parse::<Fen>()
                .unwrap()
                .into_position(CastlingMode::Standard)
                .unwrap()
        };
        let mut history = vec![pos.clone()];
        for m in case["moves"].as_array().unwrap() {
            let m = m
                .as_str()
                .unwrap()
                .parse::<UciMove>()
                .unwrap()
                .to_move(&pos)
                .unwrap();
            pos = maia::after(&pos, m);
            history.push(pos.clone());
        }
        if !case["history"].as_bool().unwrap() {
            history = vec![pos.clone()];
        }
        let t = std::time::Instant::now();
        let p = maia.predict(
            &history,
            case["self"].as_u64().unwrap() as u32,
            case["oppo"].as_u64().unwrap() as u32,
        );
        let ms = t.elapsed().as_millis();
        let ours: Vec<(String, f32)> = p
            .moves
            .iter()
            .take(5)
            .map(|(m, pr)| (m.to_uci(CastlingMode::Standard).to_string(), *pr))
            .collect();
        let want: Vec<(String, f32)> = case["top"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t[0].as_str().unwrap().to_owned(),
                    t[1].as_f64().unwrap() as f32,
                )
            })
            .collect();
        println!(
            "{start} {:?} hist={} {ms} ms\n  ours {ours:?}\n  want {want:?}\n  wdl {:?} want {:?}",
            case["moves"], case["history"], p.wdl, case["value"]
        );
        for (k, (m, pr)) in want.iter().enumerate().take(3) {
            assert_eq!(&ours[k].0, m, "move {k} of {start}");
            assert!((ours[k].1 - pr).abs() < 0.02, "{m}: {} vs {pr}", ours[k].1);
        }
    }
}
