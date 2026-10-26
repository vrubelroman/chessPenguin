//! The player's games (docs/design/player-games.md, decisions «Партии игрока
//! записываются сами»): every game against the computer goes by itself into a database
//! of its player — «Roman — games against the computer», made by the program in the
//! folder of new databases at its first game. Only new records are appended; replacing
//! and deleting stay explicit. While a game is played it is kept in a journal after
//! every move, so that a crash loses nothing: the next start writes it (result `*`).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::i18n::{tr, trf};

/// A player of this computer: the name as entered, the rating, the database.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    pub name: String,
    pub rating: Option<u32>,
    /// Their database, once a game was written (renaming it in the program keeps the
    /// link).
    pub path: Option<PathBuf>,
}

/// The name as it is kept: trimmed, inner spaces single.
pub fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Names that are the same player: «Иван» and «иван ».
fn same(a: &str, b: &str) -> bool {
    tidy(a).to_lowercase() == tidy(b).to_lowercase()
}

/// The player of this name, if they played before.
pub fn player<'a>(players: &'a [Player], name: &str) -> Option<&'a Player> {
    players.iter().find(|p| same(&p.name, name))
}

/// Remembers the player who starts a game (first in the list) and their rating.
pub fn remember(players: &mut Vec<Player>, name: &str, rating: Option<u32>) {
    let mut p = match players.iter().position(|p| same(&p.name, name)) {
        Some(i) => players.remove(i),
        None => Player::default(),
    };
    p.name = tidy(name);
    p.rating = rating;
    players.insert(0, p);
}

/// Links the player to the database their game was written to.
pub fn link(players: &mut Vec<Player>, name: &str, path: &Path) {
    match players.iter_mut().find(|p| same(&p.name, name)) {
        Some(p) => p.path = Some(path.to_owned()),
        None => players.push(Player {
            name: tidy(name),
            rating: None,
            path: Some(path.to_owned()),
        }),
    }
}

/// The database's name: «Roman — games against the computer»; without a name, one for
/// everybody. The name is not declined (in Russian it could not be done safely).
pub fn db_name(name: &str) -> String {
    let name = tidy(name);
    if name.is_empty() {
        tr("Games against the computer").to_owned()
    } else {
        trf("{name} — games against the computer", &[("name", &name)])
    }
}

/// The longest player's name a file name takes, in characters.
const NAME_CHARS: usize = 40;

/// The longest file name, in bytes, with room for « 2.pgn» (file systems take 255).
const STEM_BYTES: usize = 114;

