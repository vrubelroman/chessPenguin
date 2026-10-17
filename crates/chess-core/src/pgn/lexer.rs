//! Byte-level PGN lexer shared by the index scanner and the game parser.
//!
//! Works on raw bytes of the original file (all supported encodings are
//! ASCII-compatible), so token spans are byte offsets in the original file.
//! Every non-whitespace byte belongs to exactly one token: nothing is skipped
//! silently, which is what makes coverage diagnostics possible.

use crate::encoding::{TextEncoding, decode_text};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokKind {
    /// `[Name "value"]`; `malformed` if the line could not be parsed as a tag.
    Tag {
        malformed: bool,
    },
    /// `{…}`. Content is `start+1..end-1` if terminated, else `start+1..end`.
    Comment {
        terminated: bool,
    },
    /// `;…` to end of line.
    LineComment,
    /// `%…` line at column 0.
    Escape,
    Open,
    Close,
    StrayCloseBrace,
    /// `$digits`.
    Nag,
    /// Any other run of non-delimiter bytes (moves, numbers, results, garbage).
    Word,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
    /// 1-based line of `start`.
    pub line: u32,
}

impl Token {
    pub fn comment_content(&self) -> (usize, usize) {
        match self.kind {
            TokKind::Comment { terminated: true } => (self.start + 1, self.end.saturating_sub(1)),
            _ => (self.start + 1, self.end),
        }
    }
}

pub(crate) struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
    line: u32,
    /// Only whitespace (or tags) since the last newline.
    line_blank: bool,
    /// Directly after a newline (column 0).
    col0: bool,
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'{' | b'}' | b'(' | b')' | b';' | b'$') || b.is_ascii_whitespace()
}

impl<'a> Lexer<'a> {
    pub fn new(bytes: &'a [u8], start: usize, end: usize, first_line: u32) -> Self {
        let end = end.min(bytes.len());
        let start = start.min(end);
        Lexer {
            bytes,
            pos: start,
            end,
            line: first_line,
            line_blank: true,
            col0: true,
        }
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    fn peek(&self) -> Option<u8> {
        (self.pos < self.end).then(|| self.bytes[self.pos])
    }

    fn at(&self, i: usize) -> Option<u8> {
        (i < self.end).then(|| self.bytes[i])
    }

    fn bump(&mut self) {
        if let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'\n' {
                self.line += 1;
                self.line_blank = true;
                self.col0 = true;
            } else {
                self.col0 = false;
                if !b.is_ascii_whitespace() {
                    self.line_blank = false;
                }
            }
        }
    }

    fn skip_to_eol(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'\n' {
                break;
            }
            self.bump();
        }
    }

    /// Is there a tag-looking line (`[Name "`) at `i`?
    fn tag_like_at(&self, mut i: usize) -> bool {
        if self.at(i) != Some(b'[') {
            return false;
        }
        i += 1;
        let name_start = i;
        while let Some(b) = self.at(i) {
            if b.is_ascii_alphanumeric() || b == b'_' {
                i += 1;
            } else {
                break;
            }
        }
        if i == name_start {
            return false;
        }
        while let Some(b' ' | b'\t') = self.at(i) {
            i += 1;
        }
        self.at(i) == Some(b'"')
    }

    /// Inside a comment: does a blank line followed by a header line start at `i`
    /// (which points at a `\n`)? Used to stop runaway unterminated comments.
    fn header_after_blank_line(&self, i: usize) -> bool {
        let mut j = i + 1;
        let mut saw_blank = false;
        loop {
            match self.at(j) {
                Some(b' ' | b'\t' | b'\r') => j += 1,
                Some(b'\n') => {
                    saw_blank = true;
                    j += 1;
                }
                _ => break,
            }
        }
        saw_blank && self.tag_like_at(j)
    }

    pub fn next_token(&mut self) -> Option<Token> {
        // Skip whitespace.
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
        let b = self.peek()?;
        let start = self.pos;
        let line = self.line;
        let kind = match b {
            b'%' if self.col0 => {
                self.skip_to_eol();
                TokKind::Escape
            }
            b'[' if self.line_blank && self.tag_like_at(self.pos) => self.lex_tag(),
            b'{' => {
                self.bump();
                let mut terminated = false;
                while let Some(c) = self.peek() {
                    if c == b'}' {
                        self.bump();
                        terminated = true;
                        break;
                    }
                    if c == b'\n' && self.header_after_blank_line(self.pos) {
                        break;
                    }
                    self.bump();
                }
                TokKind::Comment { terminated }
            }
            b';' => {
                self.skip_to_eol();
                TokKind::LineComment
            }
            b'(' => {
                self.bump();
                TokKind::Open
            }
            b')' => {
                self.bump();
                TokKind::Close
            }
            b'}' => {
                self.bump();
                TokKind::StrayCloseBrace
            }
            b'$' if self.at(self.pos + 1).is_some_and(|d| d.is_ascii_digit()) => {
                self.bump();
                while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                    self.bump();
                }
                TokKind::Nag
            }
            _ => {
                self.bump();
                while self.peek().is_some_and(|c| !is_delim(c)) {
                    self.bump();
                }
                TokKind::Word
            }
        };
        Some(Token {
            kind,
            start,
            end: self.pos,
            line,
        })
    }

    fn lex_tag(&mut self) -> TokKind {
        // Called at '['. Parse `[Name "value"]` with escapes; tolerate missing ']'.
        self.bump();
        while self.peek().is_some_and(|c| c != b'"' && c != b'\n') {
            self.bump();
        }
        if self.peek() != Some(b'"') {
            return TokKind::Tag { malformed: true };
        }
        self.bump();
        let mut closed_quote = false;
        while let Some(c) = self.peek() {
            match c {
                b'\\' => {
                    self.bump();
                    if self.peek().is_some_and(|n| n != b'\n') {
                        self.bump();
                    }
                }
                b'"' => {
                    self.bump();
                    closed_quote = true;
                    break;
                }
                b'\n' => break,
                _ => self.bump(),
            }
        }
        if !closed_quote {
            return TokKind::Tag { malformed: true };
        }
        while let Some(b' ' | b'\t' | b'\r') = self.peek() {
            self.bump();
        }
        let malformed = if self.peek() == Some(b']') {
            self.bump();
            false
        } else {
            true
        };
        // A tag line counts as "blank" so that several tags may share a line.
        self.line_blank = true;
        TokKind::Tag { malformed }
    }
}

