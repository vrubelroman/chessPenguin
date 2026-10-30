//! What the screen needs to know about the databases' files (there or not, their size,
//! read only) without touching the disk in the frame: a network drive that has lost its
//! network (an NFS mount waits for it) would hold the whole window. A thread of its own
//! looks at the files the screen asks about, again every few seconds; the screen reads
//! what it last found. A file not looked at yet counts as there. In a browser the
//! files are the page's, in memory: they are looked at when asked.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A file as last found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub exists: bool,
    pub size: u64,
    pub readonly: bool,
}

#[derive(Default)]
struct Shared {
    found: HashMap<PathBuf, State>,
    asked: HashSet<PathBuf>,
}

pub struct FileStates {
    shared: Arc<Mutex<Shared>>,
}

/// How often the files are looked at again.
const EVERY: Duration = Duration::from_secs(3);

impl FileStates {
    #[cfg(not(web))]
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let looked = shared.clone();
        let _ = std::thread::Builder::new()
            .name("file-states".into())
            .spawn(move || {
                loop {
                    let asked: Vec<PathBuf> = match looked.lock() {
                        Ok(s) => s.asked.iter().cloned().collect(),
                        Err(_) => return,
                    };
                    for path in asked {
                        // May wait long (a lost network drive): only this thread waits.
                        let state = look(&path);
                        if let Ok(mut s) = looked.lock() {
                            s.found.insert(path, state);
                        }
                    }
                    std::thread::sleep(EVERY);
                }
            });
        Self { shared }
    }

    #[cfg(web)]
    pub fn new() -> Self {
        Self {
            shared: Arc::default(),
        }
    }

    /// The file as it is (the page's files are in memory: no waiting).
    #[cfg(web)]
    pub fn get(&self, path: &Path) -> Option<State> {
        Some(look(path))
    }

    /// The file as last found (`None`: not looked at yet); it is looked at from now on.
    #[cfg(not(web))]
    pub fn get(&self, path: &Path) -> Option<State> {
        let mut s = self.shared.lock().ok()?;
        if !s.asked.contains(path) {
            s.asked.insert(path.to_owned());
        }
        s.found.get(path).copied()
    }

    /// Whether the file is there (as last found; yes until looked at).
    pub fn exists(&self, path: &Path) -> bool {
        self.get(path).is_none_or(|s| s.exists)
    }
}

fn look(path: &Path) -> State {
    match crate::platform::fs::meta(path) {
        Ok(m) => State {
            exists: true,
            size: m.len,
            readonly: m.readonly,
        },
        Err(_) => State {
            exists: false,
            size: 0,
            readonly: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_looked_at_away_from_the_caller() {
        let dir = std::env::temp_dir().join(format!("cp-file-states-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let there = dir.join("a.pgn");
        std::fs::write(&there, "1. e4 *\n").unwrap();
        let gone = dir.join("b.pgn");
        let states = FileStates::new();
        // Not looked at yet: counted as there, nothing waited for.
        assert!(states.get(&there).is_none());
        assert!(states.exists(&gone));
        let deadline = web_time::Instant::now() + Duration::from_secs(5);
        while states.get(&gone).is_none() && web_time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!states.exists(&gone));
        assert_eq!(states.get(&there).map(|s| s.size), Some(8));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
