//! The web version: a page in a browser, without threads, processes or a file system.
//!
//! The program's folders are names in the page's own file tree ([`vfs`]): `/config`,
//! `/data` and `/cache` are kept in the browser's storage ([`idb`], loaded before the
//! interface starts, written behind it); `/examples` (the samples, built in) and
//! `/opened` (the files the user gave the page, only read: decisions «Веб-версия») live
//! for the session. Background work runs on the page's thread after the frame.

pub mod idb;
pub mod page;
pub mod vfs;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chess_core::ScanProgress;
use chess_import::{ImportError, Opened};

pub fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(vfs::CONFIG))
}

pub fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(vfs::DATA))
}

pub fn cache_dir() -> Option<PathBuf> {
    Some(PathBuf::from(vfs::CACHE))
}

pub fn examples_dir() -> Option<PathBuf> {
    Some(PathBuf::from(vfs::EXAMPLES))
}

/// The page has no threads: the job runs on its thread once the current frame is
/// done (the screen shows the state before it, «Opening…», first).
pub fn spawn(_name: &str, job: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    wasm_bindgen_futures::spawn_local(async move {
        page::next_task().await;
        job();
    });
    Ok(())
}

pub fn open_database(
    path: &Path,
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> Result<Opened, ImportError> {
    let bytes = vfs::read(path).map_err(|source| ImportError::Io {
        path: path.to_owned(),
        source,
    })?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    chess_import::open_bytes(&name, bytes, progress, cancel)
}
