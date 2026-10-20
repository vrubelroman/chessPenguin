//! A player's games from Lichess and Chess.com, by nickname (no login; anyone's: the
//! user's own, a pupil's, an opponent's): they become a PGN database in
//! `<local data dir>/online/` (one per site, or both sites in one), with a small state
//! file next to it so that "Update" fetches only the new games (and continues an
//! interrupted download). Both public APIs are probed at start; a platform that does not
//! answer as expected (offline, the API changed) is simply not offered.
//!
//! Lichess: `GET /api/games/user/{name}` as NDJSON (`pgnInJson`), oldest first, from
//! `since`; anonymous requests are throttled to 20 games a second. Chess.com: the list of
//! monthly archives, then each month's games as JSON (their PGN inside), from the last
//! month already fetched. Only standard chess is kept (variants are not supported).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    fn index(self) -> usize {
        match self {
            Platform::Lichess => 0,
            Platform::ChessCom => 1,
        }
    }
}

/// Lichess's anonymous export rate (games per second), for the time estimate.
pub const LICHESS_RATE: u64 = 20;

fn agent(stream: bool) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(format!(
            "ChessPenguin/{} (+https://chess.vrubel.xyz)",
            env!("CARGO_PKG_VERSION")
        ))
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // A long export is one response: hours for the body (a stalled one ends).
        .timeout_recv_body(if stream {
            Some(Duration::from_secs(4 * 3600))
        } else {
            Some(Duration::from_secs(60))
        })
        .build();
    ureq::Agent::new_with_config(config)
}

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

/// GET returning (status, body); the body of an error status is not read.
fn get(url: &str) -> Result<(u16, String), Failure> {
    let mut resp = agent(false)
        .get(url)
        .header("Accept", "application/json")
        .call()
        .map_err(|e| Failure::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if status == 429 {
        return Err(Failure::RateLimited);
    }
    if status != 200 {
        return Ok((status, String::new()));
    }
    let body = resp
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| Failure::Network(e.to_string()))?;
    Ok((status, body))
}

fn json(url: &str) -> Result<Option<Value>, Failure> {
    match get(url)? {
        (200, body) => serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| Failure::Unexpected(e.to_string())),
        (404, _) => Ok(None),
        (code, _) => Err(Failure::Unexpected(format!("HTTP {code}"))),
    }
}

/// Whether the platform answers the way this code expects: the profile of an account
/// that has always existed (Lichess's own, Chess.com's founder's).
pub fn probe(p: Platform) -> bool {
    let url = match p {
        Platform::Lichess => "https://lichess.org/api/user/lichess",
        Platform::ChessCom => "https://api.chess.com/pub/player/erik",
    };
    matches!(json(url), Ok(Some(v)) if probe_answer_ok(p, &v))
}

/// The probe's answer has the fields this code relies on.
fn probe_answer_ok(p: Platform, v: &Value) -> bool {
    match p {
        Platform::Lichess => {
            v["id"] == "lichess" && v["username"].is_string() && v["count"]["all"].is_u64()
        }
        Platform::ChessCom => v["player_id"].is_u64() && v["username"] == "erik",
    }
}

