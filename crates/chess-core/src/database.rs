//! A read-only PGN source: original bytes, encoding, index and lazy parsing.
//!
//! The source file is only read, never written. Saving is a separate explicit
//! operation of the application (write a new file from `Game::to_pgn`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use crate::encoding::{EncodingGuess, TextEncoding, decode_text, detect_encoding};
use crate::pgn::{
    Cancelled, GameEntry, ParseOptions, ParsedGame, ScanProgress, parse_game_range_with, scan_games,
};
use crate::repair::{LetterRepairDetection, detect_letter_substitution};

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("import cancelled")]
    Cancelled,
}

impl From<Cancelled> for OpenError {
    fn from(_: Cancelled) -> Self {
        OpenError::Cancelled
    }
}

/// Identity of the source content, for change detection and cache keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceFingerprint {
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// FNV-1a 64 of the content (stable across runs and platforms).
    pub content_hash: u64,
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// An opened PGN file (or in-memory text). `Send + Sync`; open it on a
/// background thread and move it to the UI.
#[derive(Clone, Debug)]
pub struct PgnDatabase {
    path: Option<PathBuf>,
    bytes: Vec<u8>,
    detected: EncodingGuess,
    encoding: TextEncoding,
    entries: Vec<GameEntry>,
    fingerprint: SourceFingerprint,
    letter_repair: LetterRepairDetection,
    repair_letters: bool,
}

impl PgnDatabase {
    /// Reads and indexes a file. Progress/cancellation as in [`scan_games`].
    pub fn open(
        path: &Path,
        progress: &mut dyn FnMut(ScanProgress),
        cancel: &AtomicBool,
    ) -> Result<Self, OpenError> {
        let io_err = |source| OpenError::Io {
            path: path.to_owned(),
            source,
        };
        let bytes = std::fs::read(path).map_err(io_err)?;
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut db = Self::from_bytes(bytes, None, progress, cancel)?;
        db.path = Some(path.to_owned());
        db.fingerprint.modified = modified;
        Ok(db)
    }

