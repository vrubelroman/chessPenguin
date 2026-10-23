//! A player's games from Lichess and Chess.com for the Android app: the `online-games`
//! crate (the same downloads, state files and resuming as on the desktop) with its
//! download on a thread of its own. The app looks accounts up, starts a download into a
//! file of its own folder, asks for the progress while it runs, and can stop it; what
//! was fetched stays, and the next download continues without duplicates.
//!
//! Every call that talks to a site blocks: call it off the main thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use online_games::{Account, Done, Failure, Lookup, Platform, Progress, State};

/// A site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Site {
    Lichess,
    ChessCom,
}

impl From<Site> for Platform {
    fn from(s: Site) -> Self {
        match s {
            Site::Lichess => Platform::Lichess,
            Site::ChessCom => Platform::ChessCom,
        }
    }
}

impl From<Platform> for Site {
    fn from(p: Platform) -> Self {
        match p {
            Platform::Lichess => Site::Lichess,
            Platform::ChessCom => Site::ChessCom,
        }
    }
}

/// An account on a site.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OnlineAccount {
    pub site: Site,
    /// As the site spells it.
    pub username: String,
    /// Lichess: the games of the account (of all kinds, so at most this many).
    pub games: Option<u64>,
    /// Chess.com: the months with games.
    pub months: Option<u32>,
}

impl From<&Account> for OnlineAccount {
    fn from(a: &Account) -> Self {
        OnlineAccount {
            site: a.platform.into(),
            username: a.username.clone(),
            games: a.games,
            months: a.months.map(|m| m as u32),
        }
    }
}

impl From<&OnlineAccount> for Account {
    fn from(a: &OnlineAccount) -> Self {
        Account {
            platform: a.site.into(),
            username: a.username.clone(),
            games: a.games,
            months: a.months.map(|m| m as usize),
        }
    }
}

/// What went wrong talking to a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OnlineProblem {
    /// No answer (offline, a timeout).
    Network,
    /// The site asks for a pause: too many requests.
    RateLimited,
    /// An answer this version does not understand (the site changed).
    Unexpected,
    /// The database could not be written (the phone is full).
    Disk,
    /// The database is being downloaded already.
    Busy,
}

/// A problem, the site it came from (when known), and the technical text.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OnlineFailure {
    pub problem: OnlineProblem,
    pub site: Option<Site>,
    pub detail: String,
}

fn failure(e: &Failure, site: Option<Site>) -> OnlineFailure {
    let problem = match e {
        Failure::Network(_) => OnlineProblem::Network,
        Failure::RateLimited => OnlineProblem::RateLimited,
        Failure::Unexpected(_) => OnlineProblem::Unexpected,
        Failure::Disk(_) => OnlineProblem::Disk,
        Failure::Busy => OnlineProblem::Busy,
    };
    OnlineFailure {
        problem,
        site,
        detail: e.to_string(),
    }
}

/// What looking a nickname up on a site gave.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AccountLookup {
    Found { account: OnlineAccount },
    NotFound,
    Failed { failure: OnlineFailure },
}

/// A nickname both sites could have: 2–30 letters, digits, `_` or `-`.
#[uniffi::export]
pub fn valid_nickname(nick: String) -> bool {
    online_games::valid_nick(nick.trim())
}

/// Looks the nickname up on the site (blocks: it asks the site).
#[uniffi::export]
pub fn find_account(site: Site, nick: String) -> AccountLookup {
    match online_games::lookup(site.into(), &nick) {
        Lookup::Found(a) => AccountLookup::Found {
            account: (&a).into(),
        },
        Lookup::NotFound => AccountLookup::NotFound,
        Lookup::Failed(e) => AccountLookup::Failed {
            failure: failure(&e, Some(site)),
        },
    }
}

/// Accounts and the database file they are downloaded into.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OnlineTarget {
    pub accounts: Vec<OnlineAccount>,
    pub path: String,
    /// The file is there already (its new games are fetched).
    pub exists: bool,
}

/// Where the found accounts go in `dir`: all into one database (`together`) or each into
/// its own; a database already holding them is kept.
#[uniffi::export]
pub fn online_targets(
    dir: String,
    accounts: Vec<OnlineAccount>,
    together: bool,
) -> Vec<OnlineTarget> {
    let accounts: Vec<Account> = accounts.iter().map(Into::into).collect();
    online_games::targets(Path::new(&dir), &accounts, together)
        .into_iter()
        .map(|(group, path)| OnlineTarget {
            accounts: group.iter().map(Into::into).collect(),
            exists: path.exists(),
            path: path.to_string_lossy().into_owned(),
        })
        .collect()
}

/// What a downloaded database holds.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OnlineDatabase {
    pub accounts: Vec<OnlineAccount>,
    /// Games written into it.
    pub games: u64,
    /// When the last download finished (Unix seconds); none while it never finished.
    pub updated: Option<i64>,
}

/// The state of the database at `path`, when it is a downloaded one.
#[uniffi::export]
pub fn online_database(path: String) -> Option<OnlineDatabase> {
    let s = State::load(Path::new(&path))?;
    Some(OnlineDatabase {
        accounts: s.accounts().iter().map(Into::into).collect(),
        games: s.games,
        updated: s.updated,
    })
}

