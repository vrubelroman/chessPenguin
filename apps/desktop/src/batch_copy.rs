//! Several records of a database copied into a new database (task 42,
//! docs/design/tasks-42-43-2026-11-27.md): the records chosen in the table, a set of
//! record numbers that keeps a bit per record, and the copy itself, written in the
//! background into a temporary file beside the new one and published only when whole.
//! The source database is only read.

use std::collections::HashSet;
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
    /// The same game already copied (players, date, moves of the main line).
    pub duplicates: usize,
    /// Record numbers (from 0) that could not be read.
    pub unreadable: Vec<usize>,
}

impl CopyReport {
    pub fn skipped(&self) -> usize {
        self.duplicates + self.unreadable.len()
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

fn partial_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.part-{}", std::process::id()))
}

/// Copies the records of `req` from `db` into the new file `req.target`, in the table's
/// order (records of equal value by number). Each record has the bytes «Add to
/// database» would append to it; a game already copied is skipped, a record that cannot
/// be read too. Written into a temporary file beside the target and published by a hard
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
        let mut seen = HashSet::new();
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
                Some((parsed, text)) => {
                    if seen.insert(store::same_game(&parsed.game)) {
                        out.write_all(&store::record_for_new_file(&text))?;
                        report.copied += 1;
                    } else {
                        report.duplicates += 1;
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

/// «Name: copied 3, skipped 1 — duplicates 1, unreadable 0 (No. …)».
pub fn summary(name: &str, report: &CopyReport) -> String {
    if report.skipped() == 0 {
        return trf(
            "{name}: copied {n}, skipped {m}",
            &[("name", &name), ("n", &report.copied), ("m", &0)],
        );
    }
    let mut numbers: Vec<String> = report
        .unreadable
        .iter()
        .take(5)
        .map(|i| (i + 1).to_string())
        .collect();
    if report.unreadable.len() > 5 {
        numbers.push(trf(
            "and {n} more",
            &[("n", &(report.unreadable.len() - 5))],
        ));
    }
    let mut text = trf(
        "{name}: copied {n}, skipped {m} — duplicates {d}, unreadable {u}",
        &[
            ("name", &name),
            ("n", &report.copied),
            ("m", &report.skipped()),
            ("d", &report.duplicates),
            ("u", &report.unreadable.len()),
        ],
    );
    if !numbers.is_empty() {
        text.push_str(&format!(
            " ({})",
            trf("No. {list}", &[("list", &numbers.join(", "))])
        ));
    }
    text
}

#[cfg(test)]
#[path = "batch_copy_tests.rs"]
mod tests;