/// The file name for a database name: what a file name cannot have becomes «_», the
/// player's part is cut to [`NAME_CHARS`] (and to [`STEM_BYTES`] for the whole), no dot,
/// space or hyphen left at the cut, none of the first two at the end (Windows).
fn file_stem(player: &str, db: &str) -> String {
    let player = tidy(player);
    let mut chars: Vec<char> = player.chars().take(NAME_CHARS).collect();
    let rest = db.len() - player.len();
    while !chars.is_empty() && rest + chars.iter().map(|c| c.len_utf8()).sum::<usize>() > STEM_BYTES
    {
        chars.pop();
    }
    let mut cut: String = chars.into_iter().collect();
    if cut.len() < player.len() {
        cut = cut.trim_end_matches(['.', ' ', '-']).to_owned();
    }
    let db = if cut.len() < player.len() {
        db.replacen(&player, &cut, 1)
    } else {
        db.to_owned()
    };
    let clean: String = db
        .chars()
        .map(|c| {
            if crate::own_db::FORBIDDEN.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    clean.trim_end_matches(['.', ' ']).to_owned()
}

/// Where the player's games go: their database, else a new one in `dir`. A file of
/// that name made otherwise is taken when it is a PGN in UTF-8 (appending keeps its
/// bytes: a database brought from another computer goes on), else «… 2».
pub fn target(players: &[Player], name: &str, dir: &Path) -> PathBuf {
    if let Some(path) = player(players, name).and_then(|p| p.path.clone()) {
        return path;
    }
    let stem = file_stem(name, &db_name(name));
    let taken =
        |p: &Path| p.exists() && !std::fs::read(p).is_ok_and(|b| std::str::from_utf8(&b).is_ok());
    let mut path = dir.join(format!("{stem}.pgn"));
    let mut n = 2;
    while taken(&path) {
        path = dir.join(format!("{stem} {n}.pgn"));
        n += 1;
    }
    path
}

/// A game written: its record and whether the database was made for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub path: PathBuf,
    pub index: usize,
    pub created: bool,
}

/// Appends `pgn` to the database at `path`, making it (and its folder) when absent.
pub fn write(path: &Path, pgn: &str) -> Result<Written, String> {
    let created = !path.exists();
    if created {
        path.parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|()| File::create_new(path))
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    let db = chess_core::PgnDatabase::open(path, &mut |_| {}, &AtomicBool::new(false))
        .map_err(|e| e.to_string())?;
    let index = db.len();
    chess_core::pgn::store::append(&db, pgn).map_err(|e| e.to_string())?;
    Ok(Written {
        path: path.to_owned(),
        index,
        created,
    })
}

/// What the journal keeps of a game being played.
#[derive(Serialize, Deserialize)]
struct Entry {
    /// The player's database (`None`: games are not written, the journal is only for
    /// going on with it).
    target: Option<PathBuf>,
    /// Whose game it is: the player is linked to the database when it is written.
    #[serde(default)]
    player: String,
    pgn: String,
    /// The database's size before the game was appended: set just before the write, so
    /// that a crash right after it does not write the game twice.
    before: Option<u64>,
    /// What the game is played with (the opponent, the clock…): it can go on from here.
    /// None in journals of older versions: their games are only written.
    #[serde(default)]
    game: Option<crate::play::Saved>,
    /// Put aside on purpose (not left by a crash).
    #[serde(default)]
    aside: bool,
    /// The side to move's time left when it was put aside, in milliseconds (its last
    /// `[%clk]` does not count the time it thought since).
    #[serde(default)]
    to_move_left_ms: Option<u64>,
    /// When the game was last played (seconds since 1970): the newest of several put
    /// aside is the one offered. Older journals: the file's time.
    #[serde(default)]
    played_at: Option<u64>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The folder of the journals.
fn journal_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "chesspenguin").map(|d| d.data_local_dir().join("play"))
}

/// The journal of the game being played: a file rewritten after each move, and a lock
/// held while it is played (a journal whose lock is free is a game put aside, or one a
/// crash left).
pub struct Journal {
    lock: Option<(File, PathBuf)>,
    path: PathBuf,
    target: Option<PathBuf>,
    player: String,
    game: Option<crate::play::Saved>,
}

impl Journal {
    /// A journal for a game going to `target` (none when the folder cannot be made: the
    /// game is still written at its end).
    pub fn new(target: Option<PathBuf>, player: &str, game: crate::play::Saved) -> Self {
        Self::in_dir(journal_dir(), target, player, Some(game))
    }