/// Both platforms probed (in parallel); `[lichess, chess.com]`.
pub fn probe_all() -> [bool; 2] {
    std::thread::scope(|s| {
        let l = s.spawn(|| probe(Platform::Lichess));
        let c = s.spawn(|| probe(Platform::ChessCom));
        [l.join().unwrap_or(false), c.join().unwrap_or(false)]
    })
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

/// Looks the nickname up on the platform.
pub fn lookup(p: Platform, nick: &str) -> Lookup {
    let nick = nick.trim();
    if !valid_nick(nick) {
        return Lookup::NotFound;
    }
    let result = match p {
        Platform::Lichess => json(&format!("https://lichess.org/api/user/{nick}")).map(|v| {
            v.filter(|v| v["disabled"] != true).and_then(|v| {
                Some(Account {
                    platform: p,
                    username: v["username"].as_str()?.to_owned(),
                    games: v["count"]["all"].as_u64(),
                    months: None,
                })
            })
        }),
        Platform::ChessCom => {
            let lower = nick.to_ascii_lowercase();
            json(&format!("https://api.chess.com/pub/player/{lower}")).and_then(|v| {
                let Some(username) = v.and_then(|v| v["username"].as_str().map(str::to_owned))
                else {
                    return Ok(None);
                };
                // Chess.com answers in lower case; the user's spelling reads better.
                let username = if username.eq_ignore_ascii_case(nick) {
                    nick.to_owned()
                } else {
                    username
                };
                let months = archives(&lower)?.len();
                Ok(Some(Account {
                    platform: p,
                    username,
                    games: None,
                    months: Some(months),
                }))
            })
        }
    };
    match result {
        Ok(Some(a)) => Lookup::Found(a),
        Ok(None) => Lookup::NotFound,
        Err(e) => Lookup::Failed(e),
    }
}

/// Chess.com's monthly archive URLs of a player, oldest first.
fn archives(lower: &str) -> Result<Vec<String>, Failure> {
    let v = json(&format!(
        "https://api.chess.com/pub/player/{lower}/games/archives"
    ))?
    .ok_or_else(|| Failure::Unexpected("no archives".into()))?;
    let list = v["archives"]
        .as_array()
        .ok_or_else(|| Failure::Unexpected("archives".into()))?;
    Ok(list
        .iter()
        .filter_map(|u| u.as_str().map(str::to_owned))
        .collect())
}

/// Where the online databases live.
pub fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("online"))
}

/// The database file of one account: "Name (Lichess).pgn".
pub fn database_path(p: Platform, username: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(format!("{username} ({}).pgn", p.name())))
}

