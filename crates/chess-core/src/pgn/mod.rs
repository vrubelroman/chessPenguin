//! PGN import (scan + lazy parse) and export.

mod lexer;
pub mod parse;
pub mod scan;
pub mod write;

pub use parse::{
    ParseOptions, ParsedGame, parse_game, parse_game_range, parse_game_range_with, parse_str,
};
pub use scan::{Cancelled, EntryFlags, GameEntry, ScanProgress, scan_games};
pub use write::{WriteOptions, write_game};

use crate::encoding::TextEncoding;
use std::sync::atomic::AtomicBool;

/// Scans and parses every record (convenience for tests and small inputs).
pub fn parse_all(bytes: &[u8], encoding: TextEncoding) -> Vec<(GameEntry, ParsedGame)> {
    let entries =
        scan_games(bytes, encoding, &mut |_| {}, &AtomicBool::new(false)).unwrap_or_default();
    entries
        .into_iter()
        .map(|e| {
            let parsed = parse_game_range(bytes, e.byte_range.clone(), e.first_line, encoding);
            (e, parsed)
        })
        .collect()
}
