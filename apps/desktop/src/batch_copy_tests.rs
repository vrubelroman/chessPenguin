use std::sync::atomic::{AtomicBool, AtomicUsize};

use chess_core::PgnDatabase;
use chess_core::pgn::store;

use super::*;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let path = std::env::temp_dir().join(format!("cp-batch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Files of the folder besides `keep` (a temporary file left behind shows here).
    fn others(&self, keep: &[&Path]) -> Vec<PathBuf> {
        std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| !keep.contains(&p.as_path()))
            .collect()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(path: &Path) -> Database {
    chess_import::open(path, &mut |_| {}, &AtomicBool::new(false))
        .unwrap()
        .db
}

/// Games with what a copy must keep: a start position with fullmove 0, variations three
/// deep, comments at the root and after moves, NAG, arrows, a tag of its own, and the
/// first game once more (a duplicate).
const SOURCE: &str = r#"[Event "Club"]
[Site "?"]
[Date "2020.01.02"]
[Round "1"]
[White "Alpha"]
[Black "Beta"]
[Result "1-0"]
[MyTag "kept"]

{A comment at the root} 1. e4 $1 {[%cal Ge2e4]} e5 (1... c5 2. Nf3 (2. c3 d5 (2... Nf6 3. e5)) d6) 2. Nf3 $14 Nc6 1-0

[Event "Study"]
[Site "?"]
[Date "2021.??.??"]
[Round "?"]
[White "Gamma"]
[Black "Delta"]
[Result "*"]
[SetUp "1"]
[FEN "4k3/8/8/8/8/8/4P3/4K3 w - - 0 0"]

1. e4 Kd7 {the king} 2. Kf2 *

[Event "Club"]
[Site "?"]
[Date "2019.05.05"]
[Round "2"]
[White "Beta"]
[Black "Alpha"]
[Result "0-1"]

1. d4 d5 2. c4 $2 dxc4 0-1

[Event "Club again"]
[Site "?"]
[Date "2020.01.02"]
[Round "1"]
[White "alpha"]
[Black "BETA"]
[Result "1-0"]

1. e4 e5 2. Nf3 Nc6 1-0
"#;

fn request(records: &[u32], target: &Path) -> CopyRequest {
    CopyRequest {
        records: records.to_vec(),
        sort: SortKey::Number,
        ascending: true,
        target: target.to_path_buf(),
        slow: false,
    }
}

fn copy(db: &Database, req: &CopyRequest) -> Option<Result<CopyReport, CopyError>> {
    copy_into_new(db, req, &AtomicUsize::new(0), &AtomicBool::new(false))
}

/// The new database as «Add to database» would make it: an empty file, then each record
/// appended in turn, a game already there skipped.
fn one_by_one(db: &Database, order: &[usize], target: &Path) {
    std::fs::write(target, b"").unwrap();
    for &i in order {
        let into = PgnDatabase::open(target, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let game = db.load_game(i).unwrap().game;
        if store::find_same_game(&into, &game).is_none() {
            store::append(&into, &record_text(db, i).unwrap()).unwrap();
        }
    }
}

#[test]
fn a_selection_keeps_a_bit_per_record() {
    let mut s = Selection::default();
    assert!(s.is_empty() && !s.contains(5));
    s.insert(5);
    s.insert(5);
    s.insert(200);
    assert_eq!((s.len(), s.records()), (2, vec![5, 200]));
    s.toggle(5);
    s.toggle(7);
    assert_eq!(s.records(), vec![7, 200]);
    s.extend(&[1, 7, 300]);
    assert_eq!((s.len(), s.records()), (4, vec![1, 7, 200, 300]));
    assert_eq!(s.count_in(&[0, 1, 2, 300]), 2);
    s.remove(1000);
    s.remove(7);
    assert_eq!(s.len(), 3);
    assert_eq!(s.last(), Some(300));
    s.retain_below(201);
    assert_eq!(s.records(), vec![1, 200]);
    s.only(42);
    assert_eq!(s.records(), vec![42]);
    s.clear();
    assert!(s.is_empty());
}

#[test]
fn a_copy_has_the_bytes_of_adding_the_games_one_by_one() {
    let dir = Dir::new("same");
    let source = dir.file("source.pgn", SOURCE.as_bytes());
    let before = std::fs::read(&source).unwrap();
    let db = open(&source);
    assert_eq!(db.len(), 4);
    let target = dir.0.join("copy.pgn");
    let report = copy(&db, &request(&[3, 0, 2, 1], &target))
        .unwrap()
        .unwrap();
    // Game 4 is game 1 again (names without case, the same date and moves).
    assert_eq!(
        report,
        CopyReport {
            copied: 3,
            duplicates: 1,
            unreadable: Vec::new()
        }
    );
    let expected = dir.0.join("expected.pgn");
    one_by_one(&db, &[0, 1, 2, 3], &expected);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        std::fs::read(&expected).unwrap()
    );
    // The source is only read; no temporary file is left.
    assert_eq!(std::fs::read(&source).unwrap(), before);
    assert!(dir.others(&[&source, &target, &expected]).is_empty());
    // Every game whole: tags, start position, variations, comments, NAG, arrows.
    let copied = open(&target);
    for (i, j) in [(0, 0), (1, 1), (2, 2)] {
        assert_eq!(
            copied.load_game(j).unwrap().game.to_pgn(),
            db.load_game(i).unwrap().game.to_pgn()
        );
    }
    let text = std::fs::read_to_string(&target).unwrap();
    for kept in [
        "[MyTag \"kept\"]",
        "{A comment at the root}",
        "$1",
        "[%cal Ge2e4]",
        "(2... Nf6 3. e5)",
        "4k3/8/8/8/8/8/4P3/4K3 w - - 0 0",
    ] {
        assert!(text.contains(kept), "{kept}");
    }
}

#[test]
fn a_copy_follows_the_tables_order() {
    let dir = Dir::new("order");
    let source = dir.file("source.pgn", SOURCE.as_bytes());
    let db = open(&source);
    let target = dir.0.join("copy.pgn");
    let req = CopyRequest {
        sort: SortKey::White,
        ascending: false,
        ..request(&[0, 1, 2], &target)
    };
    copy(&db, &req).unwrap().unwrap();
    let mut table = vec![0, 1, 2];
    db.sort_records(&mut table, SortKey::White, false);
    let whites: Vec<String> = (0..3)
        .map(|i| {
            open(&target)
                .load_game(i)
                .unwrap()
                .game
                .tag("White")
                .unwrap()
                .to_owned()
        })
        .collect();
    let wanted: Vec<String> = table
        .iter()
        .map(|&i| {
            db.load_game(i as usize)
                .unwrap()
                .game
                .tag("White")
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(whites, wanted);
    assert_eq!(whites, ["Gamma", "Beta", "Alpha"]);
}

#[test]
fn a_cancelled_copy_leaves_nothing() {
    let dir = Dir::new("cancel");
    let source = dir.file("source.pgn", SOURCE.as_bytes());
    let db = open(&source);
    let target = dir.0.join("copy.pgn");
    let cancel = AtomicBool::new(true);
    let got = copy_into_new(
        &db,
        &request(&[0, 1], &target),
        &AtomicUsize::new(0),
        &cancel,
    );
    assert!(got.is_none());
    assert!(!target.exists());
    assert!(dir.others(&[&source]).is_empty());

    // Cancelled while writing (another thread, a slow copy).
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let progress = std::sync::Arc::new(AtomicUsize::new(0));
    let (c, p) = (cancel.clone(), progress.clone());
    let req = CopyRequest {
        slow: true,
        ..request(&[0, 1, 2, 3], &target)
    };
    let worker = std::thread::spawn(move || copy_into_new(&db, &req, &p, &c));
    while progress.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        std::thread::yield_now();
    }
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(worker.join().unwrap().is_none());
    assert!(!target.exists());
    assert!(dir.others(&[&source]).is_empty());
}

#[test]
fn a_copy_never_replaces_a_file() {
    let dir = Dir::new("exists");
    let source = dir.file("source.pgn", SOURCE.as_bytes());
    let db = open(&source);
    let target = dir.file("copy.pgn", b"mine");
    let got = copy(&db, &request(&[0], &target)).unwrap();
    assert!(matches!(got, Err(CopyError::Exists)));
    assert_eq!(std::fs::read(&target).unwrap(), b"mine");
    assert!(dir.others(&[&source, &target]).is_empty());
}

#[test]
fn a_source_changed_while_copying_writes_nothing() {
    let dir = Dir::new("changed");
    let source = dir.file("source.pgn", SOURCE.as_bytes());
    let db = open(&source);
    std::fs::write(&source, format!("{SOURCE}\n[White \"x\"]\n\n1. e4 *\n")).unwrap();
    let target = dir.0.join("copy.pgn");
    let got = copy(&db, &request(&[0], &target)).unwrap();
    assert!(matches!(got, Err(CopyError::SourceChanged)));
    assert!(!target.exists());
    assert!(dir.others(&[&source]).is_empty());
}

#[test]
fn a_legacy_source_is_copied_as_its_text_reads() {
    let dir = Dir::new("legacy");
    // windows-1252: «é», «ü» and «—» are one byte each.
    let mut bytes = b"[Event \"Caf\xe9\"]\n[White \"M\xfcller\"]\n[Black \"Ren\xe9\"]\n[Result \"1-0\"]\n\n{Le d\xe9but \x97 bien} 1. e4 e5 1-0\n".to_vec();
    bytes.extend_from_slice(b"\n[White \"B\"]\n[Black \"C\"]\n[Result \"*\"]\n\n1. d4 *\n");
    let source = dir.file("legacy.pgn", &bytes);
    let db = open(&source);
    let target = dir.0.join("copy.pgn");
    copy(&db, &request(&[0, 1], &target)).unwrap().unwrap();
    let expected = dir.0.join("expected.pgn");
    one_by_one(&db, &[0, 1], &expected);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        std::fs::read(&expected).unwrap()
    );
    let copied = open(&target);
    let (a, b) = (
        db.load_game(0).unwrap().game,
        copied.load_game(0).unwrap().game,
    );
    assert_eq!(a.tag("White"), b.tag("White"));
    assert_eq!(a.to_pgn(), b.to_pgn());
    assert!(String::from_utf8(std::fs::read(&target).unwrap()).is_ok());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
}

/// windows-1251 bytes of `text` (ASCII and the Russian letters without «ё»).
fn cp1251(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            'А'..='я' => (c as u32 - 'А' as u32 + 0xC0) as u8,
            c => c as u8,
        })
        .collect()
}