/// The database of several accounts in one: "Name (Lichess + Chess.com).pgn" when the
/// nicknames are the same, "Name (Lichess) + Other (Chess.com).pgn" otherwise (so that
/// another set of accounts never lands in the same file).
pub fn combined_path(accounts: &[Account]) -> Option<PathBuf> {
    let first = accounts.first()?;
    if accounts.len() == 1 {
        return database_path(first.platform, &first.username);
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
    dir().map(|d| d.join(format!("{name}.pgn")))
}

/// The online databases already holding `account` (several when it was downloaded
/// alone and together with another one), those with more accounts first.
pub fn holding(account: &Account) -> Vec<PathBuf> {
    let Some(dir) = dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
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

/// How many last bytes of the file the state remembers.
const TAIL: usize = 64;

impl State {
    pub fn load(pgn: &Path) -> Option<State> {
        let text = std::fs::read_to_string(state_path(pgn)).ok()?;
        serde_json::from_str::<State>(&text)
            .ok()
            .filter(|s| !s.accounts.is_empty() && s.accounts.iter().all(|a| a.platform.is_some()))
    }

    fn save(&self, pgn: &Path) -> Result<(), Failure> {
        let text = serde_json::to_string_pretty(self).map_err(|e| Failure::Disk(e.to_string()))?;
        let path = state_path(pgn);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| Failure::Disk(e.to_string()))
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

/// An online database (it has a state file).
pub fn is_online(pgn: &Path) -> bool {
    dir().is_some_and(|d| pgn.parent() == Some(d.as_path())) && state_path(pgn).exists()
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// "5 min ago", "3 h ago", "2 d ago" for a time in Unix seconds.
pub fn ago(t: i64) -> String {
    use crate::i18n::trf;
    let s = (now_secs() - t).max(0);
    match s {
        0..3600 => trf("{n} min ago", &[("n", &(s / 60).max(1))]),
        3600..86400 => trf("{n} h ago", &[("n", &(s / 3600))]),
        _ => trf("{n} d ago", &[("n", &(s / 86400))]),
    }
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

/// The databases being written now: one download per file at a time.
static WRITING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Holds a database for one download; released when dropped.
struct Claim(PathBuf);

impl Claim {
    fn take(path: &Path) -> Option<Claim> {
        let mut w = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        if w.iter().any(|p| p == path) {
            return None;
        }
        w.push(path.to_owned());
        Some(Claim(path.to_owned()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut w = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        w.retain(|p| *p != self.0);
    }
}

/// Appends games to the database file and keeps the state in step.
struct Writer {
    path: PathBuf,
    file: std::io::BufWriter<std::fs::File>,
    state: State,
    added: u64,
    /// The file's length and last bytes as written (flushed or not).
    len: u64,
    tail: Vec<u8>,
}

impl Writer {
    /// Opens the database for appending; a state that does not match the file (the file
    /// is gone, or a crash left games after the state) is put right first.
    fn open(path: &Path, mut state: State) -> Result<Writer, Failure> {
        let disk = |e: std::io::Error| Failure::Disk(e.to_string());
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(disk)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)
            .map_err(disk)?;
        let mut len = file.metadata().map_err(disk)?.len();
        if let Some(b) = state.bytes
            && len > b
            && b as usize >= state.tail.len()
        {
            let mut end = vec![0; state.tail.len()];
            use std::io::{Read, Seek, SeekFrom};
            let mut f = &file;
            f.seek(SeekFrom::Start(b - state.tail.len() as u64))
                .and_then(|_| f.read_exact(&mut end))
                .map_err(disk)?;
            if end == state.tail {
                file.set_len(b).map_err(disk)?;
                len = b;
            }
        }
        let tail = if len == state.bytes.unwrap_or(u64::MAX) {
            std::mem::take(&mut state.tail)
        } else {
            Vec::new()
        };
        Ok(Writer {
            path: path.to_owned(),
            file: std::io::BufWriter::new(file),
            state,
            added: 0,
            len,
            tail,
        })
    }

    fn game(&mut self, pgn: &str) -> Result<(), Failure> {
        let pgn = pgn.trim();
        if pgn.is_empty() {
            return Ok(());
        }
        for part in [pgn.as_bytes(), b"\n\n"] {
            self.file
                .write_all(part)
                .map_err(|e| Failure::Disk(e.to_string()))?;
            self.len += part.len() as u64;
            self.tail.extend_from_slice(part);
        }
        if self.tail.len() > TAIL {
            self.tail.drain(..self.tail.len() - TAIL);
        }
        self.added += 1;
        self.state.games += 1;
        Ok(())
    }

    /// Makes what was written durable, the state last (it never runs ahead of the file).
    fn commit(&mut self) -> Result<(), Failure> {
        self.file
            .flush()
            .map_err(|e| Failure::Disk(e.to_string()))?;
        self.state.bytes = Some(self.len);
        self.state.tail = self.tail.clone();
        self.state.save(&self.path)
    }

    fn done(mut self, cancelled: bool) -> Result<Done, Failure> {
        if !cancelled {
            self.state.updated = Some(now_secs());
        }
        self.commit()?;
        Ok(Done {
            path: self.path,
            added: self.added,
            total: self.state.games,
            cancelled,
        })
    }
}

/// Fetches the games of `accounts` new since the last run into the database at `path`
/// (one database for all of them); `progress` counts the games added, `status` tells
/// what is being fetched. Cancelling keeps what was fetched. One download per database
/// at a time ([`Failure::Busy`] otherwise).
pub fn download(
    accounts: &[Account],
    path: &Path,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
    status: &Progress,
) -> Result<Done, Failure> {
    let _claim = Claim::take(path).ok_or(Failure::Busy)?;
    let path = path.to_owned();
    let mut state = State::load(&path)
        // The games are gone (the file was deleted): fetch them again.
        .filter(|s| path.exists() || s.games == 0)
        .unwrap_or_else(|| {
            // A database without its state: start it anew rather than append duplicates.
            let _ = std::fs::remove_file(&path);
            State::default()
        });
    // Accounts not in the database yet start from their first game.
    for a in accounts {
        if !state.holds(a) {
            state.accounts.push(AccountState {
                platform: Some(a.platform),
                username: a.username.clone(),
                ..AccountState::default()
            });
        }
    }
    let mut w = Writer::open(&path, state)?;
    let mut result = Ok(false);
    for i in 0..w.state.accounts.len() {
        let Some(k) = accounts.iter().position(|a| w.state.accounts[i].is(a)) else {
            continue;
        };
        status.account.store(k, Ordering::Relaxed);
        status.games.store(0, Ordering::Relaxed);
        result = match w.state.accounts[i].platform {
            Some(Platform::Lichess) => lichess(&mut w, i, progress, cancel, status),
            Some(Platform::ChessCom) => chesscom(&mut w, i, progress, cancel, status),
            None => Ok(false),
        };
        if !matches!(result, Ok(false)) {
            break;
        }
    }
    match result {
        Ok(c) => w.done(c),
        Err(e) => {
            // Keep what arrived before the failure; the next update continues.
            let _ = w.commit();
            Err(e)
        }
    }
}

#[cfg(test)]
fn download_to(
    accounts: &[Account],
    path: &Path,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Done, Failure> {
    download(accounts, path, progress, cancel, &Progress::default())
}

/// Lichess's answer for one game, as far as the download cares.
struct LichessGame<'a> {
    id: &'a str,
    created: i64,
    /// Still being played (not exported as a game yet).
    playing: bool,
    standard: bool,
    pgn: &'a str,
}

fn lichess_game(v: &Value) -> Option<LichessGame<'_>> {
    Some(LichessGame {
        id: v["id"].as_str()?,
        created: v["createdAt"].as_i64()?,
        playing: matches!(v["status"].as_str(), Some("created" | "started")),
        standard: v["variant"]
            .as_str()
            .is_none_or(|x| x == "standard" || x == "fromPosition"),
        pgn: v["pgn"].as_str()?,
    })
}

/// What to do with a game of the Lichess export: `Some(write?)`, given the newest game
/// fetched before this run and the games pending then (unfinished). The export repeats
/// games from the oldest pending one on; those fetched already are skipped.
fn lichess_keep(a: &mut AccountState, g: &LichessGame, before: Option<i64>) -> bool {
    let pending = a.lichess_pending.iter().position(|(id, _)| id == g.id);
    if g.playing {
        if pending.is_none() && g.standard {
            a.lichess_pending.push((g.id.to_owned(), g.created));
        }
        return false;
    }
    if let Some(k) = pending {
        a.lichess_pending.remove(k);
        return g.standard;
    }
    g.standard && before.is_none_or(|b| g.created > b)
}

/// The games of account `i` of the writer's database from Lichess. `Ok(true)` when
/// cancelled.
fn lichess(
    w: &mut Writer,
    i: usize,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
    status: &Progress,
) -> Result<bool, Failure> {
    let a = &w.state.accounts[i];
    let before = a.lichess_last;
    let from = a
        .lichess_pending
        .iter()
        .map(|(_, t)| *t)
        .chain(before.map(|b| b + 1))
        .min();
    let pending_before: Vec<String> = a.lichess_pending.iter().map(|(id, _)| id.clone()).collect();
    let mut url = format!(
        "https://lichess.org/api/games/user/{}?sort=dateAsc&finished=false&pgnInJson=true\
         &perfType=ultraBullet,bullet,blitz,rapid,classical,correspondence\
         &clocks=false&evals=false&opening=true",
        a.username
    );
    if let Some(from) = from {
        url.push_str(&format!("&since={from}"));
    }
    let resp = agent(true)
        .get(&url)
        .header("Accept", "application/x-ndjson")
        .call()
        .map_err(|e| Failure::Network(e.to_string()))?;
    match resp.status().as_u16() {
        200 => {}
        429 => return Err(Failure::RateLimited),
        code => return Err(Failure::Unexpected(format!("HTTP {code}"))),
    }
    let reader = BufReader::new(resp.into_body().into_reader());
    let mut seen: Vec<String> = Vec::new();
    for (n, line) in reader.lines().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let line = line.map_err(|e| Failure::Network(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value =
            serde_json::from_str(&line).map_err(|e| Failure::Unexpected(e.to_string()))?;
        let g = lichess_game(&v)
            .ok_or_else(|| Failure::Unexpected("a game without id, pgn or createdAt".into()))?;
        if pending_before.iter().any(|id| id == g.id) {
            seen.push(g.id.to_owned());
        }
        let a = &mut w.state.accounts[i];
        if lichess_keep(a, &g, before) {
            let pgn = g.pgn.to_owned();
            w.game(&pgn)?;
            progress.fetch_add(1, Ordering::Relaxed);
            status.games.fetch_add(1, Ordering::Relaxed);
        }
        let a = &mut w.state.accounts[i];
        a.lichess_last = Some(a.lichess_last.map_or(g.created, |l| l.max(g.created)));
        if n % 100 == 99 {
            w.commit()?;
        }
    }
    // The whole export arrived: a game pending before that did not come again is gone
    // (deleted, or a variant that ended).
    w.state.accounts[i]
        .lichess_pending
        .retain(|(id, _)| !pending_before.contains(id) || seen.contains(id));
    Ok(false)
}

/// The games of account `i` of the writer's database from Chess.com. `Ok(true)` when
/// cancelled.
fn chesscom(
    w: &mut Writer,
    i: usize,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
    status: &Progress,
) -> Result<bool, Failure> {
    let lower = w.state.accounts[i].username.to_ascii_lowercase();
    let months = archives(&lower)?;
    status.months_total.store(months.len(), Ordering::Relaxed);
    status.months_done.store(0, Ordering::Relaxed);
    for url in months {
        status.months_done.fetch_add(1, Ordering::Relaxed);
        // ".../games/2026/10" → "2026/10"; months before the last fetched are done.
        let month = url.rsplitn(3, '/').take(2).collect::<Vec<_>>();
        let month = format!(
            "{}/{}",
            month.get(1).unwrap_or(&""),
            month.first().unwrap_or(&"")
        );
        let a = &w.state.accounts[i];
        if a.chesscom_month.as_ref().is_some_and(|m| month < *m) {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let v = json(&url)?.ok_or_else(|| Failure::Unexpected(format!("{month}: not found")))?;
        let games = v["games"]
            .as_array()
            .ok_or_else(|| Failure::Unexpected(format!("{month}: no games")))?;
        let mut list: Vec<(i64, &str)> = games
            .iter()
            .filter(|g| g["rules"] == "chess")
            .filter_map(|g| Some((g["end_time"].as_i64()?, g["pgn"].as_str()?)))
            .collect();
        list.sort_by_key(|(t, _)| *t);
        let a = &w.state.accounts[i];
        let same_month = a.chesscom_month.as_deref() == Some(month.as_str());
        let last = if same_month {
            a.chesscom_last_end
        } else {
            None
        };
        for (end, pgn) in list {
            if last.is_some_and(|l| end <= l) {
                continue;
            }
            w.game(pgn)?;
            progress.fetch_add(1, Ordering::Relaxed);
            status.games.fetch_add(1, Ordering::Relaxed);
            w.state.accounts[i].chesscom_last_end = Some(end);
        }
        // The mark moves to this month even when it had no standard games (the end
        // time then still belongs to an earlier month and is not compared).
        w.state.accounts[i].chesscom_month = Some(month);
        w.commit()?;
    }
    Ok(false)
}

/// The probe results and what the window shows, kept by the application.
pub struct Probe {
    job: Option<std::sync::mpsc::Receiver<[bool; 2]>>,
    pub available: [bool; 2],
    /// When the last probe finished (a platform that did not answer is asked again
    /// after [`Probe::RETRY`]: the machine may have come online).
    last: Option<std::time::Instant>,
    ctx: egui::Context,
}

impl Probe {
    const RETRY: Duration = Duration::from_secs(300);

    /// Starts probing both platforms in the background.
    pub fn start(ctx: &egui::Context) -> Probe {
        let mut p = Probe {
            job: None,
            available: [false; 2],
            last: None,
            ctx: ctx.clone(),
        };
        p.spawn();
        p
    }

    fn spawn(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("online-probe".into())
            .spawn(move || {
                let _ = tx.send(probe_all());
                ctx.request_repaint();
            });
        self.job = spawned.ok().map(|_| rx);
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.job
            && let Ok(a) = rx.try_recv()
        {
            // A platform that answered once stays offered for the session (a later
            // failure shows as an error in the window).
            for (have, now) in self.available.iter_mut().zip(a) {
                *have |= now;
            }
            self.job = None;
            self.last = Some(std::time::Instant::now());
        }
        if self.job.is_none() && !self.available.iter().all(|a| *a) {
            match self.last {
                Some(t) if t.elapsed() >= Self::RETRY => self.spawn(),
                // Wake up for the retry even when nothing else repaints.
                Some(t) => self.ctx.request_repaint_after(Self::RETRY - t.elapsed()),
                None => {}
            }
        }
    }

    pub fn any(&self) -> bool {
        self.available.iter().any(|a| *a)
    }

    pub fn has(&self, p: Platform) -> bool {
        self.available[p.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cp-online-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn account(p: Platform, name: &str) -> Account {
        Account {
            platform: p,
            username: name.into(),
            games: None,
            months: None,
        }
    }

    const GAME: &str = "[Event \"x\"]\n\n1. e4 e5 1-0";

    #[test]
    fn a_crash_leaves_no_duplicates() {
        let dir = temp_dir("crash");
        let path = dir.join("db.pgn");
        let state = State {
            accounts: vec![AccountState {
                platform: Some(Platform::Lichess),
                username: "a".into(),
                ..AccountState::default()
            }],
            ..State::default()
        };
        let mut w = Writer::open(&path, state).unwrap();
        w.game(GAME).unwrap();
        w.game(GAME).unwrap();
        w.commit().unwrap();
        let good = std::fs::read(&path).unwrap();
        // Games written after the last commit, then the process died.
        w.game(GAME).unwrap();
        w.file.flush().unwrap();
        drop(w);
        assert!(std::fs::read(&path).unwrap().len() > good.len());
        let w = Writer::open(&path, State::load(&path).unwrap()).unwrap();
        assert_eq!(w.state.games, 2);
        drop(w);
        assert_eq!(std::fs::read(&path).unwrap(), good, "cut back to the state");

        // Somebody else changed the file: it is left alone.
        let mut other = good.clone();
        other[0] = b' ';
        other.extend_from_slice(GAME.as_bytes());
        std::fs::write(&path, &other).unwrap();
        let w = Writer::open(&path, State::load(&path).unwrap()).unwrap();
        drop(w);
        assert_eq!(std::fs::read(&path).unwrap(), other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_database_is_downloaded_again() {
        let dir = temp_dir("deleted");
        let path = dir.join("db.pgn");
        let state = State {
            accounts: vec![AccountState {
                platform: Some(Platform::Lichess),
                username: "a".into(),
                lichess_last: Some(5),
                ..AccountState::default()
            }],
            games: 7,
            ..State::default()
        };
        state.save(&path).unwrap();
        let never = AtomicBool::new(false);
        // No account asked for: nothing fetched, only the state put right.
        let d = download_to(&[], &path, &AtomicUsize::new(0), &never).unwrap();
        assert_eq!(d.total, 0);
        let s = State::load(&path).unwrap_or_default();
        assert!(s.accounts.iter().all(|a| a.lichess_last.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_download_per_database() {
        let path = PathBuf::from("/nonexistent/cp-claim.pgn");
        let first = Claim::take(&path).expect("free");
        assert!(Claim::take(&path).is_none());
        let never = AtomicBool::new(false);
        assert_eq!(
            download_to(&[], &path, &AtomicUsize::new(0), &never),
            Err(Failure::Busy)
        );
        drop(first);
        assert!(Claim::take(&path).is_some());
    }

    #[test]
    fn names_of_merged_databases() {
        let name = |a: &[Account]| {
            combined_path(a)
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        let li = account(Platform::Lichess, "Magnus");
        assert_eq!(name(std::slice::from_ref(&li)), "Magnus (Lichess).pgn");
        let cc = account(Platform::ChessCom, "magnus");
        assert_eq!(name(&[li.clone(), cc]), "Magnus (Lichess + Chess.com).pgn");
        let other = account(Platform::ChessCom, "MagnusCarlsen");
        assert_eq!(
            name(&[li, other]),
            "Magnus (Lichess) + MagnusCarlsen (Chess.com).pgn"
        );
    }

    #[test]
    fn unfinished_lichess_games_come_later() {
        let mut a = AccountState::default();
        let g = |id: &'static str, created: i64, playing: bool| LichessGame {
            id,
            created,
            playing,
            standard: true,
            pgn: "",
        };
        // First run: a correspondence game is still on, a later one is over.
        assert!(!lichess_keep(&mut a, &g("corr", 10, true), None));
        assert!(lichess_keep(&mut a, &g("blitz", 20, false), None));
        assert_eq!(a.lichess_pending, vec![("corr".to_owned(), 10)]);
        // The next run starts from the pending game: it is over now and is kept, the
        // game fetched before is not repeated.
        assert!(lichess_keep(&mut a, &g("corr", 10, false), Some(20)));
        assert!(!lichess_keep(&mut a, &g("blitz", 20, false), Some(20)));
        assert!(lichess_keep(&mut a, &g("new", 30, false), Some(20)));
        assert!(a.lichess_pending.is_empty());
        // A variant game is neither kept nor waited for.
        let mut v = g("960", 40, true);
        v.standard = false;
        assert!(!lichess_keep(&mut a, &v, Some(30)));
        assert!(a.lichess_pending.is_empty());
    }

    /// Against the real sites, with an account given by `CP_ONLINE_NICK` (kept out of
    /// the repository): lookup, an interrupted Lichess download continued without gaps
    /// or duplicates, a full Chess.com download, and an update that adds nothing. Run
    /// with `CP_ONLINE_NICK=<nick> cargo test -p chesspenguin online_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn online_live() {
        let Ok(nick) = std::env::var("CP_ONLINE_NICK") else {
            eprintln!("skipped: no CP_ONLINE_NICK");
            return;
        };
        assert_eq!(probe_all(), [true, true], "both platforms answer");
        assert_eq!(
            lookup(Platform::Lichess, "no-such-player-cp-x9"),
            Lookup::NotFound
        );
        let dir = std::env::temp_dir().join(format!("cp-online-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let read_db = |path: &Path| {
            let db = chess_core::PgnDatabase::open(path, &mut |_| {}, &AtomicBool::new(false))
                .expect("opens");
            let mut sites = Vec::new();
            let mut problems = 0;
            for i in 0..db.len() {
                let g = db.load_game(i).expect("game");
                problems += usize::from(!g.diagnostics.is_empty());
                let e = &db.entries()[i];
                let site = e.tag("Link").or_else(|| e.tag("Site")).unwrap_or_default();
                sites.push(site.to_owned());
            }
            (db.len(), sites, problems)
        };

        // Lichess: stop after about 150 games, twice, then compare with what the
        // site says about those games: no gaps, no duplicates.
        let Lookup::Found(li) = lookup(Platform::Lichess, &nick) else {
            panic!("not on Lichess");
        };
        let path = dir.join("li.pgn");
        for round in 1..=2 {
            let progress = AtomicUsize::new(0);
            let cancel = AtomicBool::new(false);
            let done = std::thread::scope(|s| {
                s.spawn(|| {
                    while progress.load(Ordering::Relaxed) < 150 {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    cancel.store(true, Ordering::Relaxed);
                });
                download_to(std::slice::from_ref(&li), &path, &progress, &cancel)
                    .expect("downloads")
            });
            eprintln!("lichess round {round}: {done:?}");
            assert!(done.cancelled && done.added >= 150);
        }
        let (n, sites, problems) = read_db(&path);
        let unique: std::collections::HashSet<_> = sites.iter().collect();
        assert_eq!(unique.len(), n, "no duplicates");
        assert_eq!(State::load(&path).unwrap().games as usize, n);
        eprintln!("lichess: {n} games after two interrupted runs, {problems} with diagnostics");
        // The same games as one uninterrupted request for that many.
        let (code, body) = get(&format!(
            "https://lichess.org/api/games/user/{}?sort=dateAsc&finished=true&max={n}\
             &perfType=ultraBullet,bullet,blitz,rapid,classical,correspondence&moves=false",
            li.username
        ))
        .expect("answers");
        assert_eq!(code, 200);
        // Asked for JSON, Lichess answers one game per line.
        let expected: Vec<String> = body
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| Some(format!("https://lichess.org/{}", v["id"].as_str()?)))
            .collect();
        // Ours are the reference's first games, in its order (the reference may hold a
        // few more: Lichess does not count `max` exactly as we count games).
        if expected.len() < sites.len() || sites[..] != expected[..sites.len()] {
            let only_ours: Vec<_> = sites.iter().filter(|x| !expected.contains(x)).collect();
            let only_ref: Vec<_> = expected.iter().filter(|x| !sites.contains(x)).collect();
            let first = sites.iter().zip(&expected).position(|(a, b)| a != b);
            panic!(
                "differ: first at {first:?}, ours {} ref {}, only ours {only_ours:?}, only in the reference {only_ref:?}",
                sites.len(),
                expected.len()
            );
        }

        // Chess.com: everything, then an update that finds nothing new.
        let Lookup::Found(cc) = lookup(Platform::ChessCom, &nick) else {
            panic!("not on Chess.com");
        };
        let path = dir.join("cc.pgn");
        let never = AtomicBool::new(false);
        let done = download_to(
            std::slice::from_ref(&cc),
            &path,
            &AtomicUsize::new(0),
            &never,
        )
        .expect("downloads");
        eprintln!("chess.com: {done:?}");
        assert!(!done.cancelled && done.total > 0);
        let again = download_to(
            std::slice::from_ref(&cc),
            &path,
            &AtomicUsize::new(0),
            &never,
        )
        .expect("updates");
        assert_eq!((again.added, again.total), (0, done.total));
        let (n, sites, problems) = read_db(&path);
        assert_eq!(n as u64, done.total);
        let unique: std::collections::HashSet<_> = sites.iter().collect();
        assert_eq!(unique.len(), n, "no duplicates");
        eprintln!("chess.com: {n} games, {problems} with diagnostics");

        // Both into one database: Chess.com whole, then Lichess stopped twice; the
        // second run adds Lichess games only, without duplicates.
        let path = dir.join("both.pgn");
        let accounts = [cc.clone(), li.clone()];
        let mut totals = Vec::new();
        for round in 1..=2 {
            let progress = AtomicUsize::new(0);
            let cancel = AtomicBool::new(false);
            let goal = if round == 1 {
                done.total as usize + 100
            } else {
                100
            };
            let d = std::thread::scope(|s| {
                s.spawn(|| {
                    while progress.load(Ordering::Relaxed) < goal {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    cancel.store(true, Ordering::Relaxed);
                });
                download_to(&accounts, &path, &progress, &cancel).expect("downloads")
            });
            eprintln!("both, round {round}: {d:?}");
            assert!(d.cancelled);
            totals.push(d.total);
        }
        let state = State::load(&path).unwrap();
        assert_eq!(state.accounts.len(), 2);
        assert_eq!(state.accounts[0].platform, Some(Platform::ChessCom));
        assert!(state.accounts[0].chesscom_month.is_some());
        assert!(state.accounts[1].lichess_last.is_some());
        let (n, sites, problems) = read_db(&path);
        assert_eq!(n as u64, totals[1]);
        let unique: std::collections::HashSet<_> = sites.iter().collect();
        assert_eq!(unique.len(), n, "no duplicates");
        let chesscom = sites.iter().filter(|s| s.contains("chess.com")).count() as u64;
        assert_eq!(chesscom, done.total, "all of Chess.com, once");
        eprintln!("both: {n} games ({chesscom} from Chess.com), {problems} with diagnostics");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_answers() {
        let li: Value = serde_json::from_str(
            r#"{"id":"lichess","username":"Lichess","count":{"all":0,"rated":0}}"#,
        )
        .unwrap();
        assert!(probe_answer_ok(Platform::Lichess, &li));
        // A changed API (renamed or missing fields) is not offered.
        let changed: Value =
            serde_json::from_str(r#"{"id":"lichess","username":"Lichess","games":0}"#).unwrap();
        assert!(!probe_answer_ok(Platform::Lichess, &changed));
        let cc: Value =
            serde_json::from_str(r#"{"player_id":41,"username":"erik","status":"staff"}"#).unwrap();
        assert!(probe_answer_ok(Platform::ChessCom, &cc));
        let html = Value::String("<html>maintenance</html>".into());
        assert!(!probe_answer_ok(Platform::ChessCom, &html));
        assert!(!probe_answer_ok(Platform::Lichess, &html));
    }

    #[test]
    fn nicknames() {
        assert!(valid_nick("Magnus_C-1"));
        assert!(!valid_nick("a"));
        assert!(!valid_nick("bad nick"));
        assert!(!valid_nick("../etc"));
        assert!(!valid_nick(&"x".repeat(31)));
    }

    #[test]
    fn state_round_trip() {
        let dir = std::env::temp_dir().join(format!("cp-online-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pgn = dir.join("Someone (Lichess).pgn");
        let s = State {
            accounts: vec![AccountState {
                platform: Some(Platform::Lichess),
                username: "Someone".into(),
                lichess_last: Some(1_700_000_000_000),
                ..AccountState::default()
            }],
            games: 3,
            ..State::default()
        };
        s.save(&pgn).unwrap();
        assert_eq!(State::load(&pgn), Some(s));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