    fn in_dir(
        dir: Option<PathBuf>,
        target: Option<PathBuf>,
        player: &str,
        game: Option<crate::play::Saved>,
    ) -> Self {
        let made = dir.and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let stem = format!("game-{}-{stamp}", std::process::id());
            let lock_path = dir.join(format!("{stem}.lock"));
            let lock = File::create(&lock_path).ok()?;
            lock.try_lock().ok()?;
            Some(((lock, lock_path), dir.join(format!("{stem}.json"))))
        });
        let (lock, path) = match made {
            Some((lock, path)) => (Some(lock), path),
            None => (None, PathBuf::new()),
        };
        Self {
            lock,
            path,
            target,
            player: tidy(player),
            game,
        }
    }

    /// Where the game goes when it ends (`None`: it is not written).
    pub fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    fn put(&self, pgn: &str, before: Option<u64>) {
        if self.lock.is_none() {
            return;
        }
        let entry = Entry {
            target: self.target.clone(),
            player: self.player.clone(),
            pgn: pgn.to_owned(),
            before,
            game: self.game.clone(),
            aside: false,
            to_move_left_ms: None,
            played_at: Some(now_secs()),
        };
        if let Ok(json) = serde_json::to_vec(&entry) {
            let _ = crate::settings::write_atomic(&self.path, &json);
        }
    }

    /// The game as it is now (written as it would be if the program stopped here).
    pub fn save(&self, pgn: &str) {
        self.put(pgn, None);
    }

    /// The game is over: written into its database. The journal stays until it is
    /// dropped: after a failed write a crash still leaves the game to the next start.
    /// `None`: the game is not written (no database for it).
    pub fn finish(&self, pgn: &str) -> Option<Result<Written, String>> {
        let target = self.target.as_ref()?;
        let before = std::fs::metadata(target).map_or(0, |m| m.len());
        self.put(pgn, Some(before));
        Some(write(target, pgn))
    }

    /// Whether the game can be put aside: its journal is kept on disk.
    pub fn can_put_aside(&self) -> bool {
        self.lock.is_some() && self.game.is_some()
    }

    /// The game is put aside as `pgn`: its journal stays (still ours; after the program
    /// stops, the next start finds it as a crash leaves one). `None`: there was no
    /// journal (its folder could not be made) or it is not a game that can go on.
    /// `to_move_left`: the side to move's time left now (with a clock).
    pub fn put_aside(
        mut self,
        pgn: &str,
        to_move_left: Option<std::time::Duration>,
    ) -> Option<PutAside> {
        let game = self.game.take()?;
        let (lock, lock_path) = self.lock.take()?;
        let entry = Entry {
            target: self.target.take(),
            player: std::mem::take(&mut self.player),
            pgn: pgn.to_owned(),
            before: None,
            game: Some(game),
            aside: true,
            to_move_left_ms: to_move_left.map(|d| d.as_millis() as u64),
            played_at: Some(now_secs()),
        };
        if let Ok(json) = serde_json::to_vec(&entry) {
            let _ = crate::settings::write_atomic(&self.path, &json);
        }
        Some(PutAside {
            lock,
            lock_path,
            path: std::mem::take(&mut self.path),
            entry,
            when: std::time::SystemTime::now(),
        })
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        if let Some((lock, lock_path)) = self.lock.take() {
            let _ = std::fs::remove_file(&self.path);
            drop(lock);
            let _ = std::fs::remove_file(lock_path);
        }
    }
}

/// A game a crash left: whose, where to, and what came of writing it.
pub struct Recovered {
    pub player: String,
    pub target: PathBuf,
    pub written: Result<Written, String>,
    /// A game put aside on purpose (not one a crash left).
    pub put_aside: bool,
}

/// A game put aside (or left by a crash) that can go on: its journal, locked by us now.
pub struct PutAside {
    lock: File,
    lock_path: PathBuf,
    path: PathBuf,
    entry: Entry,
    /// When it was last written (the newest is the one offered).
    when: std::time::SystemTime,
}

impl PutAside {
    pub fn pgn(&self) -> &str {
        &self.entry.pgn
    }

    pub fn game(&self) -> &crate::play::Saved {
        self.entry
            .game
            .as_ref()
            .expect("a game put aside has its settings")
    }

    /// Left by a crash, not put aside on purpose.
    pub fn crashed(&self) -> bool {
        !self.entry.aside
    }

    /// The side to move's time left when it was put aside.
    pub fn to_move_left(&self) -> Option<std::time::Duration> {
        self.entry
            .to_move_left_ms
            .map(std::time::Duration::from_millis)
    }

    /// Where it goes when it ends (`None`: not written).
    pub fn target(&self) -> Option<&Path> {
        self.entry.target.as_deref()
    }

