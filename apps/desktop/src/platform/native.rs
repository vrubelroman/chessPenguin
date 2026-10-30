//! The desktop program: the system's folders, files and threads.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chess_core::ScanProgress;
use chess_import::{ImportError, Opened};

fn project() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
}

pub fn config_dir() -> Option<PathBuf> {
    project().map(|d| d.config_dir().to_path_buf())
}

pub fn data_dir() -> Option<PathBuf> {
    project().map(|d| d.data_dir().to_path_buf())
}

pub fn cache_dir() -> Option<PathBuf> {
    project().map(|d| d.cache_dir().to_path_buf())
}

pub fn examples_dir() -> Option<PathBuf> {
    project().map(|d| d.data_local_dir().join("examples"))
}

pub fn spawn(name: &str, job: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(job)
        .map(drop)
}

pub fn open_database(
    path: &Path,
    mut progress: impl FnMut(ScanProgress) + Send + 'static,
    cancel: std::sync::Arc<AtomicBool>,
    done: impl FnOnce(Result<Opened, ImportError>) + Send + 'static,
) -> std::io::Result<()> {
    let path = path.to_owned();
    spawn("db-open", move || {
        done(chess_import::open(&path, &mut progress, &cancel));
    })
}
