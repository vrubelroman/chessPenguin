use std::sync::Arc;

use game_review::CacheDir;

use super::*;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let path = std::env::temp_dir().join(format!("cp-brilliant-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("cache")).unwrap();
        Dir(path)
    }

    fn cache(&self) -> PathBuf {
        self.0.join("cache")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The fake engine of the `uci` crate (`cargo build -p uci --bin fake_uci`).
fn fake() -> PathBuf {
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fake_uci");
    assert!(
        fake.exists(),
        "build the fake engine first: cargo build -p uci --bin fake_uci"
    );
    fake
}

fn engine_at(path: &Path) -> Engine {
    Engine {
        config: uci::EngineConfig {
            // The second search of !! and ! (`searchmoves`), as Stockfish makes it.
            args: vec!["--searchmoves".into()],
            ..uci::EngineConfig::new(path)
        },
        settings: JobSettings::new(2, None),
        fits: Box::new(|_| true),
        pause: std::time::Duration::ZERO,
    }
}

/// An engine that cannot start: a pass that asks for it fails.
fn no_engine(dir: &Dir) -> Engine {
    engine_at(&dir.0.join("no-such-engine"))
}

const GAMES: &str = "[White \"Carlsen\"]\n[Black \"Kasparov\"]\n[Result \"*\"]\n\n1. e4 e5 2. Nf3 Nc6 *\n\n\
[White \"Kasparov\"]\n[Black \"Carlsen\"]\n[Result \"*\"]\n\n1. d4 d5 2. c4 *\n\n\
[White \"Anand\"]\n[Black \"Topalov\"]\n[Result \"*\"]\n\n1. e4 -- 2. d4 *\n\n\
[White \"Anand\"]\n[Black \"Kramnik\"]\n[Result \"*\"]\n\n*\n";

fn source(dir: &Dir) -> (PathBuf, Database) {
    let path = dir.0.join("games.pgn");
    std::fs::write(&path, GAMES).unwrap();
    let db = chess_import::open(&path, &mut |_| {}, &AtomicBool::new(false))
        .unwrap()
        .db;
    (path, db)
}

fn pass(
    db: &Database,
    records: &[u32],
    index: &Mutex<Index>,
    dir: &Dir,
    engine: &Engine,
) -> PassEnd {
    let cache = dir.cache();
    run_pass(
        db,
        records,
        index,
        &CacheDir(&cache),
        engine,
        &Progress::default(),
        &AtomicBool::new(false),
    )
}

#[test]
fn the_limit_is_one_hundred_games_inclusive() {
    assert_eq!(can_start(0), Err(Blocked::NoGames));
    assert_eq!(can_start(1), Ok(()));
    assert_eq!(can_start(LIMIT), Ok(()));
    assert_eq!(LIMIT, 100);
    assert_eq!(can_start(101), Err(Blocked::TooMany));
}

#[test]
fn a_pass_analyses_keeps_the_index_and_takes_the_cache_next_time() {
    let dir = Dir::new("pass");
    let (path, db) = source(&dir);
    let before = std::fs::read(&path).unwrap();
    let file = dir.0.join("marks.json");
    let index = Mutex::new(Index::load(file.clone()));
    let end = pass(&db, &[0, 1, 2, 3], &index, &dir, &engine_at(&fake()));
    assert_eq!(end.outcome, Outcome::Finished);
    assert_eq!(end.reviewed, vec![0, 1]);
    assert_eq!(end.from_cache, 0);
    assert_eq!(end.skipped, vec![(2, Skip::NullMove), (3, Skip::NoMoves)]);
    // The index lives on: read anew (as after a restart) it has every record.
    let again = Index::load(file.clone());
    assert_eq!(again.len(), 4);
    let entry = again.get(0).unwrap();
    let game = db.load_game(0).unwrap().game;
    assert_eq!(entry.line, GameAnalysis::new(&game, None).cache_key());
    assert_eq!(again.get(2).unwrap().skip, Some(Skip::NullMove));
    // Again: from the cache, the engine is not asked (there is none now).
    let index = Mutex::new(again);
    let end = pass(&db, &[1, 0], &index, &dir, &no_engine(&dir));
    assert_eq!(end.outcome, Outcome::Finished);
    assert_eq!((end.reviewed, end.from_cache), (vec![1, 0], 2));
    // The source is only read.
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn an_analysis_of_the_settings_of_before_counts_as_made() {
    let dir = Dir::new("stale");
    let (_, db) = source(&dir);
    let index = Mutex::new(Index::load(dir.0.join("marks.json")));
    pass(&db, &[0], &index, &dir, &engine_at(&fake()));
    // The cached analysis made «before the fixed search».
    for f in std::fs::read_dir(dir.cache()).unwrap() {
        let f = f.unwrap().path();
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        v["meta"]["search"] = serde_json::json!(0);
        std::fs::write(&f, serde_json::to_vec(&v).unwrap()).unwrap();
    }
    let end = pass(&db, &[0], &index, &dir, &no_engine(&dir));
    assert_eq!((end.outcome, end.from_cache), (Outcome::Finished, 1));
    assert!(index.lock().unwrap().get(0).unwrap().stale);
}

#[test]
fn a_stopped_pass_keeps_what_it_analysed() {
    let dir = Dir::new("stop");
    let (_, db) = source(&dir);
    let file = dir.0.join("marks.json");
    let index = Mutex::new(Index::load(file.clone()));
    // Stopped when the second game's engine starts: the first one is kept.
    let cancel = Arc::new(AtomicBool::new(false));
    let starts = Arc::new(AtomicUsize::new(0));
    let (c, s) = (cancel.clone(), starts.clone());
    let engine = Engine {
        fits: Box::new(move |_| {
            if s.fetch_add(1, Ordering::Relaxed) >= 1 {
                c.store(true, Ordering::Relaxed);
            }
            true
        }),
        ..engine_at(&fake())
    };
    let cache = dir.cache();
    let end = run_pass(
        &db,
        &[0, 1],
        &index,
        &CacheDir(&cache),
        &engine,
        &Progress::default(),
        &cancel,
    );
    assert_eq!(end.outcome, Outcome::Stopped);
    assert_eq!(end.reviewed, vec![0]);
    let kept = Index::load(file);
    assert!(kept.get(0).is_some() && kept.get(1).is_none());
}

#[test]
fn starting_and_stopping_at_once_never_breaks_the_index() {
    let dir = Dir::new("race");
    let (_, db) = source(&dir);
    let db = Arc::new(db);
    let file = dir.0.join("marks.json");
    for round in 0..10 {
        let index = Arc::new(Mutex::new(Index::load(file.clone())));
        let cancel = Arc::new(AtomicBool::new(false));
        let (d, i, c, cache) = (db.clone(), index.clone(), cancel.clone(), dir.cache());
        let worker = std::thread::spawn(move || {
            run_pass(
                &d,
                &[0, 1, 2, 3],
                &i,
                &CacheDir(&cache),
                &engine_at(&fake()),
                &Progress::default(),
                &c,
            )
        });
        if round % 2 == 0 {
            std::thread::yield_now();
        }
        cancel.store(true, Ordering::Relaxed);
        let end = worker.join().unwrap();
        assert!(matches!(end.outcome, Outcome::Stopped | Outcome::Finished));
        // The file is whole: it reads.
        if file.exists() {
            let bytes = std::fs::read(&file).unwrap();
            assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
        }
    }
}

#[test]
fn a_broken_index_is_an_empty_one() {
    let dir = Dir::new("broken");
    let file = dir.0.join("marks.json");
    std::fs::write(&file, b"{not json").unwrap();
    assert_eq!(Index::load(file.clone()).len(), 0);
    std::fs::write(&file, br#"{"version":99,"records":[]}"#).unwrap();
    assert_eq!(Index::load(file).len(), 0);
    assert_eq!(Index::load(dir.0.join("none.json")).len(), 0);
}

fn side(brilliant: u32, great: u32, blunders: u32, acpl: u32, accuracy: f64) -> SideMarks {
    SideMarks {
        counts: Counts {
            brilliant,
            great,
            blunders,
            ..Counts::default()
        },
        acpl: Some(acpl),
        accuracy: Some(accuracy),
    }
}

fn found(ply: u16, white: bool, number: u32, san: &str, brilliant: bool) -> Found {
    Found {
        ply,
        white,
        number,
        san: san.into(),
        brilliant,
    }
}

#[test]
fn totals_count_only_the_filters_players_moves() {
    let mut index = Index::default();
    let entry = |white, black, found| Entry {
        line: String::new(),
        white,
        black,
        found,
        stale: false,
        skip: None,
    };
    index.set(
        0,
        entry(
            side(1, 0, 0, 20, 90.0),
            side(0, 2, 1, 40, 70.0),
            vec![
                found(40, true, 21, "Rxf7", true),
                found(13, false, 7, "Nd5", false),
            ],
        ),
    );
    index.set(
        1,
        entry(
            side(0, 0, 3, 60, 60.0),
            side(1, 1, 0, 10, 96.0),
            vec![found(67, false, 34, "Nf3+", true)],
        ),
    );
    index.set(
        2,
        Entry {
            skip: Some(Skip::Chess960),
            ..entry(SideMarks::default(), SideMarks::default(), Vec::new())
        },
    );
    let all = totals(&index, &[0, 1, 2], &Sides::default());
    assert_eq!(
        (all.games, all.sides, all.with_brilliant, all.with_great),
        (2, 4, 2, 2)
    );
    assert_eq!(
        all.counts,
        Counts {
            brilliant: 2,
            great: 3,
            blunders: 4,
            ..Counts::default()
        }
    );
    assert_eq!(all.acpl, Some(32.5));
    assert_eq!(all.accuracy_range, Some((60.0, 96.0)));
    assert_eq!(all.brilliant.len(), 2);
    // The player had White in game 0 and Black in game 1: only those sides count.
    let player = Sides {
        player: Some((vec![0], vec![1])),
    };
    let t = totals(&index, &[0, 1, 2], &player);
    assert_eq!(
        (t.games, t.sides, t.with_brilliant, t.with_great),
        (2, 2, 2, 1)
    );
    assert_eq!(t.counts.blunders, 0);
    assert_eq!(t.acpl, Some(15.0));
    assert_eq!(t.accuracy, Some(93.0));
    assert_eq!(
        t.brilliant
            .iter()
            .map(|(r, f)| (*r, f.label()))
            .collect::<Vec<_>>(),
        [(0, "21.Rxf7!!".to_owned()), (1, "34…Nf3+!!".to_owned())]
    );
    // A player found on neither side is said.
    let nobody = Sides {
        player: Some((Vec::new(), Vec::new())),
    };
    assert_eq!(totals(&index, &[0, 1], &nobody).player_missing, vec![0, 1]);
}

#[test]
fn the_players_sides_come_from_the_filter() {
    let dir = Dir::new("sides");
    let (_, db) = source(&dir);
    let filter = chess_import::chessbase::base::Filter {
        player: chess_import::chessbase::base::NameFilter {
            text: "carlsen".into(),
            exact: None,
        },
        ..Default::default()
    };
    let sides = player_sides(&db, &filter, &AtomicBool::new(false));
    assert_eq!(sides.player, Some((vec![0], vec![1])));
    assert!(
        player_sides(&db, &Default::default(), &AtomicBool::new(false))
            .player
            .is_none()
    );
}
