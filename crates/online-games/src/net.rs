//! Talking to Lichess and Chess.com: lookups, probes, downloads into the database.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::*;

/// How many last bytes of the file the state remembers.
const TAIL: usize = 64;

impl State {
    fn save(&self, pgn: &Path) -> Result<(), Failure> {
        let text = serde_json::to_string_pretty(self).map_err(|e| Failure::Disk(e.to_string()))?;
        let path = state_path(pgn);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| Failure::Disk(e.to_string()))
    }
}

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

/// Makes a request again when the connection broke (Lichess drops connections for a
/// minute or so after an export was cut short), waiting `waits` seconds in between;
/// stops waiting when cancelled.
fn retrying<T>(
    waits: &[u64],
    cancel: &AtomicBool,
    mut request: impl FnMut() -> Result<T, ureq::Error>,
) -> Result<T, Failure> {
    let mut waits = waits.iter();
    loop {
        match request() {
            Ok(r) => return Ok(r),
            Err(e) => {
                let Some(&secs) = waits.next() else {
                    return Err(Failure::Network(e.to_string()));
                };
                for _ in 0..secs * 10 {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(Failure::Network(e.to_string()));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

/// Waits before the attempts of a download request: about a minute in all.
const DOWNLOAD_WAITS: &[u64] = &[3, 10, 20, 30];

/// GET returning (status, body); the body of an error status is not read.
fn get(url: &str) -> Result<(u16, String), Failure> {
    let never = AtomicBool::new(false);
    // The whole exchange is repeated: a connection may also break in the body.
    retrying(&[2, 5], &never, || {
        let mut resp = agent(false)
            .get(url)
            .header("Accept", "application/json")
            .call()?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Ok((status, String::new()));
        }
        let body = resp
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_string()?;
        Ok((status, body))
    })
    .and_then(|(status, body)| {
        if status == 429 {
            Err(Failure::RateLimited)
        } else {
            Ok((status, body))
        }
    })
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
    // Another ChessPenguin running may download into the same database: a file lock
    // (released when the file closes, also if the process dies).
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| Failure::Disk(e.to_string()))?;
    }
    let lock = std::fs::File::create(path.with_extension("online.lock"))
        .map_err(|e| Failure::Disk(e.to_string()))?;
    if let Err(e) = lock.try_lock() {
        return Err(match e {
            std::fs::TryLockError::WouldBlock => Failure::Busy,
            std::fs::TryLockError::Error(e) => Failure::Disk(e.to_string()),
        });
    }
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
    /// Present for finished games (an unfinished one may come without it).
    pgn: Option<&'a str>,
}

fn lichess_game(v: &Value) -> Option<LichessGame<'_>> {
    let g = LichessGame {
        id: v["id"].as_str()?,
        created: v["createdAt"].as_i64()?,
        playing: matches!(v["status"].as_str(), Some("created" | "started")),
        standard: v["variant"]
            .as_str()
            .is_none_or(|x| x == "standard" || x == "fromPosition"),
        pgn: v["pgn"].as_str(),
    };
    (g.playing || g.pgn.is_some()).then_some(g)
}

/// Whether to write a game of the Lichess export; an unfinished one is remembered as
/// pending instead (asked about by id next time), a pending one that ended is written.
fn lichess_keep(a: &mut AccountState, g: &LichessGame) -> bool {
    let pending = a.lichess_pending.iter().position(|(id, _)| id == g.id);
    if g.playing {
        if pending.is_none() && g.standard {
            a.lichess_pending.push((g.id.to_owned(), g.created));
        }
        return false;
    }
    if let Some(k) = pending {
        a.lichess_pending.remove(k);
    }
    g.standard
}

/// A pending game Lichess no longer returns is given up after a year.
const PENDING_DAYS: i64 = 365;

/// Reads a Lichess NDJSON answer line by line. `Ok(true)` when cancelled.
fn lichess_lines(
    resp: ureq::http::Response<ureq::Body>,
    cancel: &AtomicBool,
    mut each: impl FnMut(usize, &Value) -> Result<(), Failure>,
) -> Result<bool, Failure> {
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
        each(n, &v)?;
    }
    Ok(false)
}

const LICHESS_PARAMS: &str = "pgnInJson=true&clocks=false&evals=false&opening=true";

