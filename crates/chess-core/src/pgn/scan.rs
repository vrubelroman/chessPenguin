//! Fast index scan of a multi-game PGN file (no move parsing).

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::encoding::TextEncoding;
use crate::pgn::lexer::{Lexer, TokKind, parse_tag};

/// Summary flags computed while scanning.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryFlags {
    pub has_fen: bool,
    pub has_comments: bool,
    /// A comment with words in it, not only commands such as `[%clk 0:10:07]` or
    /// `[%eval 0.3]` (clocks of online games are no comments to look for).
    pub has_text_comments: bool,
    pub has_variations: bool,
    pub has_nags: bool,
    /// A `--` token outside comments (teaching null move).
    pub has_null_moves: bool,
    /// Structural problems found by the scanner (unbalanced parentheses,
    /// stray `}`, unterminated comment, malformed tag). Full parse gives details.
    pub has_structure_issues: bool,
    /// Text could not be decoded cleanly (UTF-8 replacement characters).
    pub has_decode_errors: bool,
    /// No movetext tokens at all (tags only).
    pub empty_movetext: bool,
}

/// One record of a PGN file.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameEntry {
    /// 0-based ordinal in the file.
    pub index: usize,
    /// Byte range in the original file (includes trailing whitespace).
    pub byte_range: Range<usize>,
    /// 1-based line where the record starts.
    pub first_line: u32,
    /// All tags in source order (decoded).
    pub tags: Vec<(String, String)>,
    pub flags: EntryFlags,
    /// Half-moves of the main line (outside variations), as written.
    pub main_plies: u32,
    /// Half-moves of the whole tree, variations included.
    pub all_plies: u32,
}

impl GameEntry {
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn white(&self) -> Option<&str> {
        self.tag("White")
    }
    pub fn black(&self) -> Option<&str> {
        self.tag("Black")
    }
    pub fn white_elo(&self) -> Option<u32> {
        self.tag("WhiteElo").and_then(|v| v.trim().parse().ok())
    }
    pub fn black_elo(&self) -> Option<u32> {
        self.tag("BlackElo").and_then(|v| v.trim().parse().ok())
    }
    pub fn result(&self) -> Option<&str> {
        self.tag("Result")
    }
    pub fn date(&self) -> Option<&str> {
        self.tag("Date")
    }
    pub fn event(&self) -> Option<&str> {
        self.tag("Event")
    }
    pub fn eco(&self) -> Option<&str> {
        self.tag("ECO")
    }
    /// Moves of the main line: the `PlyCount` tag when the file has it, else counted.
    pub fn moves(&self) -> u32 {
        self.ply_count().unwrap_or(self.main_plies).div_ceil(2)
    }

    /// When the game was played, as text that sorts by time: `UTCDate` and `UTCTime`
    /// when both are there (Lichess, Chess.com), else `Date`.
    pub fn played_at(&self) -> String {
        match (self.tag("UTCDate"), self.tag("UTCTime")) {
            (Some(d), Some(t)) => format!("{d} {t}"),
            _ => self.date().unwrap_or("").to_owned(),
        }
    }

    pub fn ply_count(&self) -> Option<u32> {
        self.tag("PlyCount").and_then(|v| v.trim().parse().ok())
    }
}

/// Progress report, called roughly every 64 KiB and at the end.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScanProgress {
    pub bytes_done: usize,
    pub bytes_total: usize,
    pub games: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("scan cancelled")]
pub struct Cancelled;

const PROGRESS_STEP: usize = 64 * 1024;

