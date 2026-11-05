//! Saved preparations: a preparation against an opponent kept for good (not in the
//! cache) and opened again from «Preparations» in the library. One per player of a
//! database: saving the same player of the same database again replaces it. Each is
//! two files in `<data>/preparations/`: `<id>.json`, what the list shows, and
//! `<id>.report.json`, the report with the engine's evaluations (so the errors found
//! come back without the engine, even after the cache was cleared).

use std::path::{Path, PathBuf};

use prep::{ErrorCounts, Period, Player, PrepReport};
use serde::{Deserialize, Serialize};

use crate::prep_ui::ColorChoice;

/// Version of the eligibility rules used for the saved error counters.
pub const ERROR_SELECTION_VERSION: u32 = 1;

/// What the list of preparations knows of one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPrep {
    /// The name in the list (the player's at first; the user can rename it).
    pub title: String,
    /// The database.
    pub path: PathBuf,
    pub player: Player,
    pub names: Vec<String>,
    pub plies: usize,
    pub period: Period,
    pub color: ColorChoice,
    /// Unused legacy book choice, preserved when old preparations are saved again.
    #[serde(default)]
    pub book: PathBuf,
    /// Seconds since 1970, of the last save.
    pub saved: u64,
    /// The report's games and errors at that save.
    pub games: u32,
    pub errors: ErrorCounts,
    /// The errors were searched (else the counts mean nothing).
    pub evaluated: bool,
    #[serde(default)]
    pub error_selection_version: u32,
}

impl SavedPrep {
    pub fn errors_current(&self) -> bool {
        self.error_selection_version == ERROR_SELECTION_VERSION
    }
}

/// The folder of the saved preparations.
pub fn dir() -> Option<PathBuf> {
    crate::platform::data_dir().map(|d| d.join("preparations"))
}

/// The id of the preparation of the player named `names` in the database `path`: by
/// the names, not the database's name ids, which change when a game before his first
/// one is deleted or the database is replaced by a new version.
pub fn id(path: &Path, names: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for b in path.to_string_lossy().bytes() {
        eat(b);
    }
    let mut names: Vec<&String> = names.iter().collect();
    names.sort();
    names.dedup();
    for n in names {
        eat(0);
        for b in n.bytes() {
            eat(b);
        }
    }
    format!("{h:016x}")
}

fn meta_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn report_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.report.json"))
}

/// Writes through a file beside it, so a crash leaves the old file or the new one.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Every saved preparation (id, what it is), the last saved first.
pub fn list(dir: &Path) -> Vec<(String, SavedPrep)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, SavedPrep)> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            let name = path.file_name()?.to_str()?;
            let id = name.strip_suffix(".json")?;
            if id.ends_with(".report") {
                return None;
            }
            let meta = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            Some((id.to_owned(), meta))
        })
        .collect();
    out.sort_by(|a, b| b.1.saved.cmp(&a.1.saved).then(a.1.title.cmp(&b.1.title)));
    out
}

/// The preparation `id`, if saved.
pub fn get(dir: &Path, id: &str) -> Option<SavedPrep> {
    serde_json::from_slice(&std::fs::read(meta_path(dir, id)).ok()?).ok()
}

/// Its report, if it can be read.
pub fn report(dir: &Path, id: &str) -> Option<PrepReport> {
    let mut report: PrepReport =
        serde_json::from_slice(&std::fs::read(report_path(dir, id)).ok()?).ok()?;
    // Stored evaluations remain useful when the rules for selecting errors change.
    report.refresh();
    Some(report)
}

/// Keeps `meta` and `report` as the preparation of their player and database (the
/// report first: a list entry never points to a missing report).
pub fn save(dir: &Path, meta: &SavedPrep, report: &PrepReport) -> std::io::Result<()> {
    let id = id(&meta.path, &meta.names);
    let bytes = serde_json::to_vec(report).map_err(std::io::Error::other)?;
    write_atomic(&report_path(dir, &id), &bytes)?;
    save_meta(dir, meta)
}