/// The games of account `i` of the writer's database from Lichess: first the pending
/// (unfinished last time) ones by id, then the new ones. `Ok(true)` when cancelled.
fn lichess(
    w: &mut Writer,
    i: usize,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
    status: &Progress,
) -> Result<bool, Failure> {
    let write = |w: &mut Writer, v: &Value| -> Result<i64, Failure> {
        let g = lichess_game(v)
            .ok_or_else(|| Failure::Unexpected("a game without id, pgn or createdAt".into()))?;
        if lichess_keep(&mut w.state.accounts[i], &g)
            && let Some(pgn) = g.pgn
        {
            let pgn = pgn.to_owned();
            w.game(&pgn)?;
            progress.fetch_add(1, Ordering::Relaxed);
            status.games.fetch_add(1, Ordering::Relaxed);
        }
        Ok(g.created)
    };
    let pending: Vec<String> = w.state.accounts[i]
        .lichess_pending
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    for chunk in pending.chunks(300) {
        let resp = retrying(DOWNLOAD_WAITS, cancel, || {
            agent(true)
                .post(&format!(
                    "https://lichess.org/api/games/export/_ids?{LICHESS_PARAMS}"
                ))
                .header("Accept", "application/x-ndjson")
                .send(chunk.join(","))
        })?;
        if lichess_lines(resp, cancel, |_, v| write(w, v).map(|_| ()))? {
            return Ok(true);
        }
    }
    let year_ago = now_secs() * 1000 - PENDING_DAYS * 86_400_000;
    w.state.accounts[i]
        .lichess_pending
        .retain(|(_, created)| *created > year_ago);
    w.commit()?;

    let a = &w.state.accounts[i];
    let mut url = format!(
        "https://lichess.org/api/games/user/{}?sort=dateAsc&finished=false\
         &perfType=ultraBullet,bullet,blitz,rapid,classical,correspondence&{LICHESS_PARAMS}",
        a.username
    );
    if let Some(last) = a.lichess_last {
        url.push_str(&format!("&since={}", last + 1));
    }
    let resp = retrying(DOWNLOAD_WAITS, cancel, || {
        agent(true)
            .get(&url)
            .header("Accept", "application/x-ndjson")
            .call()
    })?;
    lichess_lines(resp, cancel, |n, v| {
        let created = write(w, v)?;
        let a = &mut w.state.accounts[i];
        a.lichess_last = Some(a.lichess_last.map_or(created, |l| l.max(created)));
        if n % 100 == 99 {
            w.commit()?;
        }
        Ok(())
    })
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
            combined_path(Path::new("/online"), a)
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
        let g = |id: &'static str, playing: bool| LichessGame {
            id,
            created: 10,
            playing,
            standard: true,
            pgn: (!playing).then_some("1. e4 *"),
        };
        // First run: a correspondence game is still on, another one is over.
        assert!(!lichess_keep(&mut a, &g("corr", true)));
        assert!(lichess_keep(&mut a, &g("blitz", false)));
        assert_eq!(a.lichess_pending, vec![("corr".to_owned(), 10)]);
        // Asked about by id: still on, then over.
        assert!(!lichess_keep(&mut a, &g("corr", true)));
        assert_eq!(a.lichess_pending.len(), 1);
        assert!(lichess_keep(&mut a, &g("corr", false)));
        assert!(a.lichess_pending.is_empty());
        // A variant game is neither kept nor waited for.
        let mut v = g("960", true);
        v.standard = false;
        assert!(!lichess_keep(&mut a, &v));
        assert!(a.lichess_pending.is_empty());
        // An unfinished game may come without its PGN, a finished one may not.
        let json = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        let on = json(r#"{"id":"a","createdAt":1,"status":"started"}"#);
        assert!(lichess_game(&on).is_some_and(|g| g.playing));
        let over = json(r#"{"id":"a","createdAt":1,"status":"mate"}"#);
        assert!(lichess_game(&over).is_none());
    }

    /// Against the real sites, with an account given by `CP_ONLINE_NICK` (kept out of
    /// the repository): lookup, an interrupted Lichess download continued without gaps
    /// or duplicates, a full Chess.com download, and an update that adds nothing. Run
    /// with `CP_ONLINE_NICK=<nick> cargo test -p online-games online_live -- --ignored --nocapture`.
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
            let over = AtomicBool::new(false);
            let done = std::thread::scope(|s| {
                s.spawn(|| {
                    while progress.load(Ordering::Relaxed) < 150 && !over.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    cancel.store(true, Ordering::Relaxed);
                });
                let r = download_to(std::slice::from_ref(&li), &path, &progress, &cancel);
                over.store(true, Ordering::Relaxed);
                r.expect("downloads")
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
            let over = AtomicBool::new(false);
            let goal = if round == 1 {
                done.total as usize + 100
            } else {
                100
            };
            let d = std::thread::scope(|s| {
                s.spawn(|| {
                    while progress.load(Ordering::Relaxed) < goal && !over.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    cancel.store(true, Ordering::Relaxed);
                });
                let r = download_to(&accounts, &path, &progress, &cancel);
                over.store(true, Ordering::Relaxed);
                r.expect("downloads")
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
