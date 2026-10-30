//! «My games» of the web version (docs/design/web-version.md §2.3): one PGN database
//! kept in the browser (the page's `/data`, in IndexedDB), always on the shelf. Saving
//! a game (Ctrl+S, 💾) puts it there — into its own record, or as a new one; the user's
//! files are only read. «Download all as PGN» gives it back as a file.

use std::path::PathBuf;

use crate::i18n::tr;
use crate::settings::Settings;

const DIR: &str = "my-games";

/// The database: `/data/my-games/<name>.pgn`, named in the language of the first start.
pub fn path() -> PathBuf {
    let dir = crate::platform::data_dir().unwrap_or_default().join(DIR);
    crate::platform::web::vfs::list(&dir)
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "pgn"))
        .unwrap_or_else(|| dir.join(format!("{}.pgn", tr("My games"))))
}

/// Makes the database at the first start and keeps it on the shelf; `true` when the
/// settings changed.
pub fn setup(settings: &mut Settings) -> bool {
    let path = path();
    if !crate::platform::fs::exists(&path) {
        let _ = crate::platform::fs::write_atomic(&path, b"");
    }
    if settings.databases.contains(&path) {
        return false;
    }
    settings.databases.push(path);
    true
}

/// Whether `path` is «My games».
pub fn is(path: &std::path::Path) -> bool {
    path == self::path()
}

/// Gives all of «My games» as a downloaded PGN file (`ChessPenguin-my-games-<date>.pgn`).
pub fn download_all() -> Result<(), String> {
    let text = crate::platform::fs::read_to_string(&path()).map_err(|e| e.to_string())?;
    let date = chrono::Local::now().format("%Y-%m-%d");
    crate::platform::files::download_pgn(&format!("ChessPenguin-my-games-{date}.pgn"), &text)
}

/// Games in «My games» (records of its text; 0 when empty).
pub fn count() -> usize {
    crate::platform::fs::read(&path())
        .ok()
        .and_then(|b| {
            chess_core::PgnDatabase::from_bytes(
                b,
                Some(chess_core::TextEncoding::Utf8),
                &mut |_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .ok()
        })
        .map_or(0, |db| db.len())
}

/// Its size in bytes.
pub fn size() -> u64 {
    crate::platform::fs::meta(&path()).map_or(0, |m| m.len)
}

/// A game left unsaved when the page was closed (docs/design/web-version.md §2.6): one,
/// the last, written every few seconds while it has changes. It guards against a
/// closed tab; it is not a save: offered at the next start, opened or deleted.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Draft {
    pub title: String,
    pub moves: usize,
    pub pgn: String,
}

fn draft_path() -> PathBuf {
    crate::platform::data_dir()
        .unwrap_or_default()
        .join("draft.json")
}

pub fn write_draft(draft: &Draft) {
    if let Ok(json) = serde_json::to_vec(draft) {
        let _ = crate::platform::fs::write_atomic(&draft_path(), &json);
    }
}

pub fn read_draft() -> Option<Draft> {
    let bytes = crate::platform::fs::read(&draft_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn delete_draft() {
    let _ = crate::platform::fs::remove_file(&draft_path());
}