    /// When it was put aside (its journal last written).
    pub fn when(&self) -> std::time::SystemTime {
        self.when
    }

    /// Goes on with the game: the same journal, kept on.
    pub fn resume(self) -> Journal {
        Journal {
            lock: Some((self.lock, self.lock_path)),
            path: self.path,
            target: self.entry.target,
            player: self.entry.player,
            game: self.entry.game,
        }
    }

    /// The crash that left it was said: from now on it is a game put aside.
    pub fn crash_said(&mut self) {
        self.entry.aside = true;
        self.store();
    }

    /// Lets the game go: written into its player's database unfinished (it is a game
    /// that was played), then the journal goes. `Ok(None)`: games are not written: it
    /// is only deleted. `Err`: it could not be written: it stays put aside, as it was.
    pub fn discard(mut self) -> Result<Option<Recovered>, Box<(Recovered, PutAside)>> {
        let Some(target) = self.entry.target.clone() else {
            self.remove();
            return Ok(None);
        };
        // The database's size first: a crash right after the write does not write it
        // again (the next start sees the file grown).
        self.entry.before = Some(std::fs::metadata(&target).map_or(0, |m| m.len()));
        self.store();
        let r = Recovered {
            written: write(&target, &self.entry.pgn),
            player: self.entry.player.clone(),
            target,
            put_aside: self.entry.aside,
        };
        if r.written.is_ok() {
            self.remove();
            Ok(Some(r))
        } else {
            self.entry.before = None;
            self.store();
            Err(Box::new((r, self)))
        }
    }

    fn store(&self) {
        if let Ok(json) = serde_json::to_vec(&self.entry) {
            let _ = crate::settings::write_atomic(&self.path, &json);
        }
    }

