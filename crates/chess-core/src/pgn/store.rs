//! Writing records into a PGN file the user keeps (docs/design/own-databases.md): one
//! record appended to the end, a record replaced or removed, a removed record put
//! back (undo). Every write is explicit, and the other records stay byte for byte as
//! they were: an append only adds at the end; a replacement or a removal writes the
//! whole file anew into a temporary file beside it and renames it over the old one,
//! after `sync_all`. A file that changed on disk since it was read is not touched.
//!
//! Only files the program decodes as UTF-8 (or plain ASCII) are written: a record is
//! written as UTF-8, so a file in another encoding would end up mixed. The line endings
//! of a new record follow the file's (CRLF when its first line ends so).

use std::io::{self, Write as _};
use std::path::Path;

use crate::database::PgnDatabase;

/// Why a write did not happen.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The file on disk is not what was read: another program wrote it meanwhile.
    #[error("the file changed on disk since it was read")]
    Changed,
    /// The database was opened from memory, not from a file.
    #[error("the database has no file")]
    NoFile,
    /// The database's text is not UTF-8: appending UTF-8 would mix encodings.
    #[error("the file is not in UTF-8")]
    Encoding,
    #[error("no record {0}")]
    NoRecord(usize),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// What an append tells about the record it wrote: where it starts in the file (after
/// the file is read again, the record whose range starts here is the new one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appended {
    pub offset: u64,
}

/// A record removed from the file, enough to put it back where it was (undo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    pub index: usize,
    pub bytes: Vec<u8>,
}

/// `"\r\n"` when the first line of `bytes` ends so, else `"\n"`.
fn line_ending(bytes: &[u8]) -> &'static str {
    match bytes.iter().position(|b| *b == b'\n') {
        Some(i) if i > 0 && bytes[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// `record` with its line endings as the file has them, ending with one blank line.
fn record_bytes(record: &str, eol: &str) -> Vec<u8> {
    let text = record.trim_end();
    let mut out = String::with_capacity(text.len() + 4);
    for line in text.split('\n') {
        out.push_str(line.trim_end_matches('\r'));
        out.push_str(eol);
    }
    out.push_str(eol);
    out.into_bytes()
}

/// What to put between the end of `bytes` and an appended record, so that the record
/// starts after a blank line: nothing, one or two line endings.
fn separator(bytes: &[u8], eol: &str) -> &'static str {
    let ends_with = |suffix: &str| bytes.ends_with(suffix.as_bytes());
    if bytes.is_empty() || ends_with(&format!("{eol}{eol}")) {
        ""
    } else if ends_with(eol) {
        if eol == "\r\n" { "\r\n" } else { "\n" }
    } else if eol == "\r\n" {
        "\r\n\r\n"
    } else {
        "\n\n"
    }
}

fn check_writable(db: &PgnDatabase) -> Result<&Path, WriteError> {
    let path = db.path().ok_or(WriteError::NoFile)?;
    if db.encoding() != crate::encoding::TextEncoding::Utf8 && !db.detected_encoding().ascii_only {
        return Err(WriteError::Encoding);
    }
    if db.source_changed()? {
        return Err(WriteError::Changed);
    }
    Ok(path)
}

/// Appends `record` (PGN text of one game) to the file of `db`. The file is opened for
/// appending, so a record added by someone else meanwhile is not lost; the file's bytes
/// before the new record stay as they were.
pub fn append(db: &PgnDatabase, record: &str) -> Result<Appended, WriteError> {
    let path = check_writable(db)?;
    let (data, sep) = tail_for(db.bytes(), record);
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    let offset = file.metadata()?.len() + sep as u64;
    file.write_all(&data)?;
    file.sync_all()?;
    Ok(Appended { offset })
}

/// What goes at the end of `bytes` to add `record` after a blank line, and how many of
/// its bytes come before the record.
fn tail_for(bytes: &[u8], record: &str) -> (Vec<u8>, usize) {
    let eol = line_ending(bytes);
    let sep = separator(bytes, eol);
    let mut data = sep.as_bytes().to_vec();
    data.extend(record_bytes(record, eol));
    (data, sep.len())
}

/// The text of `db` with `record` added at the end, and where the record starts: the
/// same bytes [`append`] writes, for a database kept elsewhere than in a file (the web
/// version's games in the browser).
pub fn appended(db: &PgnDatabase, record: &str) -> (Vec<u8>, u64) {
    let bytes = db.bytes();
    let (tail, sep) = tail_for(bytes, record);
    let mut out = Vec::with_capacity(bytes.len() + tail.len());
    out.extend_from_slice(bytes);
    out.extend(tail);
    (out, (bytes.len() + sep) as u64)
}

/// The text of `db` with record `index` replaced by `record` (what [`replace`] writes).
pub fn replaced(db: &PgnDatabase, index: usize, record: &str) -> Result<Vec<u8>, WriteError> {
    let bytes = db.bytes();
    let range = db
        .entries()
        .get(index)
        .ok_or(WriteError::NoRecord(index))?
        .byte_range
        .clone();
    let eol = line_ending(bytes);
    let mut out = Vec::with_capacity(bytes.len() + record.len());
    out.extend_from_slice(&bytes[..range.start]);
    out.extend(record_bytes(record, eol));
    out.extend_from_slice(&bytes[range.end..]);
    Ok(out)
}

/// The text of `db` without record `index` (what [`remove`] writes) and the record.
pub fn removed(db: &PgnDatabase, index: usize) -> Result<(Vec<u8>, Removed), WriteError> {
    let bytes = db.bytes();
    let range = db
        .entries()
        .get(index)
        .ok_or(WriteError::NoRecord(index))?
        .byte_range
        .clone();
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..range.start]);
    out.extend_from_slice(&bytes[range.end..]);
    Ok((
        out,
        Removed {
            index,
            bytes: bytes[range].to_vec(),
        },
    ))
}

