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
    target: PathBuf,
    /// Whose game it is: the player is linked to the database when it is written.
    #[serde(default)]
    player: String,
    pgn: String,
    /// The database's size before the game was appended: set just before the write, so
    /// that a crash right after it does not write the game twice.
    before: Option<u64>,
}

/// The folder of the journals.
fn journal_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "chesspenguin").map(|d| d.data_local_dir().join("play"))
}

/// The journal of the game being played: a file rewritten after each move, and a lock
/// held while the program runs (a journal whose lock is free is one a crash left).
pub struct Journal {
    lock: Option<(File, PathBuf)>,
    path: PathBuf,
    target: PathBuf,
    player: String,
}

impl Journal {
    /// A journal for a game going to `target` (none when the folder cannot be made: the
    /// game is still written at its end).
    pub fn new(target: PathBuf, player: &str) -> Self {
        Self::in_dir(journal_dir(), target, player)
    }

    fn in_dir(dir: Option<PathBuf>, target: PathBuf, player: &str) -> Self {
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
        }
    }

    pub fn target(&self) -> &Path {
        &self.target
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
    pub fn finish(&self, pgn: &str) -> Result<Written, String> {
        let before = std::fs::metadata(&self.target).map_or(0, |m| m.len());
        self.put(pgn, Some(before));
        write(&self.target, pgn)
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
}

/// Games a crash left in journals (their program is gone: the lock is free) are written
/// into their databases now.
pub fn recover() -> Vec<Recovered> {
    journal_dir().map(|d| recover_in(&d)).unwrap_or_default()
}

fn recover_in(dir: &Path) -> Vec<Recovered> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
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
        if let Some(entry) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Entry>(&b).ok())
        {
            // The write had been made (the file grew since): not again.
            let done = entry
                .before
                .is_some_and(|b| std::fs::metadata(&entry.target).is_ok_and(|m| m.len() > b));
            if !done {
                out.push(Recovered {
                    written: write(&entry.target, &entry.pgn),
                    player: entry.player,
                    target: entry.target,
                });
            }
        }
        let _ = std::fs::remove_file(&path);
        drop(lock);
        let _ = std::fs::remove_file(&lock_path);
    }
    out
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

    #[test]
    fn a_game_left_by_a_crash_is_written_once() {
        let dir = std::env::temp_dir().join(format!("cp-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let journals = dir.join("play");
        let db = dir.join("db.pgn");
        let game = "[Event \"?\"]\n[Result \"*\"]\n\n1. d4 d5 *\n";
        // Played, then the program stopped: the lock goes, the files stay.
        let j = Journal::in_dir(Some(journals.clone()), db.clone(), "Роман");
        j.save(game);
        let mut j = j;
        drop(j.lock.take());
        drop(j);
        // A game being played now (its lock held) is left alone.
        let live = Journal::in_dir(Some(journals.clone()), dir.join("live.pgn"), "");
        live.save(game);
        let got = recover_in(&journals);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].player, "Роман");
        assert_eq!(got[0].written.as_ref().map(|w| w.index), Ok(0));
        assert!(recover_in(&journals).is_empty());
        // Crashed right after the write: not written again.
        let j = Journal::in_dir(Some(journals.clone()), db.clone(), "Роман");
        let before = std::fs::metadata(&db).unwrap().len();
        j.put(game, Some(before));
        write(&db, game).unwrap();
        let mut j = j;
        drop(j.lock.take());
        drop(j);
        assert!(recover_in(&journals).is_empty());
        let n = chess_core::PgnDatabase::open(&db, &mut |_| {}, &AtomicBool::new(false))
            .unwrap()
            .len();
        assert_eq!(n, 2);
        drop(live);
        assert_eq!(std::fs::read_dir(&journals).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