/// Splits `bytes` into records. Thread-safe to call from a background thread;
/// `cancel` is polled between tokens.
///
/// Record boundaries: a tag line (`[Name "`, at the start of a line, outside
/// comments) after movetext; or a new movetext token after a top-level result.
pub fn scan_games(
    bytes: &[u8],
    encoding: TextEncoding,
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> Result<Vec<GameEntry>, Cancelled> {
    #[derive(PartialEq)]
    enum State {
        Between,
        Header,
        Movetext,
    }

    let start = if bytes.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    let mut lx = Lexer::new(bytes, start, bytes.len(), 1);
    let mut entries: Vec<GameEntry> = Vec::new();
    let mut state = State::Between;
    let mut cur: Option<GameEntry> = None;
    let mut depth: i64 = 0;
    let mut result_seen = false;
    let mut movetext_tokens = 0usize;
    let mut next_report = PROGRESS_STEP;

    let finish = |cur: &mut Option<GameEntry>,
                  entries: &mut Vec<GameEntry>,
                  end: usize,
                  depth: i64,
                  movetext_tokens: usize| {
        if let Some(mut e) = cur.take() {
            e.byte_range.end = end;
            if depth != 0 {
                e.flags.has_structure_issues = true;
            }
            e.flags.empty_movetext = movetext_tokens == 0;
            entries.push(e);
        }
    };

    while let Some(tok) = lx.next_token() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        if tok.start >= next_report {
            next_report = tok.start + PROGRESS_STEP;
            progress(ScanProgress {
                bytes_done: tok.start,
                bytes_total: bytes.len(),
                games: entries.len(),
            });
        }

        if tok.kind == TokKind::Escape && state == State::Between {
            continue;
        }
        let is_tag = matches!(tok.kind, TokKind::Tag { .. });
        let new_game = match state {
            State::Between => true,
            State::Header => false,
            State::Movetext => {
                is_tag || (result_seen && matches!(tok.kind, TokKind::Word | TokKind::Open))
            }
        };
        if new_game {
            finish(&mut cur, &mut entries, tok.start, depth, movetext_tokens);
            cur = Some(GameEntry {
                index: entries.len(),
                byte_range: tok.start..tok.start,
                first_line: tok.line,
                tags: Vec::new(),
                flags: EntryFlags::default(),
                main_plies: 0,
                all_plies: 0,
            });
            depth = 0;
            result_seen = false;
            movetext_tokens = 0;
            state = if is_tag {
                State::Header
            } else {
                State::Movetext
            };
        } else if !is_tag && state == State::Header {
            state = State::Movetext;
        }
        let Some(entry) = cur.as_mut() else { continue };

        match tok.kind {
            TokKind::Tag { malformed } => {
                if state == State::Movetext {
                    // Unreachable by construction (tag in movetext starts a new game).
                    continue;
                }
                match parse_tag(bytes, &tok, encoding) {
                    Some((name, value, had_errors)) => {
                        if name == "FEN" {
                            entry.flags.has_fen = true;
                        }
                        entry.flags.has_decode_errors |= had_errors;
                        entry.tags.push((name, value));
                    }
                    None => entry.flags.has_structure_issues = true,
                }
                if malformed {
                    entry.flags.has_structure_issues = true;
                }
            }
            TokKind::Comment { terminated } => {
                movetext_tokens += 1;
                entry.flags.has_comments = true;
                let (from, to) = tok.comment_content();
                if !entry.flags.has_text_comments && has_words(&bytes[from..to.max(from)]) {
                    entry.flags.has_text_comments = true;
                }
                if !terminated {
                    entry.flags.has_structure_issues = true;
                }
            }
            TokKind::LineComment => {
                movetext_tokens += 1;
                entry.flags.has_comments = true;
                entry.flags.has_text_comments = true;
            }
            TokKind::Escape => {}
            TokKind::Open => {
                movetext_tokens += 1;
                entry.flags.has_variations = true;
                depth += 1;
            }
            TokKind::Close => {
                movetext_tokens += 1;
                depth -= 1;
                if depth < 0 {
                    entry.flags.has_structure_issues = true;
                    depth = 0;
                }
            }
            TokKind::StrayCloseBrace => {
                movetext_tokens += 1;
                entry.flags.has_structure_issues = true;
            }
            TokKind::Nag => {
                movetext_tokens += 1;
                entry.flags.has_nags = true;
            }
            TokKind::Word => {
                movetext_tokens += 1;
                let w = &bytes[tok.start..tok.end];
                if w == b"--" || w.ends_with(b".--") {
                    entry.flags.has_null_moves = true;
                }
                if depth == 0 && matches!(w, b"1-0" | b"0-1" | b"1/2-1/2" | b"*") {
                    result_seen = true;
                    lx.allow_tag();
                } else if is_move(w) {
                    entry.all_plies += 1;
                    if depth == 0 {
                        entry.main_plies += 1;
                    }
                }
            }
        }
    }
    finish(&mut cur, &mut entries, bytes.len(), depth, movetext_tokens);
    progress(ScanProgress {
        bytes_done: bytes.len(),
        bytes_total: bytes.len(),
        games: entries.len(),
    });
    Ok(entries)
}