/// Replaces record `index` with `record`; every other byte of the file stays.
pub fn replace(db: &PgnDatabase, index: usize, record: &str) -> Result<(), WriteError> {
    let path = check_writable(db)?;
    write_atomic(path, &replaced(db, index, record)?)?;
    Ok(())
}

/// Removes record `index`; the bytes before and after it stay. What was removed comes
/// back with [`restore`].
pub fn remove(db: &PgnDatabase, index: usize) -> Result<Removed, WriteError> {
    let path = check_writable(db)?;
    let (out, gone) = removed(db, index)?;
    write_atomic(path, &out)?;
    Ok(gone)
}

/// Puts a removed record back at its place: the file is byte for byte what it was
/// before the removal, as long as nothing else was written in between. `db` is the
/// database as read after the removal.
pub fn restore(db: &PgnDatabase, removed: &Removed) -> Result<(), WriteError> {
    let path = check_writable(db)?;
    write_atomic(path, &restored(db, removed)?)?;
    Ok(())
}

/// The text of `db` with a removed record back at its place (what [`restore`] writes).
pub fn restored(db: &PgnDatabase, removed: &Removed) -> Result<Vec<u8>, WriteError> {
    let bytes = db.bytes();
    let at = match db.entries().get(removed.index) {
        Some(e) => e.byte_range.start,
        None if removed.index == db.len() => bytes.len(),
        None => return Err(WriteError::NoRecord(removed.index)),
    };
    let mut out = Vec::with_capacity(bytes.len() + removed.bytes.len());
    out.extend_from_slice(&bytes[..at]);
    out.extend_from_slice(&removed.bytes);
    out.extend_from_slice(&bytes[at..]);
    Ok(out)
}

