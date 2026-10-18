//! Conversion of the real databases in `pgnFilesforExample/` (not in the repository:
//! the tests skip what is missing).
//!
//! Every converted game must parse back without warnings, a ChessBase database must
//! convert the same from its `.cbv` archive as unpacked, and texts must keep their
//! letters.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chess_core::Severity;
use chess_import::{Format, Opened};

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pgnFilesforExample")
}

fn open(name: &str) -> Option<Opened> {
    let path = samples().join(name);
    if !path.exists() {
        eprintln!("skipped: {} not found", path.display());
        return None;
    }
    let never = AtomicBool::new(false);
    Some(chess_import::open(&path, &mut |_| {}, &never).expect("opens"))
}

/// Parses every game; returns (games with moves, all PGN text joined).
fn check_all(o: &Opened) -> (usize, String) {
    let mut with_moves = 0;
    let mut text = String::new();
    for i in 0..o.db.len() {
        let g = o.db.load_game(i).expect("game");
        assert!(
            g.max_severity().is_none_or(|s| s < Severity::Warning),
            "game {i}: {:?}\n{}",
            g.diagnostics,
            o.db.raw_text(i).unwrap_or_default()
        );
        if !g.game.mainline().is_empty() {
            with_moves += 1;
        }
        let raw = o.db.raw_text(i).unwrap_or_default();
        assert!(
            !raw.chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
            "game {i}: control characters in {raw:?}"
        );
        text.push_str(&raw);
    }
    (with_moves, text)
}

#[test]
fn chessbase_archives_convert_cleanly() {
    for (name, games, texts) in [
        ("Rubinstein russian.cbv", 129, 9),
        ("Anand 01-14.cbv", 399, 0),
        ("Endgame Manual.cbv", 847, 134),
    ] {
        let Some(o) = open(name) else { continue };
        assert_eq!(o.format, Format::ChessBaseArchive);
        assert!(o.notes.is_empty(), "{name}: {:?}", o.notes);
        assert_eq!(o.db.len(), games + texts, "{name}: records");
        let (with_moves, _) = check_all(&o);
        // A few games of a book may be only a position with a comment.
        assert!(
            with_moves + texts + 5 >= games + texts,
            "{name}: {with_moves} games with moves"
        );
    }
}

#[test]
fn texts_keep_their_letters() {
    if let Some(o) = open("Rubinstein russian.cbv") {
        let (_, text) = check_all(&o);
        assert!(text.contains("Размены необходимы, чтобы уравнять игру."));
        assert!(!text.contains('\u{fffd}'));
    }
    if let Some(o) = open("Endgame Manual.cbv") {
        let (_, text) = check_all(&o);
        assert!(
            text.contains("Nürnberg"),
            "Western tournament names in a Russian database"
        );
        assert!(!text.contains("Nьrnberg"));
    }
    if let Some(o) = open("Anand 01-14.cbv") {
        let (_, text) = check_all(&o);
        assert!(text.contains("3...Bg4"), "figurines become letters");
        // Only figurines before a square or a capture are converted; of the ~2600 in
        // the book two are typos ("19 ¥5", "5...¥Ne7") and stay as they are.
        assert!(text.matches('\u{a5}').count() <= 2);
        assert!(!text.contains('\u{e027}'));
    }
}

#[test]
fn archives_with_both_forms_of_a_database() {
    // Each book archive holds the database unpacked and as .cbv: the two conversions
    // must be identical, so the archive yields one copy.
    let dir = samples();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipped: no samples");
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".7z") {
            continue;
        }
        let o = open(&name).expect("present");
        assert_eq!(o.format, Format::SevenZip);
        assert!(
            o.notes.len() == 1 && o.notes[0].contains("skipped"),
            "{name}: {:?}",
            o.notes
        );
        let (with_moves, _) = check_all(&o);
        assert!(with_moves > 10, "{name}");
    }
}