    /// Its journal goes.
    fn remove(self) {
        let _ = std::fs::remove_file(&self.path);
        drop(self.lock);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// The journals no program plays now (their lock is free): the newest game that can go
/// on is put aside; finished games a crash kept from their database, games of older
/// versions and other games left are written into their databases now.
pub fn recover() -> (Vec<Recovered>, Option<PutAside>) {
    journal_dir().map(|d| recover_in(&d)).unwrap_or_default()
}

fn recover_in(dir: &Path) -> (Vec<Recovered>, Option<PutAside>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return (Vec::new(), None);
    };
    let mut written = Vec::new();
    let mut aside: Vec<PutAside> = Vec::new();
    for lock_path in read.flatten().map(|e| e.path()) {
        if lock_path.extension().is_none_or(|e| e != "lock") {
            continue;
        }
        let Ok(lock) = File::open(&lock_path) else {
            continue;
        };
        if lock.try_lock().is_err() {
            continue; // a program running now plays it
        }
        let path = lock_path.with_extension("json");
        let read = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Entry>(&b).ok());
        let file_time = || {
            std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH)
        };
        let when = match read.as_ref().and_then(|e| e.played_at) {
            Some(t) => std::time::UNIX_EPOCH + std::time::Duration::from_secs(t),
            None => file_time(),
        };
        match read {
            // Over, or of an older version: it goes into its database (once).
            Some(entry) if entry.before.is_some() || entry.game.is_none() => {
                if let Some(target) = entry.target {
                    // The write had been made (the file grew since): not again.
                    let done = entry
                        .before
                        .is_some_and(|b| std::fs::metadata(&target).is_ok_and(|m| m.len() > b));
                    if !done {
                        let r = Recovered {
                            written: write(&target, &entry.pgn),
                            player: entry.player,
                            target,
                            put_aside: false,
                        };
                        let failed = r.written.is_err();
                        written.push(r);
                        // Not written: its journal stays for the next start.
                        if failed {
                            continue;
                        }
                    }
                }
            }
            Some(entry) => {
                aside.push(PutAside {
                    lock,
                    lock_path,
                    path,
                    entry,
                    when,
                });
                continue;
            }
            // A journal that cannot be read is kept aside, said in the log.
            None if path.exists() => {
                eprintln!("play journal unreadable, kept as .bad: {}", path.display());
                let _ = std::fs::rename(&path, path.with_extension("json.bad"));
            }
            None => {}
        }
        let _ = std::fs::remove_file(&path);
        drop(lock);
        let _ = std::fs::remove_file(&lock_path);
    }
    // One game put aside at a time: the newest; any other is written unfinished.
    aside.sort_by_key(|a| std::cmp::Reverse(a.when));
    let mut rest = aside.into_iter();
    let kept = rest.next();
    for a in rest {
        match a.discard() {
            Ok(r) => written.extend(r),
            // Not written: it stays for the next start (its lock let go).
            Err(b) => written.push(b.0),
        }
    }
    (written, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_are_the_same_whatever_the_case_and_spaces() {
        let mut ps = Vec::new();
        remember(&mut ps, "  Иван  Петров ", Some(1500));
        remember(&mut ps, "Роман", None);
        remember(&mut ps, "иван петров", Some(1600));
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].name, "иван петров");
        assert_eq!(ps[0].rating, Some(1600));
        link(&mut ps, "Роман", Path::new("/x/r.pgn"));
        assert_eq!(
            player(&ps, " роман").and_then(|p| p.path.clone()),
            Some(PathBuf::from("/x/r.pgn"))
        );
    }

    #[test]
    fn file_names_have_no_forbidden_characters() {
        let stem = file_stem("A/B: C?", "A/B: C? — games");
        assert_eq!(stem, "A_B_ C_ — games");
        let long = "Я".repeat(60);
        let stem = file_stem(&long, &format!("{long} — games"));
        assert_eq!(stem, format!("{} — games", "Я".repeat(NAME_CHARS)));
        assert_eq!(file_stem("Dot.", "Dot."), "Dot");
        // Cut at a hyphen: none left at the cut; long letters: within the bytes.
        let name = format!("{}-Сидорова", "Ж".repeat(NAME_CHARS - 1));
        let stem = file_stem(&name, &format!("{name} — партии с компьютером"));
        assert!(!stem.contains("- —"), "{stem}");
        assert!(stem.len() <= STEM_BYTES, "{}", stem.len());
    }

    #[test]
    fn a_new_database_takes_the_game_and_the_next_is_appended() {
        let dir = std::env::temp_dir().join(format!("cp-player-games-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = target(&[], "Тест", &dir.join("sub"));
        let game = "[Event \"?\"]\n[White \"Тест\"]\n[Black \"Maia 1500\"]\n[Result \"1-0\"]\n\n1. e4 e5 1-0\n";
        let w = write(&path, game).unwrap();
        assert!(w.created);
        assert_eq!(w.index, 0);
        let w = write(&path, game).unwrap();
        assert!(!w.created);
        assert_eq!(w.index, 1);
        // A file of the same name in another encoding is not written into.
        let bad = target(&[], "Bad", &dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&bad, [0xff, 0xfe, 0x00]).unwrap();
        let other = target(&[], "Bad", &dir);
        assert_ne!(other, bad);
        assert!(other.to_string_lossy().ends_with(" 2.pgn"), "{other:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recovery until only the journals of games being played are left. Another test
    /// starting a process holds a copy of a lock for a moment (the child gets the parent's
    /// files until it runs its program); recovery then leaves that journal for later.
    fn recover_until(dir: &Path, live: usize) -> Vec<Recovered> {
        let recover_in = |d: &Path| recover_in(d).0;
        let locks = || {
            std::fs::read_dir(dir)
                .map(|r| {
                    r.flatten()
                        .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
                        .count()
                })
                .unwrap_or(0)
        };
        let mut out = Vec::new();
        for _ in 0..200 {
            out.extend(recover_in(dir));
            if locks() <= live {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        out
    }

    #[test]
    fn a_game_put_aside_goes_on_or_is_written_unfinished() {
        let dir = std::env::temp_dir().join(format!("cp-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let journals = dir.join("play");
        let db = dir.join("db.pgn");
        let game = "[Event \"?\"]\n[Result \"*\"]\n\n1. e4 e5 *\n";
        let saved = crate::play::Saved {
            white: true,
            strength: 1500,
            rival: crate::play::Rival::Maia,
            style: String::new(),
            user_elo: None,
            user_name: "Роман".into(),
            clock: Some(crate::play::TimeControl {
                base: 300,
                increment: 3,
            }),
            floor: 0,
            from_game: None,
            from_after: None,
            from_after_shown: None,
        };
        let j = Journal::in_dir(
            Some(journals.clone()),
            Some(db.clone()),
            "Роман",
            Some(saved.clone()),
        );
        let aside = j.put_aside(game, None).unwrap();
        assert_eq!(aside.game(), &saved);
        assert!(!aside.crashed());
        // The program stops: the next start finds it put aside, not written.
        drop(aside);
        let (written, found) = recover_in(&journals);
        assert!(written.is_empty());
        let found = found.unwrap();
        assert_eq!(found.pgn(), game);
        // It goes on with the same journal; put aside again and let go: written.
        let j = found.resume();
        let aside = j
            .put_aside(game, Some(std::time::Duration::from_secs(61)))
            .unwrap();
        assert_eq!(
            aside.to_move_left(),
            Some(std::time::Duration::from_secs(61))
        );
        let r = aside.discard().ok().flatten().unwrap();
        assert_eq!(r.written.map(|w| w.index), Ok(0));
        assert_eq!(std::fs::read_dir(&journals).unwrap().count(), 0);
        // A database that cannot be written (a folder of its name): the game is not
        // lost, it stays put aside, at the next start too.
        let bad = dir.join("bad.pgn");
        std::fs::create_dir_all(&bad).unwrap();
        let j = Journal::in_dir(
            Some(journals.clone()),
            Some(bad.clone()),
            "Роман",
            Some(saved.clone()),
        );
        let aside = j.put_aside(game, None).unwrap();
        let Err(b) = aside.discard() else {
            panic!("written into a folder")
        };
        let (r, aside) = *b;
        assert!(r.written.is_err());
        assert_eq!(aside.pgn(), game);
        drop(aside);
        let (written, found) = recover_in(&journals);
        assert!(written.is_empty());
        assert_eq!(found.map(|a| a.pgn().to_owned()), Some(game.to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_game_left_by_a_crash_is_written_once() {
        let dir = std::env::temp_dir().join(format!("cp-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let journals = dir.join("play");
        let db = dir.join("db.pgn");
        let game = "[Event \"?\"]\n[Result \"*\"]\n\n1. d4 d5 *\n";
        // Played, then the program stopped: the lock goes, the files stay.
        let j = Journal::in_dir(Some(journals.clone()), Some(db.clone()), "Роман", None);
        j.save(game);
        let mut j = j;
        drop(j.lock.take());
        drop(j);
        // A game being played now (its lock held) is left alone.
        let live = Journal::in_dir(Some(journals.clone()), Some(dir.join("live.pgn")), "", None);
        live.save(game);
        let got = recover_until(&journals, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].player, "Роман");
        assert_eq!(got[0].written.as_ref().map(|w| w.index), Ok(0));
        assert!(recover_in(&journals).0.is_empty());
        // Crashed right after the write: not written again.
        let j = Journal::in_dir(Some(journals.clone()), Some(db.clone()), "Роман", None);
        let before = std::fs::metadata(&db).unwrap().len();
        j.put(game, Some(before));
        write(&db, game).unwrap();
        let mut j = j;
        drop(j.lock.take());
        drop(j);
        assert!(recover_until(&journals, 1).is_empty());
        let n = chess_core::PgnDatabase::open(&db, &mut |_| {}, &AtomicBool::new(false))
            .unwrap()
            .len();
        assert_eq!(n, 2);
        drop(live);
        assert_eq!(std::fs::read_dir(&journals).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
