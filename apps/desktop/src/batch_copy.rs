//! Several records of a database copied into a new database (task 42,
//! docs/design/tasks-42-43-2026-11-27.md): the records chosen in the table, a set of
//! record numbers that keeps a bit per record, and the copy itself, written in the
//! background into a temporary file beside the new one and published only when whole.
//! The source database is only read.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chess_core::pgn::store;
use chess_import::Database;
use chess_import::chessbase::base::SortKey;

use crate::i18n::{tr, trf};

/// Records chosen in a table: one bit per record number. Membership is one lookup, and
/// choosing all the records of a list of millions is one pass over it.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    words: Vec<u64>,
    count: usize,
}

impl Selection {
    pub fn contains(&self, index: usize) -> bool {
        self.words
            .get(index / 64)
            .is_some_and(|w| w & (1 << (index % 64)) != 0)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn insert(&mut self, index: usize) {
        let word = index / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        let bit = 1 << (index % 64);
        if self.words[word] & bit == 0 {
            self.words[word] |= bit;
            self.count += 1;
        }
    }

    pub fn remove(&mut self, index: usize) {
        if let Some(w) = self.words.get_mut(index / 64) {
            let bit = 1 << (index % 64);
            if *w & bit != 0 {
                *w &= !bit;
                self.count -= 1;
            }
        }
    }

    pub fn toggle(&mut self, index: usize) {
        if self.contains(index) {
            self.remove(index);
        } else {
            self.insert(index);
        }
    }

    pub fn clear(&mut self) {
        self.words.clear();
        self.count = 0;
    }

    /// Only `index` chosen.
    pub fn only(&mut self, index: usize) {
        self.clear();
        self.insert(index);
    }

    /// Adds every record of `records` (a list of the table).
    pub fn extend(&mut self, records: &[u32]) {
        if let Some(&most) = records.iter().max() {
            let words = most as usize / 64 + 1;
            if words > self.words.len() {
                self.words.resize(words, 0);
            }
        }
        for &r in records {
            let (word, bit) = (r as usize / 64, 1u64 << (r % 64));
            if self.words[word] & bit == 0 {
                self.words[word] |= bit;
                self.count += 1;
            }
        }
    }

    /// The chosen records, in the order of their numbers.
    pub fn records(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.count);
        for (n, &w) in self.words.iter().enumerate() {
            let mut w = w;
            while w != 0 {
                let bit = w.trailing_zeros();
                out.push((n * 64) as u32 + bit);
                w &= w - 1;
            }
        }
        out
    }

    /// The highest chosen record.
    pub fn last(&self) -> Option<usize> {
        let (n, w) = self
            .words
            .iter()
            .enumerate()
            .rev()
            .find(|(_, w)| **w != 0)?;
        Some(n * 64 + 63 - w.leading_zeros() as usize)
    }

    /// Records from `len` on let go (a database with fewer records now).
    pub fn retain_below(&mut self, len: usize) {
        while let Some(last) = self.last().filter(|&l| l >= len) {
            self.remove(last);
        }
    }

    /// How many of `records` are chosen.
    pub fn count_in(&self, records: &[u32]) -> usize {
        if self.is_empty() {
            return 0;
        }
        records
            .iter()
            .filter(|&&r| self.contains(r as usize))
            .count()
    }
}

/// The PGN text of record `index` as «Add to database» copies it: the record's own
/// text when the file is PGN read without restoring letters, otherwise the game written
/// anew (a ChessBase database has no PGN; restored letters are in the copy).
pub fn record_text(db: &Database, index: usize) -> Option<String> {
    match db.as_pgn() {
        Some(pgn) if !pgn.repair_letters_enabled() => db.raw_text(index),
        _ => db.load_game(index).map(|p| p.game.to_pgn()),
    }
}

/// What to copy: the chosen records (by number), the table's order and the new file.
#[derive(Clone, Debug)]
pub struct CopyRequest {
    pub records: Vec<u32>,
    pub sort: SortKey,
    pub ascending: bool,
    pub target: PathBuf,
    /// A pause per record (`CP_BATCH_SLOW`): the progress can be seen and photographed.
    pub slow: bool,
}

/// How a copy went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub copied: usize,
    /// Record numbers (from 0) skipped as a copy of a record already copied: the same
    /// PGN text, white space aside.
    pub duplicates: Vec<usize>,
    /// Record numbers (from 0) that could not be read.
    pub unreadable: Vec<usize>,
}

impl CopyReport {
    pub fn skipped(&self) -> usize {
        self.duplicates.len() + self.unreadable.len()
    }
}

/// Why nothing was written.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    /// A file of that name appeared meanwhile: it is not touched.
    #[error("a file with this name is already there")]
    Exists,
    /// The source file changed on disk: the record numbers no longer mean the same games.
    #[error("the source changed")]
    SourceChanged,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl CopyError {
    /// The reason in the user's words.
    pub fn reason(&self) -> String {
        match self {
            CopyError::Exists => tr("a file with this name is already there").to_owned(),
            CopyError::SourceChanged => tr("the source changed").to_owned(),
            CopyError::Io(e) => match e.kind() {
                std::io::ErrorKind::StorageFull => tr("no space left on the disk").to_owned(),
                std::io::ErrorKind::PermissionDenied => tr("no permission to write").to_owned(),
                _ => e.to_string(),
            },
        }
    }
}

/// The temporary file of a copy; removed unless published.
struct Partial {
    path: PathBuf,
    published: bool,
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The temporary file of a copy into `target`: `.<name>.part-<pid>-<random>`, the
/// process that writes it in its name (`clean_orphans` knows from it whether its writer
/// is gone).
fn partial_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    target.hash(&mut h);
    target.with_file_name(format!(
        ".{name}{PART}{}-{:016x}",
        std::process::id(),
        h.finish()
    ))
}