/// Writes only the list entry: a preparation is kept from the moment it is started,
/// before its report is built.
pub fn save_meta(dir: &Path, meta: &SavedPrep) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
    write_atomic(&meta_path(dir, &id(&meta.path, &meta.names)), &bytes)
}

/// A new name in the list.
pub fn rename(dir: &Path, id: &str, title: &str) -> std::io::Result<()> {
    let mut meta = get(dir, id).ok_or_else(|| std::io::Error::other("no such preparation"))?;
    meta.title = title.to_owned();
    let bytes = serde_json::to_vec_pretty(&meta).map_err(std::io::Error::other)?;
    write_atomic(&meta_path(dir, id), &bytes)
}

/// Removes the preparation (the list entry first).
pub fn delete(dir: &Path, id: &str) -> std::io::Result<()> {
    std::fs::remove_file(meta_path(dir, id))?;
    let _ = std::fs::remove_file(report_path(dir, id));
    Ok(())
}

/// Seconds since 1970 now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(path: &str, ids: [u32; 1], title: &str, saved: u64) -> SavedPrep {
        SavedPrep {
            title: title.to_owned(),
            path: PathBuf::from(path),
            player: Player::new(ids),
            names: vec![title.to_owned()],
            plies: 20,
            period: Period::Months(24),
            color: ColorChoice::Black,
            book: PathBuf::new(),
            saved,
            games: 3,
            errors: ErrorCounts::default(),
            evaluated: false,
            error_selection_version: ERROR_SELECTION_VERSION,
        }
    }

    #[test]
    fn legacy_error_counts_are_not_marked_current() {
        let current = meta("/db/a.pgn", [7], "Target", 10);
        assert!(current.errors_current());
        let mut json = serde_json::to_value(&current).expect("metadata");
        json.as_object_mut()
            .unwrap()
            .remove("error_selection_version");
        let legacy: SavedPrep = serde_json::from_value(json).expect("legacy metadata");
        assert!(!legacy.errors_current());
        let future = SavedPrep {
            error_selection_version: ERROR_SELECTION_VERSION + 1,
            ..current
        };
        assert!(!future.errors_current());
    }

    #[test]
    fn saved_listed_replaced_renamed_and_deleted() {
        let dir = std::env::temp_dir().join(format!("cp-prep-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list(&dir).is_empty());
        let report: PrepReport = serde_json::from_str(REPORT).expect("report");
        let a = meta("/db/a.pgn", [7], "Вафин", 10);
        let b = meta("/db/a.pgn", [8], "Иванов", 20);
        save(&dir, &a, &report).expect("save");
        save(&dir, &b, &report).expect("save");
        let l = list(&dir);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1.title, "Иванов", "the last saved first");
        // The same player of the same database: replaced, not added.
        let a2 = SavedPrep {
            saved: 30,
            plies: 30,
            ..a.clone()
        };
        save(&dir, &a2, &report).expect("save");
        let l = list(&dir);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1, a2);
        let ida = id(&a.path, &a.names);
        assert!(report_path(&dir, &ida).exists());
        assert!(super::report(&dir, &ida).is_some());
        rename(&dir, &ida, "Вафин — финал").expect("rename");
        assert_eq!(get(&dir, &ida).unwrap().title, "Вафин — финал");
        delete(&dir, &ida).expect("delete");
        assert_eq!(list(&dir).len(), 1);
        assert!(!report_path(&dir, &ida).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_book_choice_is_preserved() {
        let dir = std::env::temp_dir().join(format!("cp-prep-legacy-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = meta("/db/a.pgn", [7], "Вафин", 10);
        a.book = PathBuf::from("/db/book.pgn");
        save_meta(&dir, &a).expect("save_meta");
        let ida = id(&a.path, &a.names);
        let path = meta_path(&dir, &ida);
        let json = std::fs::read_to_string(&path).expect("metadata");
        let without_book = json.replace(&format!("\"book\": \"{}\",", a.book.display()), "");
        std::fs::write(&path, without_book).expect("legacy metadata");
        let mut loaded = get(&dir, &ida).expect("defaulted legacy book");
        assert_eq!(loaded.book, PathBuf::new());
        loaded.book = a.book.clone();
        save_meta(&dir, &loaded).expect("save legacy book");
        assert_eq!(get(&dir, &ida).expect("metadata").book, a.book);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_report_rejudges_stale_errors_without_losing_evaluations() {
        let dir = std::env::temp_dir().join(format!("cp-prep-rejudge-{}", std::process::id()));
        let mut stale: PrepReport = serde_json::from_str(REPORT).expect("report");
        stale.search = game_review::SEARCH_VERSION;
        stale.games.push(prep::GameInfo {
            index: 0,
            white: true,
            opponent: "Other".into(),
            opponent_elo: 0,
            date: String::new(),
            outcome: prep::Outcome::Unknown,
            ucis: vec!["e2e4".into(), "e7e5".into(), "g1f3".into()],
            sans: vec!["e4".into(), "e5".into(), "Nf3".into()],
            keys: vec![1, 2, 3, 4],
            opening: None,
            named: None,
            book: None,
        });
        stale.white.games = vec![0];
        for (key, cp, best) in [
            (1, 0, "e2e4"),
            (2, 0, "c7c5"),
            (3, 150, "f1c4"),
            (4, 0, "b8c6"),
        ] {
            stale.evals.insert(
                key,
                prep::PositionEval {
                    nodes: 1000,
                    eval: prep::Eval::Cp(cp),
                    pv: vec![best.into()],
                    depth: 10,
                },
            );
        }
        // This cached Nf3 error followed Black's earlier error, so it no longer
        // belongs in preparation despite its evaluations being fully cached.
        stale.white.errors.push(prep::OpeningError {
            game: 0,
            ply: 2,
            key: 3,
            uci: "g1f3".into(),
            san: "Nf3".into(),
            best_uci: "f1c4".into(),
            best_san: "Bc4".into(),
            judgement: prep::Judgement::Mistake,
            before: prep::Eval::Cp(150),
            after: prep::Eval::Cp(0),
            opening: None,
        });
        stale.white.counts.mistakes = 1;
        let metadata = meta("/db/stale.pgn", [7], "Target", 10);
        save(&dir, &metadata, &stale).expect("save stale report");
        let key = id(&metadata.path, &metadata.names);
        let stored = std::fs::read(report_path(&dir, &key)).expect("stored bytes");
        let loaded = super::report(&dir, &key).expect("load saved report");
        assert!(loaded.white.errors.is_empty());
        assert_eq!(loaded.white.counts.total(), 0);
        assert_eq!(loaded.evals, stale.evals);
        assert_eq!(loaded.games, stale.games);
        assert_eq!(std::fs::read(report_path(&dir, &key)).unwrap(), stored);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn started_preparation_is_listed_before_its_report_exists() {
        let dir = std::env::temp_dir().join(format!("cp-prep-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = meta("/db/a.pgn", [7], "Вафин", 10);
        save_meta(&dir, &a).expect("save_meta");
        assert_eq!(list(&dir).len(), 1, "in the list from the start");
        let ida = id(&a.path, &a.names);
        assert!(super::report(&dir, &ida).is_none(), "no report yet");
        let report: PrepReport = serde_json::from_str(REPORT).expect("report");
        save(&dir, &a, &report).expect("save");
        assert!(super::report(&dir, &ida).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const REPORT: &str = r#"{"version":4,"fingerprint":1,"player":{"ids":[7]},"names":["A"],
        "plies":20,"side_white":true,"side_black":true,"games":[],"skipped":0,"openings":[],
        "white":{"white":true,"games":[],"tree":{"nodes":[]},"families":[],"errors":[],
        "repeated":[],"counts":{"inaccuracies":0,"mistakes":0,"blunders":0}},
        "black":{"white":false,"games":[],"tree":{"nodes":[]},"families":[],"errors":[],
        "repeated":[],"counts":{"inaccuracies":0,"mistakes":0,"blunders":0}},
        "book":null,"engine":null,"evals":{}}"#;
}
