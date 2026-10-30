//! The web version: a page in a browser, without threads, processes or a file system.
//!
//! The program's folders are names in the page's own file tree ([`vfs`]): `/config`,
//! `/data` and `/cache` are kept in the browser's storage ([`idb`], loaded before the
//! interface starts, written behind it); `/examples` (the samples, built in) and
//! `/opened` (the files the user gave the page, only read: decisions «Веб-версия») live
//! for the session. Background work runs on the page's thread after the frame.

pub mod engine;
pub mod idb;
pub mod import;
pub mod page;
pub mod remembered;
pub mod storage;
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
    _progress: impl FnMut(ScanProgress) + Send + 'static,
    cancel: std::sync::Arc<AtomicBool>,
    done: impl FnOnce(Result<Opened, ImportError>) + Send + 'static,
) -> std::io::Result<()> {
    import::open(path, move |read| {
        done(if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            Err(ImportError::Cancelled)
        } else {
            read
        })
    });
    Ok(())
}
