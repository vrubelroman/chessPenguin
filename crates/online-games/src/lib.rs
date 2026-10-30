//! A player's games from Lichess and Chess.com, by nickname (no login; anyone's: the
//! user's own, a pupil's, an opponent's): they become a PGN database in a folder of the
//! app (one per site, or both sites in one), with a small state file next to it so that
//! "Update" fetches only the new games (and continues an interrupted download). The
//! desktop and the Android app share this; each passes its own folder.
//!
//! Lichess: `GET /api/games/user/{name}` as NDJSON (`pgnInJson`), oldest first, from
//! `since`; anonymous requests are throttled to 20 games a second. Chess.com: the list of
//! monthly archives, then each month's games as JSON (their PGN inside), from the last
//! month already fetched. Only standard chess is kept (variants are not supported).
//!
//! The network part (`net.rs`: lookups, downloads) is built where requests can block
//! a thread: not in a browser.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Lichess,
    ChessCom,
}

impl Platform {
    pub const ALL: [Platform; 2] = [Platform::Lichess, Platform::ChessCom];

    pub fn name(self) -> &'static str {
        match self {
            Platform::Lichess => "Lichess",
            Platform::ChessCom => "Chess.com",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Platform::Lichess => 0,
            Platform::ChessCom => 1,
        }
    }
}

/// Lichess's anonymous export rate (games per second), for the time estimate.
pub const LICHESS_RATE: u64 = 20;

#[cfg(not(target_family = "wasm"))]
mod net;
#[cfg(not(target_family = "wasm"))]
pub use net::{download, lookup, probe, probe_all};

/// What went wrong talking to a platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No answer (offline, DNS, TLS, timeout).
    Network(String),
    /// "Too many requests": the platform asks to wait.
    RateLimited,
    /// An answer we do not understand (the API changed).
    Unexpected(String),
    /// Writing the database failed.
    Disk(String),
    /// The database is being downloaded already.
    Busy,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Network(e) => write!(f, "network: {e}"),
            Failure::RateLimited => write!(f, "rate limited"),
            Failure::Unexpected(e) => write!(f, "unexpected answer: {e}"),
            Failure::Disk(e) => write!(f, "{e}"),
            Failure::Busy => write!(f, "the database is being downloaded already"),
        }
    }
}

/// A nickname both platforms could have: 2–30 letters, digits, `_` or `-`.
pub fn valid_nick(nick: &str) -> bool {
    (2..=30).contains(&nick.len())
        && nick
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// An account found by nickname.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub platform: Platform,
    /// As the platform spells it.
    pub username: String,
    /// Lichess: games of the account (all kinds, so an upper bound for ours).
    pub games: Option<u64>,
    /// Chess.com: months with games.
    pub months: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(Account),
    NotFound,
    Failed(Failure),
}

/// The database file of one account in `dir`: "Name (Lichess).pgn".
pub fn database_path(dir: &Path, p: Platform, username: &str) -> PathBuf {
    dir.join(format!("{username} ({}).pgn", p.name()))
}

/// The database of several accounts in one: "Name (Lichess + Chess.com).pgn" when the
/// nicknames are the same, "Name (Lichess) + Other (Chess.com).pgn" otherwise (so that
/// another set of accounts never lands in the same file).
pub fn combined_path(dir: &Path, accounts: &[Account]) -> Option<PathBuf> {
    let first = accounts.first()?;
    if accounts.len() == 1 {
        return Some(database_path(dir, first.platform, &first.username));
    }
    let same = accounts
        .iter()
        .all(|a| a.username.eq_ignore_ascii_case(&first.username));
    let name = if same {
        let platforms = accounts
            .iter()
            .map(|a| a.platform.name())
            .collect::<Vec<_>>()
            .join(" + ");
        format!("{} ({platforms})", first.username)
    } else {
        accounts
            .iter()
            .map(|a| format!("{} ({})", a.username, a.platform.name()))
            .collect::<Vec<_>>()
            .join(" + ")
    };
    Some(dir.join(format!("{name}.pgn")))
}

/// The online databases already holding `account` (several when it was downloaded
/// alone and together with another one), those with more accounts first.
pub fn holding(dir: &Path, account: &Account) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(usize, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pgn"))
        .filter_map(|p| {
            let s = State::load(&p)?;
            s.holds(account).then_some((s.accounts.len(), p))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, p)| p).collect()
}