/// Writes every record of `db` into `dest` as UTF-8 (LF, one blank line between
/// records): a copy of a file in another encoding, which can then be written to. `db`
/// itself is not touched. `dest` must not exist.
pub fn write_utf8_copy(db: &PgnDatabase, dest: &Path) -> io::Result<()> {
    if dest.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists", dest.display()),
        ));
    }
    let mut out = Vec::with_capacity(db.byte_len());
    // Letters the source writes as `~~`/`o^` (see repair.rs) are restored in the copy,
    // as they are on the screen: the copy is read as UTF-8, where nothing is restored.
    let restore = db.repair_letters_enabled();
    for i in 0..db.len() {
        if let Some(text) = db.raw_text(i) {
            let text = if restore {
                crate::repair::repair_letters(&text).0
            } else {
                text
            };
            out.extend(record_bytes(&text, "\n"));
        }
    }
    write_atomic(dest, &out)
}

/// The record of `db` that is the same game as `game`: the same players and date, and
/// the same moves of the main line. `None` when there is none.
pub fn find_same_game(db: &PgnDatabase, game: &crate::Game) -> Option<usize> {
    // Names compared without case; an unknown date in any spelling is one value.
    let name = |v: Option<&str>| v.unwrap_or("?").trim().to_lowercase();
    let date = |v: Option<&str>| match v.map(str::trim) {
        None | Some("" | "?") => "????.??.??".to_owned(),
        Some(d) => d.to_owned(),
    };
    let (white, black, when) = (
        name(game.tag("White")),
        name(game.tag("Black")),
        date(game.tag("Date")),
    );
    let sans: Vec<&str> = game
        .mainline()
        .iter()
        .filter_map(|id| game.game_move(*id).map(|m| m.san.as_str()))
        .collect();
    (0..db.len()).find(|&i| {
        let e = &db.entries()[i];
        if name(e.white()) != white || name(e.black()) != black || date(e.date()) != when {
            return false;
        }
        let Some(parsed) = db.load_game(i) else {
            return false;
        };
        // Two studies from different positions are different games.
        if parsed.game.tag("FEN") != game.tag("FEN") {
            return false;
        }
        let other: Vec<String> = parsed
            .game
            .mainline()
            .iter()
            .filter_map(|id| parsed.game.game_move(*id).map(|m| m.san.clone()))
            .collect();
        other.len() == sans.len() && other.iter().zip(&sans).all(|(a, b)| a == b)
    })
}