#[test]
fn a_source_with_restored_letters_is_copied_as_adding_one_game() {
    let dir = Dir::new("repair");
    // `~~`, `o^` and `><` stand for ч, ю and Ч in old Russian files; restored on reading
    // (a copy has the restored text, as a game added alone; tasks.md, task 42, Д1).
    let comment = "{Клo^~~евые поля, ><ерные играo^т, ~~ерные} ".repeat(12);
    let text = format!(
        "[White \"Петров\"]\n[Black \"Иванов\"]\n[Result \"*\"]\n\n{comment}1. e4 *\n\n[White \"Б\"]\n[Black \"В\"]\n[Result \"*\"]\n\n{{~~ерные}} 1. d4 *\n"
    );
    let bytes = cp1251(&text);
    let source = dir.file("repair.pgn", &bytes);
    let db = open(&source);
    assert!(db.as_pgn().is_some_and(|p| p.repair_letters_enabled()));
    let target = dir.0.join("copy.pgn");
    copy(&db, &request(&[0, 1], &target)).unwrap().unwrap();
    let expected = dir.0.join("expected.pgn");
    one_by_one(&db, &[0, 1], &expected);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        std::fs::read(&expected).unwrap()
    );
    assert!(std::fs::read_to_string(&target).unwrap().contains("ерные"));
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
}

