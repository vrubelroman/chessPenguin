//! How many games each database on the shelf holds before it is opened: «not loaded»
//! on a card told the user nothing. A thread of its own counts the databases one after
//! another (reading a large one takes seconds; the screen never waits) and remembers the
//! counts with the file's size and time, so the next start shows them at once. A card
//! whose database is not counted yet says nothing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use chess_import::Database;
use serde::{Deserialize, Serialize};

/// What a card says of a database: its games and whether it is a ChessBase one read in
/// place (a lock: it cannot take games).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    pub games: usize,
    pub locked: bool,
}

impl Count {
    pub fn of(db: &Database) -> Count {
        match db.as_chessbase() {
            Some(b) => Count {
                games: b.games() + b.texts(),
                locked: true,
            },
            None => Count {
                games: db.len(),
                locked: false,
            },
        }
    }
}

/// A count and the file it was taken from.
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Entry {
    size: u64,
    modified: u64,
    count: Count,
}

enum Job {
    /// Count the file, unless the count remembered is of the file as it is.
    Count(PathBuf),
    /// The database was opened: its count, nothing to read.
    Known(PathBuf, Count),
}

#[derive(Default)]
struct Shared {
    known: HashMap<PathBuf, Entry>,
}

pub struct Counts {
    shared: Arc<Mutex<Shared>>,
    asked: HashSet<PathBuf>,
    jobs: Option<Sender<Job>>,
}

impl Counts {
    pub fn new(ctx: &egui::Context) -> Self {
        let file = crate::settings::Settings::cache_dir().map(|d| d.join("database-counts.json"));
        Self::kept_in(file, ctx)
    }

    /// Counts remembered in `file` (`None`: for this run only).
    fn kept_in(file: Option<PathBuf>, ctx: &egui::Context) -> Self {
        let known: HashMap<PathBuf, Entry> = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let shared = Arc::new(Mutex::new(Shared { known }));
        let (tx, rx) = channel::<Job>();
        let (worker, ctx) = (shared.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("db-counts".into())
            .spawn(move || {
                let never = AtomicBool::new(false);
                for job in rx {
                    let (path, known) = match job {
                        Job::Count(p) => (p, None),
                        Job::Known(p, c) => (p, Some(c)),
                    };
                    let Some((size, modified)) = stamp(&path) else {
                        continue;
                    };
                    let fresh = worker.lock().ok().and_then(|s| {
                        s.known
                            .get(&path)
                            .filter(|e| e.size == size && e.modified == modified)
                            .map(|e| e.count)
                    });
                    let count = match known {
                        Some(c) if fresh == Some(c) => continue,
                        Some(c) => c,
                        None if fresh.is_some() => continue,
                        None => match chess_import::open(&path, &mut |_| {}, &never) {
                            Ok(o) => Count::of(&o.db),
                            // The card says why when the database is opened.
                            Err(_) => continue,
                        },
                    };
                    let json = match worker.lock() {
                        Ok(mut s) => {
                            s.known.insert(
                                path,
                                Entry {
                                    size,
                                    modified,
                                    count,
                                },
                            );
                            serde_json::to_vec(&s.known).ok()
                        }
                        Err(_) => return,
                    };
                    if let (Some(f), Some(json)) = (&file, json) {
                        if let Some(dir) = f.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = crate::settings::write_atomic(f, &json);
                    }
                    ctx.request_repaint();
                }
            });
        Self {
            shared,
            asked: HashSet::new(),
            jobs: spawned.ok().map(|_| tx),
        }
    }

    /// The games of a database not opened yet (`None`: not counted yet); it is counted
    /// from now on.
    pub fn get(&mut self, path: &Path) -> Option<Count> {
        if self.asked.insert(path.to_owned())
            && let Some(jobs) = &self.jobs
        {
            let _ = jobs.send(Job::Count(path.to_owned()));
        }
        self.shared.lock().ok()?.known.get(path).map(|e| e.count)
    }

    /// A database was opened: its count is remembered for the next start.
    pub fn opened(&mut self, path: &Path, db: &Database) {
        self.asked.insert(path.to_owned());
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Known(path.to_owned(), Count::of(db)));
        }
    }
}

/// The file's size and time (seconds since the epoch); a changed file is counted again.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((m.len(), modified))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(counts: &mut Counts, path: &Path, want: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if counts.get(path).map(|c| c.games) == Some(want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_database_is_counted_away_from_the_screen_and_again_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("cp-db-counts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pgn = dir.join("a.pgn");
        std::fs::write(
            &pgn,
            "[White \"A\"]\n\n1. e4 *\n\n[White \"B\"]\n\n1. d4 *\n",
        )
        .unwrap();
        let mut counts = Counts::kept_in(None, &egui::Context::default());
        // Asked: nothing known yet, nothing waited for.
        assert_eq!(counts.get(&pgn), None);
        assert!(wait(&mut counts, &pgn, 2));
        // An opened database tells its count; a changed file is counted anew.
        std::fs::write(&pgn, "[White \"A\"]\n\n1. e4 *\n").unwrap();
        let db = chess_import::open(&pgn, &mut |_| {}, &AtomicBool::new(false))
            .unwrap()
            .db;
        counts.opened(&pgn, &db);
        assert!(wait(&mut counts, &pgn, 1));
        assert!(!counts.get(&pgn).unwrap().locked);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
