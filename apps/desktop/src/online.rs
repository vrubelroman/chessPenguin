//! The user's games from Lichess and Chess.com, by nickname (no login): each platform's
//! games become a PGN database of their own in `<local data dir>/online/`, with a small
//! state file next to it so that "Update" fetches only the new games (and continues an
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
        // A long export is one response: no limit on the body, only on the rest.
        .timeout_recv_body(if stream {
            None
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
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Network(e) => write!(f, "network: {e}"),
            Failure::RateLimited => write!(f, "rate limited"),
            Failure::Unexpected(e) => write!(f, "unexpected answer: {e}"),
            Failure::Disk(e) => write!(f, "{e}"),
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

/// The database of several accounts in one: "Name (Lichess + Chess.com).pgn", named
/// after the first account.
pub fn combined_path(accounts: &[Account]) -> Option<PathBuf> {
    let first = accounts.first()?;
    if accounts.len() == 1 {
        return database_path(first.platform, &first.username);
    }
    let platforms = accounts
        .iter()
        .map(|a| a.platform.name())
        .collect::<Vec<_>>()
        .join(" + ");
    dir().map(|d| d.join(format!("{} ({platforms}).pgn", first.username)))
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
}

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

/// An online database (it has a state file).
pub fn is_online(pgn: &Path) -> bool {
    dir().is_some_and(|d| pgn.parent() == Some(d.as_path())) && state_path(pgn).exists()
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

/// Appends games to the database file and keeps the state in step.
struct Writer {
    path: PathBuf,
    file: std::io::BufWriter<std::fs::File>,
    state: State,
    added: u64,
}

impl Writer {
    fn open(path: &Path, state: State) -> Result<Writer, Failure> {
        let disk = |e: std::io::Error| Failure::Disk(e.to_string());
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(disk)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(disk)?;
        Ok(Writer {
            path: path.to_owned(),
            file: std::io::BufWriter::new(file),
            state,
            added: 0,
        })
    }

    fn game(&mut self, pgn: &str) -> Result<(), Failure> {
        let pgn = pgn.trim();
        if pgn.is_empty() {
            return Ok(());
        }
        self.file
            .write_all(pgn.as_bytes())
            .and_then(|_| self.file.write_all(b"\n\n"))
            .map_err(|e| Failure::Disk(e.to_string()))?;
        self.added += 1;
        self.state.games += 1;
        Ok(())
    }

    /// Makes what was written durable, the state last (it never runs ahead of the file).
    fn commit(&mut self) -> Result<(), Failure> {
        self.file
            .flush()
            .map_err(|e| Failure::Disk(e.to_string()))?;
        self.state.save(&self.path)
    }

    fn done(mut self, cancelled: bool) -> Result<Done, Failure> {
        self.commit()?;
        Ok(Done {
            path: self.path,
            added: self.added,
            total: self.state.games,
            cancelled,
        })
    }
}

/// Fetches the games of `accounts` new since the last run into their database (one
/// database for all of them); `progress` counts the games added. Cancelling keeps what
/// was fetched.
pub fn download(
    accounts: &[Account],
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Done, Failure> {
    let path = combined_path(accounts).ok_or_else(|| Failure::Disk("no data directory".into()))?;
    download_to(accounts, &path, progress, cancel)
}

/// [`download`] into the database at `path`.
fn download_to(
    accounts: &[Account],
    path: &Path,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Done, Failure> {
    let path = path.to_owned();
    let mut state = State::load(&path).unwrap_or_else(|| {
        // A database without its state: start it anew rather than append duplicates.
        let _ = std::fs::remove_file(&path);
        State::default()
    });
    // Accounts not in the database yet start from their first game.
    for a in accounts {
        if !state
            .accounts
            .iter()
            .any(|s| s.platform == Some(a.platform) && s.username.eq_ignore_ascii_case(&a.username))
        {
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
        let wanted = accounts.iter().any(|a| {
            Some(a.platform) == w.state.accounts[i].platform
                && a.username
                    .eq_ignore_ascii_case(&w.state.accounts[i].username)
        });
        if !wanted {
            continue;
        }
        result = match w.state.accounts[i].platform {
            Some(Platform::Lichess) => lichess(&mut w, i, progress, cancel),
            Some(Platform::ChessCom) => chesscom(&mut w, i, progress, cancel),
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

/// The games of account `i` of the writer's database from Lichess. `Ok(true)` when
/// cancelled.
fn lichess(
    w: &mut Writer,
    i: usize,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<bool, Failure> {
    let mut url = format!(
        "https://lichess.org/api/games/user/{}?sort=dateAsc&finished=true&pgnInJson=true\
         &perfType=ultraBullet,bullet,blitz,rapid,classical,correspondence\
         &clocks=false&evals=false&opening=true",
        w.state.accounts[i].username
    );
    if let Some(last) = w.state.accounts[i].lichess_last {
        url.push_str(&format!("&since={}", last + 1));
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
        let (Some(pgn), Some(created)) = (v["pgn"].as_str(), v["createdAt"].as_i64()) else {
            return Err(Failure::Unexpected(
                "a game without pgn or createdAt".into(),
            ));
        };
        if v["variant"]
            .as_str()
            .is_none_or(|x| x == "standard" || x == "fromPosition")
        {
            w.game(pgn)?;
            progress.fetch_add(1, Ordering::Relaxed);
        }
        w.state.accounts[i].lichess_last = Some(created);
        if n % 100 == 99 {
            w.commit()?;
        }
    }
    Ok(false)
}

/// The games of account `i` of the writer's database from Chess.com. `Ok(true)` when
/// cancelled.
fn chesscom(
    w: &mut Writer,
    i: usize,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<bool, Failure> {
    let lower = w.state.accounts[i].username.to_ascii_lowercase();
    let months = archives(&lower)?;
    for url in months {
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
        if self.job.is_none()
            && !self.available.iter().all(|a| *a)
            && self.last.is_some_and(|t| t.elapsed() >= Self::RETRY)
        {
            self.spawn();
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
        };
        s.save(&pgn).unwrap();
        assert_eq!(State::load(&pgn), Some(s));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