/// Deletes a downloaded database and the files beside it (its state).
#[uniffi::export]
pub fn delete_online_database(path: String) {
    let p = PathBuf::from(path);
    for f in [
        p.clone(),
        p.with_extension("online.json"),
        p.with_extension("online.lock"),
    ] {
        let _ = std::fs::remove_file(f);
    }
}

/// How a download stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DownloadState {
    pub running: bool,
    /// The account being fetched (an index into the download's accounts).
    pub account: u32,
    /// Games of that account so far.
    pub account_games: u64,
    /// Games added by this download so far.
    pub added: u64,
    /// Chess.com: months done of all.
    pub months_done: u32,
    pub months_total: u32,
    /// When it has ended: the games added and in the database now; stopped by the user.
    pub total: Option<u64>,
    pub cancelled: bool,
    pub failure: Option<OnlineFailure>,
}

struct Shared {
    progress: AtomicUsize,
    status: Progress,
    cancel: AtomicBool,
    result: Mutex<Option<Result<Done, OnlineFailure>>>,
}

/// A download of accounts into one database.
#[derive(uniffi::Object)]
pub struct OnlineDownload {
    shared: Arc<Shared>,
}

#[uniffi::export]
impl OnlineDownload {
    /// Starts fetching the games of `accounts` new since the last time into the database
    /// at `path` (created when it is not there). Returns at once.
    #[uniffi::constructor]
    pub fn start(accounts: Vec<OnlineAccount>, path: String) -> Arc<Self> {
        let shared = Arc::new(Shared {
            progress: AtomicUsize::new(0),
            status: Progress::default(),
            cancel: AtomicBool::new(false),
            result: Mutex::new(None),
        });
        let work = shared.clone();
        let accounts: Vec<Account> = accounts.iter().map(Into::into).collect();
        let spawned = std::thread::Builder::new()
            .name("online-download".into())
            .spawn(move || {
                let r = online_games::download(
                    &accounts,
                    Path::new(&path),
                    &work.progress,
                    &work.cancel,
                    &work.status,
                );
                // The account being fetched when it failed: the site to name.
                let site = accounts
                    .get(work.status.account.load(Ordering::Relaxed))
                    .map(|a| a.platform.into());
                *lock(&work.result) = Some(r.map_err(|e| failure(&e, site)));
            });
        if let Err(e) = spawned {
            *lock(&shared.result) = Some(Err(OnlineFailure {
                problem: OnlineProblem::Disk,
                site: None,
                detail: e.to_string(),
            }));
        }
        Arc::new(OnlineDownload { shared })
    }

    /// Stops it soon; what was fetched is kept.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    pub fn state(&self) -> DownloadState {
        let s = &self.shared;
        let result = lock(&s.result).clone();
        let (total, cancelled, failure) = match &result {
            Some(Ok(d)) => (Some(d.total), d.cancelled, None),
            Some(Err(f)) => (None, false, Some(f.clone())),
            None => (None, false, None),
        };
        DownloadState {
            running: result.is_none(),
            account: s.status.account.load(Ordering::Relaxed) as u32,
            account_games: s.status.games.load(Ordering::Relaxed) as u64,
            added: result
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map_or(s.progress.load(Ordering::Relaxed) as u64, |d| d.added),
            months_done: s.status.months_done.load(Ordering::Relaxed) as u32,
            months_total: s.status.months_total.load(Ordering::Relaxed) as u32,
            total,
            cancelled,
            failure,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nicknames_and_targets() {
        assert!(valid_nickname(" Magnus_C-1 ".into()));
        assert!(!valid_nickname("../etc".into()));
        let dir = std::env::temp_dir().join(format!("mobile-online-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let li = OnlineAccount {
            site: Site::Lichess,
            username: "Magnus".into(),
            games: Some(10),
            months: None,
        };
        let cc = OnlineAccount {
            site: Site::ChessCom,
            username: "magnus".into(),
            games: None,
            months: Some(3),
        };
        let d = dir.to_string_lossy().into_owned();
        let one = online_targets(d.clone(), vec![li.clone(), cc.clone()], true);
        assert_eq!(one.len(), 1);
        assert!(one[0].path.ends_with("Magnus (Lichess + Chess.com).pgn"));
        assert!(!one[0].exists);
        let two = online_targets(d, vec![li, cc], false);
        assert_eq!(two.len(), 2);
        assert!(online_database(two[0].path.clone()).is_none());
    }

    #[test]
    fn a_download_that_cannot_write_says_so() {
        // A folder that cannot be made: the download ends with a disk problem.
        let acc = OnlineAccount {
            site: Site::Lichess,
            username: "someone".into(),
            games: None,
            months: None,
        };
        let d = OnlineDownload::start(vec![acc], "/proc/cp-no-such-dir/db.pgn".into());
        let mut s = d.state();
        for _ in 0..100 {
            if !s.running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            s = d.state();
        }
        assert!(!s.running);
        assert_eq!(s.failure.map(|f| f.problem), Some(OnlineProblem::Disk));
        assert_eq!(s.total, None);
    }
}