    /// Indexes in-memory content. `encoding: None` = auto-detect.
    pub fn from_bytes(
        bytes: Vec<u8>,
        encoding: Option<TextEncoding>,
        progress: &mut dyn FnMut(ScanProgress),
        cancel: &AtomicBool,
    ) -> Result<Self, OpenError> {
        let detected = detect_encoding(&bytes);
        let encoding = encoding.unwrap_or(detected.encoding);
        let entries = scan_games(&bytes, encoding, progress, cancel)?;
        let letter_repair = detect_letter_substitution(&bytes, encoding);
        let fingerprint = SourceFingerprint {
            len: bytes.len() as u64,
            modified: None,
            content_hash: fnv1a(&bytes),
        };
        Ok(PgnDatabase {
            path: None,
            bytes,
            detected,
            encoding,
            entries,
            fingerprint,
            letter_repair,
            repair_letters: letter_repair.detected,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn entries(&self) -> &[GameEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Encoding used for decoding text.
    pub fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    /// What auto-detection found.
    pub fn detected_encoding(&self) -> &EncodingGuess {
        &self.detected
    }

    /// Re-decodes the index with a user-chosen encoding (source untouched).
    pub fn set_encoding(
        &mut self,
        encoding: TextEncoding,
        progress: &mut dyn FnMut(ScanProgress),
        cancel: &AtomicBool,
    ) -> Result<(), Cancelled> {
        let entries = scan_games(&self.bytes, encoding, progress, cancel)?;
        self.entries = entries;
        self.encoding = encoding;
        self.letter_repair = detect_letter_substitution(&self.bytes, encoding);
        self.repair_letters = self.letter_repair.detected;
        Ok(())
    }

    /// Whether the source replaces «ч/Ч/ю» by `~~`/`><`/`o^` (see [`crate::repair`]).
    pub fn letter_repair(&self) -> &LetterRepairDetection {
        &self.letter_repair
    }

    /// Repair applied by [`Self::load_game`]; defaults to the detection result.
    pub fn repair_letters_enabled(&self) -> bool {
        self.repair_letters
    }

    /// Turns the letter repair on or off (affects subsequent `load_game`).
    pub fn set_repair_letters(&mut self, enabled: bool) {
        self.repair_letters = enabled;
    }

    pub fn fingerprint(&self) -> &SourceFingerprint {
        &self.fingerprint
    }

    /// Whether the file on disk differs (size or mtime) from what was indexed.
    pub fn source_changed(&self) -> io::Result<bool> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        let meta = std::fs::metadata(path)?;
        Ok(meta.len() != self.fingerprint.len || meta.modified().ok() != self.fingerprint.modified)
    }

    /// Fully parses record `index`.
    pub fn load_game(&self, index: usize) -> Option<ParsedGame> {
        let e = self.entries.get(index)?;
        Some(parse_game_range_with(
            &self.bytes,
            e.byte_range.clone(),
            e.first_line,
            self.encoding,
            &ParseOptions {
                repair_letters: self.repair_letters,
            },
        ))
    }

    /// Original bytes of record `index` (for "copy original" and diagnostics).
    pub fn raw_bytes(&self, index: usize) -> Option<&[u8]> {
        let e = self.entries.get(index)?;
        self.bytes.get(e.byte_range.clone())
    }

    /// Original record decoded to Unicode (line endings normalized to LF).
    pub fn raw_text(&self, index: usize) -> Option<String> {
        self.raw_bytes(index)
            .map(|b| decode_text(self.encoding, b).0.trim_end().to_owned() + "\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn database_is_send_sync() {
        assert_send_sync::<PgnDatabase>();
        assert_send_sync::<crate::Game>();
        assert_send_sync::<ParsedGame>();
    }

    #[test]
    fn background_scan_with_progress_and_cancel() {
        use std::sync::Arc;
        let text: String = (0..3000)
            .map(|i| format!("[Event \"{i}\"]\n\n1. e4 e5 *\n\n"))
            .collect();
        let bytes = text.into_bytes();

        let full = {
            let bytes = bytes.clone();
            std::thread::spawn(move || {
                let mut reports = Vec::new();
                let db = PgnDatabase::from_bytes(
                    bytes,
                    None,
                    &mut |p| reports.push(p),
                    &AtomicBool::new(false),
                );
                (db.map(|d| d.len()).ok(), reports)
            })
            .join()
            .expect("thread")
        };
        assert_eq!(full.0, Some(3000));
        assert!(full.1.len() >= 2);
        assert!(
            full.1
                .windows(2)
                .all(|w| w[0].bytes_done <= w[1].bytes_done)
        );
        assert_eq!(full.1.last().map(|p| p.games), Some(3000));

        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = Arc::clone(&cancel);
        let r = std::thread::spawn(move || {
            PgnDatabase::from_bytes(
                bytes,
                None,
                &mut |_| c2.store(true, std::sync::atomic::Ordering::Relaxed),
                &cancel,
            )
            .map(|d| d.len())
        })
        .join()
        .expect("thread");
        assert!(matches!(r, Err(OpenError::Cancelled)));
    }

    #[test]
    fn letter_repair_is_detected_and_switchable() {
        let record = "[White \"><ерные\"]\n\n{><ерные играo^т, ~~то делать ~~ерным?} 1. e4 *\n\n";
        let text = record.repeat(8);
        let bytes = encoding_rs::WINDOWS_1251.encode(&text).0.into_owned();
        let mut db =
            PgnDatabase::from_bytes(bytes.clone(), None, &mut |_| {}, &AtomicBool::new(false))
                .expect("db");
        assert_eq!(db.encoding(), TextEncoding::Windows1251);
        assert!(db.letter_repair().detected);
        assert!(db.repair_letters_enabled());
        let g = db.load_game(0).expect("game");
        assert_eq!(
            g.game.node(g.game.root()).expect("root").comments_after(),
            &["Черные играют, что делать черным?".to_owned()]
        );
        assert!(
            g.diagnostics
                .iter()
                .any(|d| d.code == crate::DiagnosticCode::LettersRestored)
        );
        // Tags and raw text are not touched.
        assert_eq!(g.game.tag("White"), Some("><ерные"));
        assert!(db.raw_text(0).expect("raw").contains("{><ерные играo^т"));
        assert_eq!(
            db.raw_bytes(0),
            bytes.get(db.entries()[0].byte_range.clone())
        );

        db.set_repair_letters(false);
        let g = db.load_game(0).expect("game");
        assert_eq!(
            g.game.node(g.game.root()).expect("root").comments_after(),
            &["><ерные играo^т, ~~то делать ~~ерным?".to_owned()]
        );

        // Same text in UTF-8: never repaired.
        let db = PgnDatabase::from_bytes(
            text.into_bytes(),
            None,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .expect("db");
        assert!(!db.letter_repair().detected);
        let g = db.load_game(0).expect("game");
        assert!(g.game.node(g.game.root()).expect("root").comments_after()[0].contains("~~"));
    }

    #[test]
    fn in_memory_database() {
        let text = "[White \"Hübner\"]\n\n1. e4 *\n\n[White \"B\"]\n\n1. d4 *\n";
        let bytes: Vec<u8> = text
            .chars()
            .map(|c| if c == 'ü' { 0xFC } else { c as u8 })
            .collect();
        let db =
            PgnDatabase::from_bytes(bytes, None, &mut |_| {}, &AtomicBool::new(false)).expect("db");
        assert_eq!(db.encoding(), TextEncoding::Windows1252);
        assert_eq!(db.len(), 2);
        assert_eq!(db.entries()[0].white(), Some("Hübner"));
        let g = db.load_game(1).expect("game");
        assert_eq!(g.game.mainline().len(), 1);
        assert!(db.raw_text(0).expect("raw").contains("1. e4"));
        assert!(db.load_game(5).is_none());
    }
}
