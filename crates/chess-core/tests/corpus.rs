//! Runs the user's real PGN files from `pgnFilesforExample/` if present
//! (skipped otherwise; the files are not part of the fixtures).
//! Run with `cargo test -p chess-core --test corpus -- --nocapture` for stats.

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chess_core::shakmaty::Position;
use chess_core::{DiagnosticCode, Game, GameEntry, PgnDatabase, Severity, TextEncoding, parse_str};
use pgn_reader::{Reader, SanPlus, Visitor};

const RUY: &str = "The Ruy Lopez - Move by Move.pgn";
const TEXTBOOK: &str = "Дворецкий. Учебник жндшпиля.pgn";

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../pgnFilesforExample")
}

/// Independent oracle: main-line move count per record via pgn-reader.
struct MainlineCounter;

impl Visitor for MainlineCounter {
    type Tags = ();
    type Movetext = usize;
    type Output = usize;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(())
    }
    fn begin_movetext(&mut self, _tags: ()) -> ControlFlow<Self::Output, Self::Movetext> {
        ControlFlow::Continue(0)
    }
    fn san(&mut self, n: &mut usize, _san: SanPlus) -> ControlFlow<Self::Output> {
        *n += 1;
        ControlFlow::Continue(())
    }
    fn end_game(&mut self, n: usize) -> usize {
        n
    }
}

#[derive(Default, Debug)]
struct Stats {
    games: usize,
    nodes: usize,
    null_moves: usize,
    nags: usize,
    comments: usize,
    fen_games: usize,
    black_to_move: usize,
    max_depth: usize,
    unparsed_fragments: usize,
    diagnostics: BTreeMap<String, usize>,
    roundtrip_failures: Vec<usize>,
    plycount_mismatch: Vec<usize>,
    oracle_mismatch: Vec<usize>,
}

fn walk_stats(g: &Game, s: &mut Stats) {
    for id in g.attached_nodes() {
        let Some(n) = g.node(id) else { continue };
        if let Some(m) = n.game_move() {
            s.nodes += 1;
            if m.is_null() {
                s.null_moves += 1;
            }
            s.max_depth = s.max_depth.max(g.variation_depth(id));
        }
        s.nags += n.nags().len();
        s.comments += n.comments_before().len() + n.comments_after().len();
        s.unparsed_fragments += n.unparsed().len();
    }
}

fn run_file(name: &str) -> Option<(Stats, PgnDatabase)> {
    let path = corpus_dir().join(name);
    if !path.exists() {
        eprintln!("SKIP: {} not found", path.display());
        return None;
    }
    let before = std::fs::read(&path).expect("read");
    let mtime_before = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

    let mut reports = 0;
    let db =
        PgnDatabase::open(&path, &mut |_| reports += 1, &AtomicBool::new(false)).expect("open");
    assert!(reports >= 1);

    // Oracle counts.
    let mut reader = Reader::new(before.as_slice());
    let mut oracle = Vec::new();
    while let Ok(Some(n)) = reader.read_game(&mut MainlineCounter) {
        oracle.push(n);
    }

    let mut s = Stats::default();
    for (i, e) in db.entries().iter().enumerate() {
        s.games += 1;
        let parsed = db.load_game(i).expect("entry");
        let g = &parsed.game;
        for d in &parsed.diagnostics {
            *s.diagnostics
                .entry(format!("{:?}/{:?}", d.severity, d.code))
                .or_default() += 1;
        }
        if g.tag("FEN").is_some() {
            s.fen_games += 1;
        }
        if g.position(g.root()).map(|p| p.turn()) == Some(chess_core::shakmaty::Color::Black) {
            s.black_to_move += 1;
        }
        walk_stats(g, &mut s);

        let plies = g.mainline().len();
        if e.ply_count().is_some_and(|pc| pc as usize != plies) {
            s.plycount_mismatch.push(i + 1);
        }
        if oracle.get(i) != Some(&plies) {
            s.oracle_mismatch.push(i + 1);
        }

        let text = g.to_pgn();
        let back = parse_str(&text);
        if back.game.canonical_tree() != g.canonical_tree()
            || back.game.tags() != g.tags()
            || back.game.to_pgn() != text
        {
            s.roundtrip_failures.push(i + 1);
        }
    }

    let after = std::fs::read(&path).expect("read");
    let mtime_after = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    assert!(before == after, "source file changed");
    assert_eq!(mtime_before, mtime_after, "source mtime changed");
    assert_eq!(
        db.entries().len(),
        oracle.len(),
        "record count differs from pgn-reader"
    );
    eprintln!(
        "== {name}: encoding {:?} ({:?})",
        db.encoding(),
        db.detected_encoding().confidence
    );
    eprintln!("{s:#?}");
    Some((s, db))
}

#[derive(Debug, Default, PartialEq)]
struct LetterStats {
    tilde: usize,
    angle: usize,
    caret: usize,
    sequences: usize,
    genuine: usize,
    upper_ch: usize,
    restored_diags: usize,
}

