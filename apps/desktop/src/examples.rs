//! Sample databases for the first start, so the library is not empty: classic games
//! with short introductions, and Alekhine's games (moves only). They are written once,
//! named in the interface language, to `<local data dir>/examples/` and put on the
//! library shelf; a sample removed from the shelf does not come back.

use std::path::PathBuf;

use crate::i18n::tr;
use crate::settings::Settings;

fn files() -> [(&'static str, &'static str); 2] {
    [
        (
            tr("Classic games"),
            include_str!("../../../examples/classics.pgn"),
        ),
        (
            tr("Alekhine"),
            include_str!("../../../examples/alekhine.pgn"),
        ),
    ]
}

fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("examples"))
}

/// Adds the samples once; `true` when the settings changed. A new library starts with
/// them (the classics open); an existing one gets them after its own databases.
pub fn first_run_setup(settings: &mut Settings) -> bool {
    if settings.examples_added {
        return false;
    }
    settings.examples_added = true;
    let Some(dir) = dir() else {
        return true;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("examples: cannot create {}: {e}", dir.display());
        return true;
    }
    for (name, text) in files() {
        let path = dir.join(format!("{name}.pgn"));
        if !path.exists()
            && let Err(e) = std::fs::write(&path, text)
        {
            eprintln!("examples: cannot write {}: {e}", path.display());
            continue;
        }
        if !settings.databases.contains(&path) {
            settings.databases.push(path);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_valid_pgn() {
        for (name, text) in files() {
            let dir = std::env::temp_dir().join(format!("cp-examples-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{name}.pgn"));
            std::fs::write(&path, text).unwrap();
            let db = chess_core::PgnDatabase::open(
                &path,
                &mut |_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            assert!(db.len() >= 30, "{name}: {} games", db.len());
            for i in 0..db.len() {
                let parsed = db.load_game(i).unwrap();
                assert!(
                    parsed.diagnostics.is_empty(),
                    "{name} #{}: {:?}",
                    i + 1,
                    parsed.diagnostics
                );
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