fn state_path(pgn: &Path) -> PathBuf {
    pgn.with_extension("online.json")
}

/// What has been fetched from one account into a database.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountState {
    pub platform: Option<Platform>,
    pub username: String,
    /// Lichess: creation time (ms) of the newest game fetched.
    pub lichess_last: Option<i64>,
    /// Lichess: games still being played when last fetched (id, creation time ms):
    /// the next update asks again from the oldest of them, so that a long
    /// (correspondence) game is fetched once it ends.
    #[serde(default)]
    pub lichess_pending: Vec<(String, i64)>,
    /// Chess.com: the newest month fetched ("2026/10") and the end time (s) of its
    /// last game fetched.
    pub chesscom_month: Option<String>,
    pub chesscom_last_end: Option<i64>,
}

/// What has been fetched into a database (from one account or several), for the next
/// update.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    pub accounts: Vec<AccountState>,
    /// Games written so far.
    pub games: u64,
    /// When the last download finished (Unix seconds).
    #[serde(default)]
    pub updated: Option<i64>,
    /// The length of the database file when the state was saved, and its last bytes:
    /// what a crash appended after that is cut off before the next download (only if
    /// the file still ends the same way there, i.e. nobody else changed it).
    #[serde(default)]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub tail: Vec<u8>,
}

impl State {
    pub fn load(pgn: &Path) -> Option<State> {
        let text = std::fs::read_to_string(state_path(pgn)).ok()?;
        serde_json::from_str::<State>(&text)
            .ok()
            .filter(|s| !s.accounts.is_empty() && s.accounts.iter().all(|a| a.platform.is_some()))
    }

    fn holds(&self, a: &Account) -> bool {
        self.accounts.iter().any(|s| s.is(a))
    }

    /// The accounts of the database, to update it.
    pub fn accounts(&self) -> Vec<Account> {
        self.accounts
            .iter()
            .filter_map(|a| {
                Some(Account {
                    platform: a.platform?,
                    username: a.username.clone(),
                    games: None,
                    months: None,
                })
            })
            .collect()
    }
}

impl AccountState {
    fn is(&self, a: &Account) -> bool {
        self.platform == Some(a.platform) && self.username.eq_ignore_ascii_case(&a.username)
    }
}

impl Account {
    /// The same account (the counts aside).
    pub fn same(&self, other: &Account) -> bool {
        self.platform == other.platform && self.username.eq_ignore_ascii_case(&other.username)
    }
}

/// An online database of `dir` (it has a state file).
pub fn is_online(dir: &Path, pgn: &Path) -> bool {
    pgn.parent() == Some(dir) && state_path(pgn).exists()
}

/// Where the found accounts go: all into one database (`together`) or each into its
/// own. A database already holding the accounts is kept (also when one site does not
/// answer this time: its partner's games still go where they went before).
pub fn targets(dir: &Path, start: &[Account], together: bool) -> Vec<(Vec<Account>, PathBuf)> {
    let groups: Vec<Vec<Account>> = if together {
        vec![start.to_vec()]
    } else {
        start.iter().map(|a| vec![a.clone()]).collect()
    };
    // Only one account found: any database holding it (the merged one first).
    let any = start.len() == 1;
    groups
        .into_iter()
        .filter_map(|g| {
            let existing = holding(dir, &g[0]).into_iter().find(|p| {
                State::load(p).is_some_and(|s| {
                    let mine = s.accounts();
                    any || (mine.len() == g.len()
                        && g.iter().all(|a| mine.iter().any(|m| m.same(a))))
                })
            });
            let path = existing.or_else(|| combined_path(dir, &g))?;
            Some((g, path))
        })
        .collect()
}

/// What a running download is doing, for the progress line: the account (its index in
/// the accounts passed to [`download`]), the games of that account so far, and for
/// Chess.com the months done of all.
#[derive(Debug, Default)]
pub struct Progress {
    pub account: AtomicUsize,
    pub games: AtomicUsize,
    pub months_done: AtomicUsize,
    pub months_total: AtomicUsize,
}

/// Now, in Unix seconds.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The result of a download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub path: PathBuf,
    /// Games added by this run, and in the database now.
    pub added: u64,
    pub total: u64,
    /// Stopped by the user; what was fetched is kept and the next update continues.
    pub cancelled: bool,
}