/// A date as the user types it — `2026.10.18`, `18.10.2026`, `2026-10-18`, `2026.10`,
/// `2026`, nothing — as PGN writes it (`2026.10.18`, `2026.10.??`, `????.??.??`).
/// `None` for text that is not a date.
pub fn pgn_date(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() || t.chars().all(|c| c == '?' || c == '.') {
        return Some("????.??.??".to_owned());
    }
    let parts: Vec<&str> = t.split(['.', '-', '/']).map(str::trim).collect();
    let num = |s: &str| s.parse::<u32>().ok();
    // A month or day may be unknown, as PGN writes it: "1910.??.??".
    let part = |s: &str| -> Option<Option<u32>> {
        if !s.is_empty() && s.chars().all(|c| c == '?') {
            Some(None)
        } else {
            num(s).map(Some)
        }
    };
    // A year is written with four digits: "12.10" is not a date.
    let (y, m, d) = match parts.as_slice() {
        [y] if y.len() == 4 => (num(y)?, None, None),
        [y, m] if y.len() == 4 => (num(y)?, part(m)?, None),
        [_] | [_, _] => return None,
        [a, b, c] => {
            if a.len() == 4 && a.chars().all(|c| c.is_ascii_digit()) {
                (num(a)?, part(b)?, part(c)?)
            } else if c.len() == 4 {
                (num(c)?, part(b)?, part(a)?)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if !(1..=9999).contains(&y)
        || m.is_some_and(|m| !(1..=12).contains(&m))
        || d.is_some_and(|d| !(1..=31).contains(&d))
    {
        return None;
    }
    Some(format!(
        "{y:04}.{}.{}",
        m.map_or("??".to_owned(), |m| format!("{m:02}")),
        d.map_or("??".to_owned(), |d| format!("{d:02}"))
    ))
}

/// Writes `bytes` to `path` through a temporary file in the same directory: the old
/// file stays whole until the new one is on disk (`sync_all`), then is replaced.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // A link to the file is a way to it, not the file: the target is rewritten.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let path = target.as_path();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        // The file keeps its permissions (a private 600 stays 600).
        if let Ok(meta) = std::fs::metadata(path) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        // The rename itself reaches the disk with the folder.
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
            && let Ok(d) = std::fs::File::open(dir)
        {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgn::parse_str;
    use std::sync::atomic::AtomicBool;

    fn open(path: &Path) -> PgnDatabase {
        PgnDatabase::open(path, &mut |_| {}, &AtomicBool::new(false)).expect("opens")
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cp-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const G1: &str = "[Event \"One\"]\n[White \"A\"]\n[Black \"B\"]\n\n1. e4 e5 *\n";
    const G2: &str =
        "[Event \"Two\"]\n[White \"C\"]\n[Black \"D\"]\n\n1. d4 d5 {a comment} 1/2-1/2\n";
    const G3: &str = "[Event \"Three\"]\n[White \"E\"]\n[Black \"F\"]\n\n1. c4 1-0\n";

    fn new_record() -> String {
        parse_str("[Event \"New\"]\n[White \"X\"]\n[Black \"Y\"]\n\n1. Nf3 d5 2. g3 *\n")
            .game
            .to_pgn()
    }

    fn records(db: &PgnDatabase) -> Vec<Vec<u8>> {
        (0..db.len())
            .map(|i| db.raw_bytes(i).unwrap().to_vec())
            .collect()
    }

    /// Records without their trailing whitespace: an append adds a blank line after
    /// the last one, which is still the same record.
    fn trimmed(records: &[Vec<u8>]) -> Vec<Vec<u8>> {
        records
            .iter()
            .map(|r| {
                r[..r.len()
                    - r.iter()
                        .rev()
                        .take_while(|b| b.is_ascii_whitespace())
                        .count()]
                    .to_vec()
            })
            .collect()
    }

    #[test]
    fn append_to_files_of_every_shape() {
        for (name, content) in [
            ("blank-line", format!("{G1}\n{G2}\n")),
            ("one-newline", format!("{G1}\n{G2}")),
            ("no-newline", format!("{G1}\n{}", G2.trim_end())),
            ("crlf", format!("{G1}\n{G2}").replace('\n', "\r\n")),
            ("empty", String::new()),
        ] {
            let d = dir(name);
            let path = d.join("base.pgn");
            std::fs::write(&path, &content).unwrap();
            let db = open(&path);
            let before = records(&db);
            let added = append(&db, &new_record()).unwrap();
            let after_bytes = std::fs::read(&path).unwrap();
            assert!(
                after_bytes.starts_with(content.as_bytes()),
                "{name}: old bytes kept"
            );
            let db2 = open(&path);
            assert_eq!(db2.len(), before.len() + 1, "{name}");
            assert_eq!(
                trimmed(&records(&db2)[..before.len()]),
                trimmed(&before),
                "{name}: old records same"
            );
            let new = db2.entries().last().unwrap();
            assert_eq!(new.byte_range.start as u64, added.offset, "{name}: offset");
            assert_eq!(new.white(), Some("X"));
            let parsed = db2.load_game(db2.len() - 1).unwrap();
            assert_eq!(
                parsed.game.canonical_tree(),
                parse_str(&new_record()).game.canonical_tree()
            );
            if name == "crlf" {
                let text = String::from_utf8(after_bytes).unwrap();
                assert!(!text.contains("[^\r]\n"), "no bare LF");
                assert!(text.ends_with("\r\n\r\n"));
            } else {
                assert!(after_bytes.ends_with(b"\n\n"));
            }
            // A second append starts after a blank line too.
            let db3 = open(&path);
            append(&db3, &new_record()).unwrap();
            assert_eq!(open(&path).len(), before.len() + 2, "{name}: twice");
        }
    }

    #[test]
    fn replace_keeps_the_other_records() {
        for k in 0..3 {
            let d = dir(&format!("replace{k}"));
            let path = d.join("base.pgn");
            std::fs::write(&path, format!("{G1}\n{G2}\n{G3}\n")).unwrap();
            let db = open(&path);
            let before = records(&db);
            replace(&db, k, &new_record()).unwrap();
            let db2 = open(&path);
            assert_eq!(db2.len(), 3);
            let after = records(&db2);
            for i in 0..3 {
                if i != k {
                    assert_eq!(
                        after[i], before[i],
                        "record {i} unchanged when {k} replaced"
                    );
                }
            }
            assert_eq!(db2.entries()[k].white(), Some("X"));
        }
        // The only record.
        let d = dir("replace-only");
        let path = d.join("base.pgn");
        std::fs::write(&path, G1).unwrap();
        replace(&open(&path), 0, &new_record()).unwrap();
        let db = open(&path);
        assert_eq!(db.len(), 1);
        assert_eq!(db.entries()[0].white(), Some("X"));
    }

    #[test]
    fn remove_and_restore_are_exact() {
        for k in 0..3 {
            let d = dir(&format!("remove{k}"));
            let path = d.join("base.pgn");
            let content = format!("{G1}\n{G2}\n{G3}\n");
            std::fs::write(&path, &content).unwrap();
            let db = open(&path);
            let before = records(&db);
            let removed = remove(&db, k).unwrap();
            let db2 = open(&path);
            assert_eq!(db2.len(), 2);
            let after = records(&db2);
            let expected: Vec<_> = before
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != k)
                .map(|(_, r)| r.clone())
                .collect();
            assert_eq!(after, expected, "others kept when {k} removed");
            restore(&db2, &removed).unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                content.as_bytes(),
                "restored byte for byte"
            );
        }
    }

    #[test]
    fn a_changed_file_is_not_written() {
        let d = dir("changed");
        let path = d.join("base.pgn");
        std::fs::write(&path, format!("{G1}\n{G2}\n")).unwrap();
        let db = open(&path);
        // Another program appends; the modification time moves on.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(G3.as_bytes())
            .unwrap();
        assert!(db.source_changed().unwrap());
        assert!(matches!(
            replace(&db, 0, &new_record()),
            Err(WriteError::Changed)
        ));
        assert!(matches!(remove(&db, 0), Err(WriteError::Changed)));
        assert!(matches!(
            append(&db, &new_record()),
            Err(WriteError::Changed)
        ));
    }

    /// A crash in the middle of an append leaves a broken tail; the next append goes
    /// after it, never into it, and the records before it stay whole.
    #[test]
    fn a_broken_tail_is_appended_after() {
        let d = dir("broken");
        let path = d.join("base.pgn");
        let half = &G2[..G2.len() - 8];
        let content = format!("{G1}\n{half}");
        std::fs::write(&path, &content).unwrap();
        let db = open(&path);
        assert_eq!(db.len(), 2);
        let added = append(&db, &new_record()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(content.as_bytes()));
        assert_eq!(
            &bytes[added.offset as usize..],
            &record_bytes(&new_record(), "\n")[..]
        );
        let db2 = open(&path);
        assert_eq!(db2.len(), 3);
        assert_eq!(db2.entries()[0].white(), Some("A"));
        assert_eq!(db2.entries()[2].white(), Some("X"));
    }

    #[test]
    fn other_encodings_are_refused_and_copied() {
        let d = dir("cp1251");
        let path = d.join("base.pgn");
        let (bytes, _, _) = encoding_rs::WINDOWS_1251
            .encode("[Event \"Турнир\"]\n[White \"Иванов\"]\n[Black \"Петров\"]\n\n1. e4 e5 *\n");
        std::fs::write(&path, &bytes).unwrap();
        let db = open(&path);
        assert_eq!(db.encoding(), crate::encoding::TextEncoding::Windows1251);
        assert!(matches!(
            append(&db, &new_record()),
            Err(WriteError::Encoding)
        ));
        let copy = d.join("base (UTF-8).pgn");
        write_utf8_copy(&db, &copy).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes.as_ref(),
            "original untouched"
        );
        let db2 = open(&copy);
        assert_eq!(db2.encoding(), crate::encoding::TextEncoding::Utf8);
        assert_eq!(db2.len(), 1);
        assert_eq!(db2.entries()[0].white(), Some("Иванов"));
        assert_eq!(db2.raw_text(0), db.raw_text(0));
        assert!(
            matches!(write_utf8_copy(&db, &copy), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
        );
    }

    #[test]
    fn an_empty_file_is_a_database() {
        let d = dir("empty");
        let path = d.join("new.pgn");
        std::fs::write(&path, "").unwrap();
        let db = open(&path);
        assert_eq!(db.len(), 0);
        append(&db, &new_record()).unwrap();
        assert_eq!(open(&path).len(), 1);
    }

    #[test]
    fn dates_as_typed() {
        assert_eq!(pgn_date("").as_deref(), Some("????.??.??"));
        assert_eq!(pgn_date("?").as_deref(), Some("????.??.??"));
        assert_eq!(pgn_date("2026.10.18").as_deref(), Some("2026.10.18"));
        assert_eq!(pgn_date("18.10.2026").as_deref(), Some("2026.10.18"));
        assert_eq!(pgn_date("2026-10-18").as_deref(), Some("2026.10.18"));
        assert_eq!(pgn_date("2026.10").as_deref(), Some("2026.10.??"));
        assert_eq!(pgn_date(" 2026 ").as_deref(), Some("2026.??.??"));
        assert_eq!(pgn_date("1.1.2026").as_deref(), Some("2026.01.01"));
        assert_eq!(pgn_date("tomorrow"), None);
        assert_eq!(pgn_date("2026.13.01"), None);
        assert_eq!(pgn_date("12.10"), None);
        // PGN's own unknown parts.
        assert_eq!(pgn_date("1910.??.??").as_deref(), Some("1910.??.??"));
        assert_eq!(pgn_date("1910.05.??").as_deref(), Some("1910.05.??"));
        assert_eq!(pgn_date("1910.??.05").as_deref(), Some("1910.??.05"));
    }

    #[test]
    fn the_same_game_is_found() {
        let d = dir("same");
        let path = d.join("base.pgn");
        std::fs::write(&path, format!("{G1}\n{G2}\n")).unwrap();
        let db = open(&path);
        let same = parse_str("[White \"C\"]\n[Black \"D\"]\n[Date \"?\"]\n\n1. d4 d5 *\n").game;
        assert_eq!(find_same_game(&db, &same), Some(1));
        let other_moves = parse_str("[White \"C\"]\n[Black \"D\"]\n\n1. d4 Nf6 *\n").game;
        assert_eq!(find_same_game(&db, &other_moves), None);
        let other_player = parse_str("[White \"C\"]\n[Black \"Z\"]\n\n1. d4 d5 *\n").game;
        assert_eq!(find_same_game(&db, &other_player), None);
    }

    #[test]
    fn tag_values_with_quotes_survive() {
        let mut g = parse_str("[Event \"?\"]\n\n*\n").game;
        g.set_tag("White", "Smith \"Jr\" \\ O'Neil");
        g.set_tag("Event", "Cup");
        let text = g.to_pgn();
        let again = parse_str(&text).game;
        assert_eq!(again.tag("White"), Some("Smith \"Jr\" \\ O'Neil"));
    }
}