/// Copying 1000 games and choosing 4 million rows: timed (`cargo test -- --ignored
/// --nocapture`, release for figures).
#[test]
#[ignore]
fn timing() {
    let dir = Dir::new("timing");
    let mut text = String::new();
    for i in 0..1000 {
        text.push_str(&format!(
            "[Event \"E\"]\n[White \"W{i}\"]\n[Black \"B{i}\"]\n[Result \"*\"]\n\n1. e4 e5 2. Nf3 (2. f4 exf4 {{gambit}}) Nc6 3. Bb5 $1 a6 4. Ba4 Nf6 5. O-O Be7 *\n\n"
        ));
    }
    let source = dir.file("source.pgn", text.as_bytes());
    let db = open(&source);
    let target = dir.0.join("copy.pgn");
    let records: Vec<u32> = (0..1000).collect();
    let t = std::time::Instant::now();
    let report = copy(&db, &request(&records, &target)).unwrap().unwrap();
    println!(
        "copy 1000 games: {:?} ({} copied)",
        t.elapsed(),
        report.copied
    );
    let view: Vec<u32> = (0..4_000_000).collect();
    let t = std::time::Instant::now();
    let mut s = Selection::default();
    s.extend(&view);
    println!("select all of 4 000 000: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let n = s.count_in(&view);
    println!("count chosen in 4 000 000: {:?} ({n})", t.elapsed());
}
