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
    /// The book as the setting keeps it (`BookChoice::setting`).
    pub book: PathBuf,
    /// Seconds since 1970, of the last save.
    pub saved: u64,
    /// The report's games and errors at that save.
    pub games: u32,
    pub errors: ErrorCounts,
    /// The errors were searched (else the counts mean nothing).
    pub evaluated: bool,
}

/// The folder of the saved preparations.
pub fn dir() -> Option<PathBuf> {
    crate::platform::data_dir().map(|d| d.join("preparations"))
}

/// The id of the preparation of `player` in the database `path`.
pub fn id(path: &Path, player: &Player) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for b in path.to_string_lossy().bytes() {
        eat(b);
    }
    for id in &player.ids {
        for b in id.to_le_bytes() {
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
    serde_json::from_slice(&std::fs::read(report_path(dir, id)).ok()?).ok()
}

/// Keeps `meta` and `report` as the preparation of their player and database (the
/// report first: a list entry never points to a missing report).
pub fn save(dir: &Path, meta: &SavedPrep, report: &PrepReport) -> std::io::Result<()> {
    let id = id(&meta.path, &meta.player);
    let bytes = serde_json::to_vec(report).map_err(std::io::Error::other)?;
    write_atomic(&report_path(dir, &id), &bytes)?;
    let bytes = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
    write_atomic(&meta_path(dir, &id), &bytes)
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
        }
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
        let ida = id(&a.path, &a.player);
        assert!(report_path(&dir, &ida).exists());
        assert!(super::report(&dir, &ida).is_some());
        rename(&dir, &ida, "Вафин — финал").expect("rename");
        assert_eq!(get(&dir, &ida).unwrap().title, "Вафин — финал");
        delete(&dir, &ida).expect("delete");
        assert_eq!(list(&dir).len(), 1);
        assert!(!report_path(&dir, &ida).exists());
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