/// Parses a `Tag` token into name and decoded value.
pub(crate) fn parse_tag(
    bytes: &[u8],
    tok: &Token,
    encoding: TextEncoding,
) -> Option<(String, String, bool)> {
    let raw = bytes.get(tok.start..tok.end)?;
    let raw = raw.strip_prefix(b"[")?;
    let name_len = raw
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))?;
    if name_len == 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&raw[..name_len]).into_owned();
    let rest = &raw[name_len..];
    let q = rest.iter().position(|b| *b == b'"')?;
    let mut value = Vec::new();
    let mut i = q + 1;
    while let Some(&c) = rest.get(i) {
        match c {
            b'\\' => {
                if let Some(&n) = rest.get(i + 1) {
                    value.push(n);
                }
                i += 2;
            }
            b'"' => break,
            b'\r' | b'\n' => break,
            _ => {
                value.push(c);
                i += 1;
            }
        }
    }
    let (value, had_errors) = decode_text(encoding, &value);
    Some((name, value, had_errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(TokKind, String)> {
        let b = s.as_bytes();
        let mut lx = Lexer::new(b, 0, b.len(), 1);
        let mut out = Vec::new();
        while let Some(t) = lx.next_token() {
            out.push((t.kind, s[t.start..t.end].to_owned()));
        }
        out
    }

    #[test]
    fn lexes_movetext() {
        let k = kinds("[White \"A \\\"q\\\"\"]\n1. e4 {c (x)} (1. d4 $1) ; rest\n%esc\n} *");
        let names: Vec<_> = k.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            names,
            vec![
                TokKind::Tag { malformed: false },
                TokKind::Word,
                TokKind::Word,
                TokKind::Comment { terminated: true },
                TokKind::Open,
                TokKind::Word,
                TokKind::Word,
                TokKind::Nag,
                TokKind::Close,
                TokKind::LineComment,
                TokKind::Escape,
                TokKind::StrayCloseBrace,
                TokKind::Word,
            ]
        );
    }

    #[test]
    fn tag_value_unescaped() {
        let s = "[White \"A \\\"q\\\" \\\\\"]";
        let b = s.as_bytes();
        let mut lx = Lexer::new(b, 0, b.len(), 1);
        let t = lx.next_token().expect("tag");
        let (n, v, _) = parse_tag(b, &t, TextEncoding::Utf8).expect("parse");
        assert_eq!(n, "White");
        assert_eq!(v, "A \"q\" \\");
    }

    #[test]
    fn runaway_comment_stops_at_header() {
        let k = kinds("1. e4 {open\n\n[Event \"x\"]\n1. d4 *");
        assert_eq!(k[2].0, TokKind::Comment { terminated: false });
        assert_eq!(k[3].0, TokKind::Tag { malformed: false });
    }
}