/// A word of movetext that is a move: not a move number ("12.", "12..."), with or
/// without the move glued to it ("12.e4"), and not the "e.p." of an en passant capture.
fn is_move(w: &[u8]) -> bool {
    let rest = &w[w
        .iter()
        .position(|c| !c.is_ascii_digit() && *c != b'.')
        .unwrap_or(w.len())..];
    !rest.is_empty() && rest != b"e.p."
}

/// Text in a comment besides commands in square brackets (`[%clk …]`, `[%eval …]`,
/// `[%cal …]`).
fn has_words(text: &[u8]) -> bool {
    let mut depth = 0usize;
    for &c in text {
        match c {
            b'[' => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            c if depth == 0 && !c.is_ascii_whitespace() => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(s: &str) -> Vec<GameEntry> {
        scan_games(
            s.as_bytes(),
            TextEncoding::Utf8,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .expect("scan")
    }

    #[test]
    fn splits_games_and_reads_tags() {
        let s = "[Event \"A\"]\r\n[White \"W\"]\r\n\r\n1. e4 {x\r\n[%cal Ge2e4]} (1. d4) 1-0\r\n\r\n[Event \"B\"]\n[FEN \"8/8/8/8/8/8/8/K1k5 w - - 0 1\"]\n\n1. -- *\n";
        let e = scan(s);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].event(), Some("A"));
        assert_eq!(e[0].white(), Some("W"));
        assert!(e[0].flags.has_comments && e[0].flags.has_variations);
        assert!(!e[0].flags.has_structure_issues);
        assert_eq!(e[0].byte_range.start, 0);
        assert_eq!(e[0].byte_range.end, e[1].byte_range.start);
        assert_eq!(e[1].first_line, 7);
        assert!(e[1].flags.has_fen && e[1].flags.has_null_moves);
        assert_eq!(e[1].byte_range.end, s.len());
    }

    #[test]
    fn tagless_games_split_after_result() {
        let e = scan("1. e4 e5 1-0\n\n1. d4 d5 *\n");
        assert_eq!(e.len(), 2);
        assert!(e[0].tags.is_empty());
    }

    #[test]
    fn a_header_on_the_line_of_the_last_result_starts_the_next_game() {
        let e = scan("[White \"Aaa\"] 1. e4 e5 * [White \"Ccc\"][Black \"Ddd\"] 1. d4 d5 *\n");
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].white(), Some("Ccc"));
        assert_eq!(e[1].black(), Some("Ddd"));
    }

    #[test]
    fn main_line_moves_and_text_comments() {
        let e = scan(concat!(
            "1. e4 { [%clk 0:10:07] } e5 { [%clk 0:10:08] } 2.Nf3 (2. f4 exf4) 2...Nc6 *\n",
            "1. d4 { A quiet start. [%eval 0.2] } d5 1-0\n",
            "[PlyCount \"81\"] 1. c4 *\n",
        ));
        assert_eq!(e[0].main_plies, 4);
        assert_eq!(e[0].all_plies, 6);
        assert_eq!(e[0].moves(), 2);
        assert!(e[0].flags.has_comments && !e[0].flags.has_text_comments);
        assert!(e[1].flags.has_text_comments);
        assert_eq!(e[1].moves(), 1);
        assert_eq!(e[2].moves(), 41);
    }

    #[test]
    fn cancellation() {
        let cancel = AtomicBool::new(true);
        let r = scan_games(b"[A \"b\"]\n*", TextEncoding::Utf8, &mut |_| {}, &cancel);
        assert_eq!(r, Err(Cancelled));
    }

    #[test]
    fn empty_input() {
        assert!(scan("").is_empty());
        assert!(scan("  \n\n").is_empty());
    }
}