/// Counts substitution sequences and genuine ч/Ч/ю/Ю in all comments,
/// with the repair off (`.0`) and on (`.1`).
fn letter_stats(db: &PgnDatabase) -> (LetterStats, LetterStats) {
    let mut db = db.clone();
    let mut out = Vec::new();
    for enabled in [false, true] {
        db.set_repair_letters(enabled);
        let mut st = LetterStats::default();
        for i in 0..db.len() {
            let p = db.load_game(i).expect("game");
            st.restored_diags += p
                .diagnostics
                .iter()
                .filter(|d| d.code == DiagnosticCode::LettersRestored)
                .count();
            for id in p.game.attached_nodes() {
                let Some(n) = p.game.node(id) else { continue };
                for c in n.comments_before().iter().chain(n.comments_after()) {
                    st.tilde += c.matches("~~").count();
                    st.angle += c.matches("><").count();
                    st.caret += c.matches("o^").count() + c.matches("о^").count();
                    st.upper_ch += c.matches('Ч').count();
                    st.genuine += c
                        .chars()
                        .filter(|ch| matches!(ch, 'ч' | 'Ч' | 'ю' | 'Ю'))
                        .count();
                }
            }
        }
        st.sequences = st.tilde + st.angle + st.caret;
        out.push(st);
    }
    let after = out.pop().unwrap_or_default();
    let before = out.pop().unwrap_or_default();
    (before, after)
}

fn entry_tag<'a>(e: &'a GameEntry, t: &str) -> &'a str {
    e.tag(t).unwrap_or("")
}

#[test]
fn real_corpus() {
    let ruy = run_file(RUY);
    let book = run_file(TEXTBOOK);

    if let Some((s, db)) = &ruy {
        assert_eq!(db.encoding(), TextEncoding::Windows1252);
        assert!(!db.letter_repair().detected);
        assert!(!db.repair_letters_enabled());
        assert_eq!(s.games, 49);
        assert!(
            db.entries()
                .iter()
                .any(|e| entry_tag(e, "White") == "Hübner, R.")
        );
        assert!(
            s.roundtrip_failures.is_empty(),
            "{:?}",
            s.roundtrip_failures
        );
        assert!(s.plycount_mismatch.is_empty(), "{:?}", s.plycount_mismatch);
        assert!(s.oracle_mismatch.is_empty(), "{:?}", s.oracle_mismatch);
        assert!(!s.diagnostics.keys().any(|k| k.starts_with("Error")));
        // Figures from the independent audit (docs/research/requirements-and-pgn.md).
        assert_eq!(
            (s.nodes, s.null_moves, s.nags, s.comments),
            (8340, 23, 835, 3040)
        );
        assert_eq!((s.fen_games, s.max_depth), (1, 4));
        assert!(s.diagnostics.is_empty(), "{:?}", s.diagnostics);
    }
    if let Some((s, db)) = &book {
        // 9192 comment blocks + 1 fragment recovered from record №335.
        assert_eq!(
            (s.nodes, s.null_moves, s.nags, s.comments),
            (53923, 246, 12705, 9193)
        );
        assert_eq!((s.fen_games, s.black_to_move), (845, 268));
        assert_eq!(s.diagnostics.get("Info/FenFullmoveZero"), Some(&72));
        assert_eq!(db.encoding(), TextEncoding::Windows1251);
        assert_eq!(s.games, 847);
        assert!(
            s.roundtrip_failures.is_empty(),
            "{:?}",
            s.roundtrip_failures
        );
        assert!(s.plycount_mismatch.is_empty(), "{:?}", s.plycount_mismatch);
        assert!(s.oracle_mismatch.is_empty(), "{:?}", s.oracle_mismatch);
        assert!(!s.diagnostics.keys().any(|k| k.starts_with("Error")));
        assert_eq!(s.max_depth, 7);

        // Record №335: a comment closed early; the text must not vanish silently.
        let parsed = db.load_game(334).expect("335");
        assert_eq!(db.entries()[334].white(), Some("Averbakh, Yuri L"));
        let codes: Vec<_> = parsed.diagnostics.iter().map(|d| d.code).collect();
        assert!(
            codes.contains(&DiagnosticCode::UnrecognizedText),
            "{codes:?}"
        );
        assert!(
            codes.contains(&DiagnosticCode::StrayClosingBrace),
            "{codes:?}"
        );
        let d = parsed
            .diagnostics
            .iter()
            .find(|d| d.code == DiagnosticCode::UnrecognizedText)
            .expect("diag");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.location.map(|l| l.line), Some(8264));
        let all_comments: String = parsed
            .game
            .attached_nodes()
            .iter()
            .filter_map(|id| parsed.game.node(*id))
            .flat_map(|n| n.comments_after().iter().chain(n.comments_before()))
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all_comments.contains("то исход"), "{all_comments}");
        assert!(all_comments.contains("вытеснению."), "{all_comments}");
        assert!(all_comments.contains("При короле на a1"));
        // Letter repair (~~ → ч, >< → Ч, o^ → ю).
        assert!(db.letter_repair().detected, "{:?}", db.letter_repair());
        let (before, after) = letter_stats(db);
        eprintln!("letter repair: before {before:?}\nafter {after:?}");
        assert_eq!(before.genuine, 0);
        assert_eq!(after.sequences, 0, "sequences left in comments");
        assert_eq!(after.genuine, before.sequences);
        assert_eq!(
            (before.tilde, before.angle, before.caret),
            (5702, 339, 2974)
        );
        eprintln!("#335 diagnostics:");
        for d in &parsed.diagnostics {
            eprintln!("  {d}");
        }
    }
    if let (Some((a, _)), Some((b, _))) = (&ruy, &book) {
        assert_eq!(a.games + b.games, 896);
        eprintln!(
            "TOTAL: games {}, move nodes {}, null moves {}, NAG {}, FEN {}",
            a.games + b.games,
            a.nodes + b.nodes,
            a.null_moves + b.null_moves,
            a.nags + b.nags,
            a.fen_games + b.fen_games
        );
    }
}
