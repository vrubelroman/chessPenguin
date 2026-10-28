//! Sample databases of the program, so the library is not empty: classic games with
//! short introductions, and Alekhine's games (moves only). At the first start they are
//! written, named in the interface language, to `<local data dir>/examples/` and put on
//! the library shelf; a sample removed from the shelf does not come back.
//!
//! The samples are read only, so their files are the program's: at every start a file
//! that is not the text built into this version (an older version of the sample, a file
//! damaged or edited outside) is replaced by it, whether on the shelf or not. Nothing
//! keeps a sample by its place in the file: the counts of the cards are of the file's
//! size and time, the marks of viewed games of the games' text.

use std::path::{Path, PathBuf};

use crate::i18n::{all_languages, tr};
use crate::settings::Settings;

/// The samples: the English name (a key of the translations) and the text.
const SAMPLES: [(&str, &str); 2] = [
    (
        "Classic games",
        include_str!("../../../examples/classics.pgn"),
    ),
    ("Alekhine", include_str!("../../../examples/alekhine.pgn")),
];

fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("examples"))
}

/// A sample database of the program: only read (games added there would end up in a
/// hidden data folder).
pub fn is_example(path: &std::path::Path) -> bool {
    dir().is_some_and(|d| path.starts_with(d))
}

/// Adds the samples at the first start and brings their files up to this version;
/// `true` when the settings changed. A new library starts with them (the classics
/// open); an existing one gets them after its own databases.
pub fn setup(settings: &mut Settings) -> bool {
    match dir() {
        Some(dir) => setup_in(&dir, settings),
        None => !std::mem::replace(&mut settings.examples_added, true),
    }
}

fn setup_in(dir: &Path, settings: &mut Settings) -> bool {
    let first = !std::mem::replace(&mut settings.examples_added, true);
    for (key, text) in SAMPLES {
        let named = dir.join(format!("{}.pgn", tr(key)));
        // The file is named in the language of the first start: the sample's file under
        // any language's name is it.
        for name in all_languages(key) {
            let path = dir.join(format!("{name}.pgn"));
            let kept = path.exists() || settings.databases.contains(&path);
            if (kept || (first && path == named))
                && let Err(e) = bring_up_to_date(&path, text)
            {
                eprintln!("examples: cannot write {}: {e}", path.display());
            }
        }
        if first && named.exists() && !settings.databases.contains(&named) {
            settings.databases.push(named);
        }
    }
    first
}

/// Writes `text` to `path` unless the file holds it already. The new file takes the old
/// one's place at once (a program started meanwhile reads one or the other whole).
fn bring_up_to_date(path: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|on_disk| on_disk == text.as_bytes()) {
        return Ok(());
    }
    chess_core::pgn::store::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own for each test, empty.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cp-examples-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn classics(dir: &Path) -> PathBuf {
        dir.join(format!("{}.pgn", tr("Classic games")))
    }

    #[test]
    fn samples_are_valid_pgn() {
        let dir = folder("valid");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in SAMPLES {
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
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_start_writes_the_samples_and_shelves_them() {
        let dir = folder("first");
        let mut settings = Settings::default();
        assert!(setup_in(&dir, &mut settings));
        assert!(settings.examples_added);
        assert_eq!(settings.databases.len(), SAMPLES.len());
        assert_eq!(settings.databases[0], classics(&dir));
        for (path, (_, text)) in settings.databases.iter().zip(SAMPLES) {
            assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        }
        // The next start changes nothing.
        let before = settings.databases.clone();
        assert!(!setup_in(&dir, &mut settings));
        assert_eq!(settings.databases, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_or_damaged_sample_is_replaced() {
        let dir = folder("older");
        let mut settings = Settings::default();
        setup_in(&dir, &mut settings);
        let path = classics(&dir);
        let old: String = SAMPLES[0].1.chars().take(SAMPLES[0].1.len() / 3).collect();
        std::fs::write(&path, old).unwrap();
        assert!(!setup_in(&dir, &mut settings));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLES[0].1);
        // Edited outside: the same.
        let mut edited = SAMPLES[0].1.to_owned();
        edited.push_str("\n[Event \"mine\"]\n\n1. e4 *\n");
        std::fs::write(&path, edited).unwrap();
        setup_in(&dir, &mut settings);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLES[0].1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A program that started in another language named the file in it.
    #[test]
    fn a_sample_named_in_another_language_is_replaced() {
        let dir = folder("language");
        std::fs::create_dir_all(&dir).unwrap();
        let russian = dir.join("Классические партии.pgn");
        std::fs::write(&russian, "[Event \"old\"]\n\n1. e4 *\n").unwrap();
        let mut settings = Settings {
            examples_added: true,
            databases: vec![russian.clone()],
            ..Settings::default()
        };
        assert!(!setup_in(&dir, &mut settings));
        assert_eq!(std::fs::read_to_string(&russian).unwrap(), SAMPLES[0].1);
        // The samples of the other names are not made.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(settings.databases, vec![russian]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sample_taken_off_the_shelf_stays_off() {
        let dir = folder("off");
        let mut settings = Settings::default();
        setup_in(&dir, &mut settings);
        let path = classics(&dir);
        settings.databases.retain(|p| *p != path);
        std::fs::write(&path, "[Event \"old\"]\n\n1. e4 *\n").unwrap();
        assert!(!setup_in(&dir, &mut settings));
        assert!(!settings.databases.contains(&path));
        // Its file is the new one, should the user open it again.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLES[0].1);
        // Deleted as well: not made again.
        std::fs::remove_file(&path).unwrap();
        setup_in(&dir, &mut settings);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