const PART: &str = ".part-";

/// Temporary files of copies whose process is gone (a crash, a power cut), in `dirs`:
/// removed. Only names this program gives (`.<name>.pgn.part-<pid>-<hex>`) and only when
/// no process `<pid>` runs; anything else is left alone.
pub fn clean_orphans(dirs: &[PathBuf]) {
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(pid) = orphan_pid(&name)
                && pid != std::process::id()
                && !Path::new(&format!("/proc/{pid}")).exists()
                && entry.file_type().is_ok_and(|t| t.is_file())
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// The writer's process of a temporary copy file named so, if the name is one.
fn orphan_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix('.')?;
    let (stem, tail) = rest.rsplit_once(PART)?;
    if !stem.to_lowercase().ends_with(".pgn") {
        return None;
    }
    let (pid, hex) = tail.split_once('-')?;
    (hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| pid.parse().ok())
        .flatten()
}

/// A record's PGN text with every run of white space as one space: what makes two
/// records the same for a copy (the owner's rule: tags, moves, variations, comments and
/// NAG all equal; line breaks and spaces aside).
fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 128 bits of a normalized record text: millions of copied records are told apart
/// without keeping their texts; equal keys are compared again in full.
fn key_of(normalized: &str) -> u128 {
    let half = |salt: u64| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        salt.hash(&mut h);
        normalized.hash(&mut h);
        h.finish()
    };
    (u128::from(half(0x9e37_79b9_7f4a_7c15)) << 64) | u128::from(half(0x6a09_e667_f3bc_c909))
}

/// Copies the records of `req` from `db` into the new file `req.target`, in the table's
/// order (records of equal value by number). Each record has the bytes «Add to
/// database» would append to it; a record whose text was already copied is skipped, a
/// record that cannot be read too. Written into a temporary file beside the target and published by a hard
/// link, which fails rather than replace a file of that name. `None`: cancelled, nothing
/// is left on disk.
pub fn copy_into_new(
    db: &Database,
    req: &CopyRequest,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<Result<CopyReport, CopyError>> {
    let mut order = req.records.clone();
    order.sort_unstable();
    db.sort_records(&mut order, req.sort, req.ascending);
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let partial = Partial {
        path: partial_path(&req.target),
        published: false,
    };
    let run = || -> Result<Option<CopyReport>, CopyError> {
        if let Some(dir) = req.target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if req.target.exists() {
            return Err(CopyError::Exists);
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial.path)?;
        let mut out = std::io::BufWriter::new(file);
        let mut report = CopyReport::default();
        // Copied records by the key of their text; equal keys of different texts (all
        // but impossible) in `more`.
        let mut seen: HashMap<u128, u32> = HashMap::new();
        let mut more: Vec<u32> = Vec::new();
        for (n, &record) in order.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if req.slow {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let index = record as usize;
            let text = db
                .load_game(index)
                .and_then(|parsed| Some((parsed, record_text(db, index)?)));
            match text {
                None => report.unreadable.push(index),
                Some((_, text)) => {
                    let flat = normalized(&text);
                    let key = key_of(&flat);
                    let same = |other: u32| {
                        record_text(db, other as usize).is_some_and(|t| normalized(&t) == flat)
                    };
                    let duplicate = match seen.get(&key) {
                        None => {
                            seen.insert(key, record);
                            false
                        }
                        Some(&first) => {
                            let dup = same(first) || more.iter().any(|&o| same(o));
                            if !dup {
                                more.push(record);
                            }
                            dup
                        }
                    };
                    if duplicate {
                        report.duplicates.push(index);
                    } else {
                        out.write_all(&store::record_for_new_file(&text))?;
                        report.copied += 1;
                    }
                }
            }
            progress.store(n + 1, Ordering::Relaxed);
        }
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        if let Some(pgn) = db.as_pgn()
            && pgn.source_changed().unwrap_or(true)
        {
            return Err(CopyError::SourceChanged);
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match std::fs::hard_link(&partial.path, &req.target) {
            Ok(()) => Ok(Some(report)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(CopyError::Exists),
            Err(e) => Err(e.into()),
        }
    };
    match run() {
        Ok(None) => None,
        Ok(Some(report)) => Some(Ok(report)),
        Err(e) => Some(Err(e)),
    }
}

/// « (No. 3, 4 and 2 more)» for up to `most` record numbers, nothing for none.
fn numbers(records: &[usize], most: usize) -> String {
    if records.is_empty() {
        return String::new();
    }
    let mut list: Vec<String> = records
        .iter()
        .take(most)
        .map(|i| (i + 1).to_string())
        .collect();
    if records.len() > most {
        list.push(trf("and {n} more", &[("n", &(records.len() - most))]));
    }
    format!(" ({})", trf("No. {list}", &[("list", &list.join(", "))]))
}

/// «Name: copied 3, skipped 1 — duplicates 1 (No. 9), unreadable 0».
pub fn summary(name: &str, report: &CopyReport) -> String {
    if report.skipped() == 0 {
        return trf(
            "{name}: copied {n}, skipped {m}",
            &[("name", &name), ("n", &report.copied), ("m", &0)],
        );
    }
    trf(
        "{name}: copied {n}, skipped {m} — duplicates {d}{dl}, unreadable {u}{ul}",
        &[
            ("name", &name),
            ("n", &report.copied),
            ("m", &report.skipped()),
            ("d", &report.duplicates.len()),
            ("dl", &numbers(&report.duplicates, 10)),
            ("u", &report.unreadable.len()),
            ("ul", &numbers(&report.unreadable, 5)),
        ],
    )
}

#[cfg(test)]
#[path = "batch_copy_tests.rs"]
mod tests;
