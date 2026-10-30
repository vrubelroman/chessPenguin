//! How many games each database on the shelf holds before it is opened: «not loaded»
//! on a card told the user nothing. The databases are counted one after another away
//! from the screen (reading a large one takes seconds; the screen never waits): on a
//! thread of its own, in a browser by the page's reader (`platform::web::import`). The
//! counts are remembered with the file's size and time, so the next start shows them
//! at once. A card whose database is not counted yet says nothing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

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

/// A file to read and count: its path and its size and time now.
struct ToCount(PathBuf, (u64, u64));

/// The counts known and where they are kept.
#[derive(Clone)]
struct Counter {
    known: Arc<Mutex<HashMap<PathBuf, Entry>>>,
    /// `None`: for this run only.
    file: Option<PathBuf>,
    ctx: egui::Context,
}

impl Counter {
    /// What the job leaves to do: a file to read, or nothing (its count is known now).
    fn take(&self, job: Job) -> Option<ToCount> {
        let (path, known) = match job {
            Job::Count(p) => (p, None),
            Job::Known(p, c) => (p, Some(c)),
        };
        let stamp = stamp(&path)?;
        let fresh = self.known.lock().ok().and_then(|k| {
            k.get(&path)
                .filter(|e| (e.size, e.modified) == stamp)
                .map(|e| e.count)
        });
        match known {
            Some(c) if fresh == Some(c) => None,
            Some(c) => {
                self.record(path, stamp, c);
                None
            }
            None if fresh.is_some() => None,
            None => Some(ToCount(path, stamp)),
        }
    }

    fn record(&self, path: PathBuf, (size, modified): (u64, u64), count: Count) {
        let json = match self.known.lock() {
            Ok(mut k) => {
                k.insert(
                    path,
                    Entry {
                        size,
                        modified,
                        count,
                    },
                );
                serde_json::to_vec(&*k).ok()
            }
            Err(_) => return,
        };
        if let (Some(f), Some(json)) = (&self.file, json) {
            let _ = crate::platform::fs::write_atomic(f, &json);
        }
        self.ctx.request_repaint();
    }

    /// Counts the files of the jobs, one after another, on a thread of its own.
    #[cfg(not(web))]
    fn start(self, jobs: Receiver<Job>) -> bool {
        let spawned = std::thread::Builder::new()
            .name("db-counts".into())
            .spawn(move || {
                let never = std::sync::atomic::AtomicBool::new(false);
                for job in jobs {
                    let Some(ToCount(path, stamp)) = self.take(job) else {
                        continue;
                    };
                    // The card says why it cannot be read when the database is opened.
                    if let Ok(o) = chess_import::open(&path, &mut |_| {}, &never) {
                        self.record(path, stamp, Count::of(&o.db));
                    }
                }
            });
        spawned.is_ok()
    }

    /// Counts the files of the jobs, one after another, in the page's reader: the next
    /// once the last is answered ([`Counts`] calls [`Counter::next`] after each job).
    #[cfg(web)]
    fn start(self, jobs: Receiver<Job>) -> bool {
        WEB.with_borrow_mut(|w| {
            *w = Some(Web {
                counter: self,
                jobs,
                busy: false,
            })
        });
        true
    }

    #[cfg(web)]
    fn next() {
        let read = WEB.with_borrow_mut(|w| {
            let w = w.as_mut()?;
            if w.busy {
                return None;
            }
            while let Ok(job) = w.jobs.try_recv() {
                if let Some(to_count) = w.counter.take(job) {
                    w.busy = true;
                    return Some((w.counter.clone(), to_count));
                }
            }
            None
        });
        let Some((counter, ToCount(path, stamp))) = read else {
            return;
        };
        crate::platform::web::import::count(&path.clone(), move |games| {
            if let Some(games) = games {
                let locked = false;
                counter.record(path, stamp, Count { games, locked });
            }
            WEB.with_borrow_mut(|w| {
                if let Some(w) = w.as_mut() {
                    w.busy = false;
                }
            });
            Counter::next();
        });
    }
}

/// The counting of the page: the jobs not taken yet, and whether a file is being read.
#[cfg(web)]
struct Web {
    counter: Counter,
    jobs: Receiver<Job>,
    busy: bool,
}

#[cfg(web)]
thread_local! {
    static WEB: std::cell::RefCell<Option<Web>> = const { std::cell::RefCell::new(None) };
}

pub struct Counts {
    known: Arc<Mutex<HashMap<PathBuf, Entry>>>,
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
            .and_then(|f| crate::platform::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let known = Arc::new(Mutex::new(known));
        let (tx, rx) = channel::<Job>();
        let counter = Counter {
            known: known.clone(),
            file,
            ctx: ctx.clone(),
        };
        Self {
            known,
            asked: HashSet::new(),
            jobs: counter.start(rx).then_some(tx),
        }
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
            #[cfg(web)]
            Counter::next();
        }
    }

    /// The games of a database not opened yet (`None`: not counted yet); it is counted
    /// from now on.
    pub fn get(&mut self, path: &Path) -> Option<Count> {
        if self.asked.insert(path.to_owned()) {
            self.send(Job::Count(path.to_owned()));
        }
        self.known.lock().ok()?.get(path).map(|e| e.count)
    }

    /// A database was opened: its count is remembered for the next start.
    pub fn opened(&mut self, path: &Path, db: &Database) {
        self.asked.insert(path.to_owned());
        self.send(Job::Known(path.to_owned(), Count::of(db)));
    }
}

/// The file's size and time (seconds since the epoch); a changed file is counted again.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let m = crate::platform::fs::meta(path).ok()?;
    Some((m.len, m.modified_ms / 1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use web_time::Instant;

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
        let never = std::sync::atomic::AtomicBool::new(false);
        let db = chess_import::open(&pgn, &mut |_| {}, &never).unwrap().db;
        counts.opened(&pgn, &db);
        assert!(wait(&mut counts, &pgn, 1));
        assert!(!counts.get(&pgn).unwrap().locked);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
