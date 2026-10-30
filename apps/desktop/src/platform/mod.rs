//! What differs between the desktop program and the web version (`docs/dev/web.md`).
//!
//! The interface asks this module, never the system, for: where the program keeps its
//! data ([`config_dir`] and the others), files ([`fs`]), background work ([`spawn`]),
//! the files the user opens or saves ([`files`]), the clipboard ([`clipboard`]) and
//! what the platform offers at all ([`CAPS`]). On a computer it is the system's
//! (`native.rs`); in a browser (`cfg(web)`) the page's (`web/`): the program's data in
//! the browser's storage (IndexedDB), the files the user opened in memory, background
//! work between frames.
#![cfg_attr(web, warn(unused))]

use std::path::PathBuf;

#[cfg(not(web))]
mod native;
#[cfg(not(web))]
use native as imp;
#[cfg(web)]
pub mod web;
#[cfg(web)]
use web as imp;

pub mod clipboard;
pub mod files;
pub mod fs;
pub mod pgn_store;

/// What the platform offers the interface: a feature it lacks is not shown.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    /// Checking for a new version and installing it (the web version is always the
    /// latest: a reload brings it).
    pub updates: bool,
    /// Downloading games from Lichess and Chess.com.
    pub online_games: bool,
    /// Games against the computer (Maia, Rodent).
    pub play: bool,
    /// Preparing against an opponent.
    pub prep: bool,
    /// Machine translation of comments.
    pub translation: bool,
    /// The user's own PGN databases on the disk: new ones, games added and replaced.
    pub own_databases: bool,
    /// Engines as programs on the disk: chosen, downloaded, started.
    pub engine_programs: bool,
    /// The opening trainer.
    pub training: bool,
    /// Saving a game is a download of its PGN (the browser's files are not written).
    pub download_pgn: bool,
    /// The reference tab (what was played here in the databases) and the search of a
    /// position in a database (not in the light web version).
    pub reference: bool,
}

/// The features of this platform.
#[cfg(not(web))]
pub const CAPS: Caps = Caps {
    updates: true,
    online_games: true,
    play: true,
    prep: true,
    translation: true,
    own_databases: true,
    engine_programs: true,
    training: true,
    download_pgn: false,
    reference: true,
};

/// The features of this platform: the light web version (`docs/decisions.md`, «Веб-версия»).
#[cfg(web)]
pub const CAPS: Caps = Caps {
    updates: false,
    online_games: false,
    play: false,
    prep: false,
    translation: false,
    own_databases: false,
    engine_programs: false,
    training: false,
    download_pgn: true,
    reference: false,
};

/// The program's settings (`~/.config/chesspenguin`; in a browser, its storage).
pub fn config_dir() -> Option<PathBuf> {
    imp::config_dir()
}

/// The program's data kept for good (`~/.local/share/chesspenguin`).
pub fn data_dir() -> Option<PathBuf> {
    imp::data_dir()
}

/// Data the program can make again (`~/.cache/chesspenguin`).
pub fn cache_dir() -> Option<PathBuf> {
    imp::cache_dir()
}

/// The sample databases: files of the program, written at the first start on a
/// computer; built in and only read in a browser.
pub fn examples_dir() -> Option<PathBuf> {
    imp::examples_dir()
}

/// Runs `job` in the background and returns at once: on a thread of its own named
/// `name` on a computer; in a browser (which gives a page no threads) on the page's
/// thread after the current frame, so a job there is one finite piece of work, never a
/// loop that waits. The error: the job could not be started.
pub fn spawn(name: &str, job: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    imp::spawn(name, job)
}

/// Bytes the browser still gives the site to keep (its quota less what is used); `None`
/// on a computer and until the browser answered.
pub fn storage_free() -> Option<u64> {
    #[cfg(web)]
    return web::storage::usage().map(|(used, quota)| quota.saturating_sub(used));
    #[cfg(not(web))]
    None
}

/// A size for people: "340 KB", "96 MB", "9.6 GB".
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).max(1.0))
    }
}

/// A line for the program's log (`chesspenguin.log`; in a browser its console).
pub fn log(line: &str) {
    #[cfg(not(web))]
    crate::graphics::log(line);
    #[cfg(web)]
    log::info!("{line}");
}

/// Opens a database the library shows: its file on the disk, or in a browser the file
/// the user gave the page (or a sample built in).
pub fn open_database(
    path: &std::path::Path,
    progress: &mut dyn FnMut(chess_core::ScanProgress),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<chess_import::Opened, chess_import::ImportError> {
    imp::open_database(path, progress, cancel)
}
