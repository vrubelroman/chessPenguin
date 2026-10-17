//! Full parse of one PGN record into a [`Game`] with diagnostics.

use std::ops::Range;

use shakmaty::{
    Chess, Position,
    san::{San, SanError, SanPlus},
};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity, SourceLocation};
use crate::encoding::{TextEncoding, decode_text};
use crate::game::{Game, GameMove, GameResult, MoveKind, NodeId, play_null};
use crate::nag::Nag;
use crate::pgn::lexer::{Lexer, TokKind, Token, parse_tag};
use crate::repair::{LetterRepairCounts, repair_letters};
use crate::start::{FenRepair, StartPosition};

/// A parsed record: the game tree plus everything noticed on the way.
#[derive(Clone, Debug)]
pub struct ParsedGame {
    pub game: Game,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedGame {
    pub fn max_severity(&self) -> Option<Severity> {
        self.diagnostics.iter().map(|d| d.severity).max()
    }
}

/// Parses the first record in `bytes` (offsets relative to `bytes`).
pub fn parse_game(bytes: &[u8], encoding: TextEncoding) -> ParsedGame {
    let start = if bytes.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    parse_game_range(bytes, start..bytes.len(), 1, encoding)
}

/// Parses UTF-8 text (e.g. clipboard).
pub fn parse_str(text: &str) -> ParsedGame {
    parse_game(text.as_bytes(), TextEncoding::Utf8)
}

/// Parses the record at `range` of a file; diagnostics use absolute offsets and
/// lines (`first_line` is the line at `range.start`).
pub fn parse_game_range(
    bytes: &[u8],
    range: Range<usize>,
    first_line: u32,
    encoding: TextEncoding,
) -> ParsedGame {
    parse_game_range_with(bytes, range, first_line, encoding, &ParseOptions::default())
}

/// Options for [`parse_game_range_with`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// Restore letters replaced by ASCII sequences in comments
    /// (see [`crate::repair`]). Enable only for sources where
    /// [`crate::repair::detect_letter_substitution`] reported the pattern.
    pub repair_letters: bool,
}

/// Like [`parse_game_range`] with options.
pub fn parse_game_range_with(
    bytes: &[u8],
    range: Range<usize>,
    first_line: u32,
    encoding: TextEncoding,
    options: &ParseOptions,
) -> ParsedGame {
    let mut lx = Lexer::new(bytes, range.start, range.end, first_line);
    let mut diags = Vec::new();
    let mut decode_errors = false;

    // --- header -----------------------------------------------------------
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut first_movetext: Option<Token> = None;
    while let Some(tok) = lx.next_token() {
        match tok.kind {
            TokKind::Tag { malformed } => {
                let loc = Some(SourceLocation {
                    byte_offset: tok.start,
                    line: tok.line,
                });
                match parse_tag(bytes, &tok, encoding) {
                    Some((name, value, err)) => {
                        decode_errors |= err;
                        if malformed {
                            diags.push(diag(
                                Severity::Warning,
                                DiagnosticCode::MalformedTag,
                                format!("tag {name} is not properly closed"),
                                loc,
                                None,
                            ));
                        }
                        if tags.iter().any(|(k, _)| *k == name) {
                            diags.push(diag(
                                Severity::Info,
                                DiagnosticCode::DuplicateTag,
                                format!("duplicate tag {name} (all occurrences kept)"),
                                loc,
                                None,
                            ));
                        }
                        tags.push((name, value));
                    }
                    None => {
                        let (raw, _) = decode_text(encoding, &bytes[tok.start..tok.end]);
                        diags.push(diag(
                            Severity::Warning,
                            DiagnosticCode::MalformedTag,
                            format!("unparsable tag line dropped: {raw}"),
                            loc,
                            None,
                        ));
                    }
                }
            }
            TokKind::Escape => {}
            _ => {
                first_movetext = Some(tok);
                break;
            }
        }
    }

    // --- start position ---------------------------------------------------
    let tag = |name: &str| {
        tags.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let header_loc = Some(SourceLocation {
        byte_offset: range.start,
        line: first_line,
    });
    let mut fen_failed = false;
    let start = match tag("FEN") {
        Some(fen) => {
            if tag("SetUp").map(str::trim) != Some("1") {
                diags.push(diag(
                    Severity::Info,
                    DiagnosticCode::FenWithoutSetUp,
                    "FEN tag without SetUp \"1\"; FEN used".into(),
                    header_loc,
                    None,
                ));
            }
            match StartPosition::from_fen(fen) {
                Ok(start) => {
                    for r in start.repairs() {
                        let (sev, code, msg) = match r {
                            FenRepair::FullmoveZero => (
                                Severity::Info,
                                DiagnosticCode::FenFullmoveZero,
                                "FEN fullmove number 0 treated as 1; tag kept unchanged",
                            ),
                            FenRepair::InvalidCastlingRightsDropped => (
                                Severity::Warning,
                                DiagnosticCode::FenRepaired,
                                "invalid castling rights in FEN ignored",
                            ),
                            FenRepair::InvalidEnPassantDropped => (
                                Severity::Warning,
                                DiagnosticCode::FenRepaired,
                                "invalid en passant square in FEN ignored",
                            ),
                            FenRepair::ImpossibleCheckAccepted => (
                                Severity::Warning,
                                DiagnosticCode::FenRepaired,
                                "FEN has an impossible check; accepted",
                            ),
                            FenRepair::TooMuchMaterialAccepted => (
                                Severity::Warning,
                                DiagnosticCode::FenRepaired,
                                "FEN has too much material; accepted",
                            ),
                        };
                        diags.push(diag(sev, code, msg.into(), header_loc, None));
                    }
                    start
                }
                Err(e) => {
                    fen_failed = true;
                    diags.push(diag(
                        Severity::Error,
                        DiagnosticCode::InvalidFen,
                        format!("{e}: {fen}; movetext kept as raw text"),
                        header_loc,
                        None,
                    ));
                    StartPosition::standard()
                }
            }
        }
        None => {
            if tag("SetUp").map(str::trim) == Some("1") {
                diags.push(diag(
                    Severity::Warning,
                    DiagnosticCode::SetUpWithoutFen,
                    "SetUp \"1\" without FEN; standard position used".into(),
                    header_loc,
                    None,
                ));
            }
            StartPosition::standard()
        }
    };
    if let Some(v) = tag("Variant") {
        let v = v.trim().to_ascii_lowercase();
        if !matches!(
            v.as_str(),
            "" | "standard" | "chess" | "from position" | "normal"
        ) {
            diags.push(diag(
                Severity::Warning,
                DiagnosticCode::UnsupportedVariant,
                format!("variant {v} is not supported; parsed as standard chess"),
                header_loc,
                None,
            ));
        }
    }

    let game = Game::with_start(start, tags);
    let mut b = Builder {
        bytes,
        encoding,
        game,
        diags,
        cur: NodeId::ROOT,
        line_moves: 0,
        stack: Vec::new(),
        pending: Vec::new(),
        fresh: false,
        unknown: None,
        decode_errors,
        unclosed_reported: false,
        repair_letters: options.repair_letters,
        repaired: LetterRepairCounts::default(),
    };

    if let Some(first) = first_movetext {
        if fen_failed {
            b.skip_raw(&mut lx, first);
        } else {
            b.run(&mut lx, first);
        }
    }
    b.finish(range.end, lx.line())
}

fn diag(
    severity: Severity,
    code: DiagnosticCode,
    message: String,
    location: Option<SourceLocation>,
    node: Option<NodeId>,
) -> Diagnostic {
    Diagnostic {
        severity,
        code,
        message,
        location,
        node,
    }
}

struct Frame {
    resume: NodeId,
    line_moves: usize,
}

struct Builder<'a> {
    bytes: &'a [u8],
    encoding: TextEncoding,
    game: Game,
    diags: Vec<Diagnostic>,
    cur: NodeId,
    /// Moves played in the current line (variation or main line).
    line_moves: usize,
    stack: Vec<Frame>,
    /// Comments waiting for the next move (after `(` or `)`).
    pending: Vec<String>,
    fresh: bool,
    /// Accumulated unrecognized words: (start, end, line).
    unknown: Option<(usize, usize, u32)>,
    decode_errors: bool,
    unclosed_reported: bool,
    repair_letters: bool,
    repaired: LetterRepairCounts,
}

enum Word {
    Result(GameResult),
    MoveNumber,
    Move { san: SanPlus, nag: Option<Nag> },
    SymbolNag(Nag),
    Unknown,
}

fn classify(word: &[u8]) -> Word {
    match word {
        b"1-0" => return Word::Result(GameResult::WhiteWins),
        b"0-1" => return Word::Result(GameResult::BlackWins),
        b"1/2-1/2" => return Word::Result(GameResult::Draw),
        b"*" => return Word::Result(GameResult::Unknown),
        _ => {}
    }
    if word == "½-½".as_bytes() {
        return Word::Result(GameResult::Draw);
    }
    if let Ok(s) = std::str::from_utf8(word)
        && let Some(nag) = Nag::from_symbol(s)
    {
        return Word::SymbolNag(nag);
    }
    // Castling written with zeros (`0-0`, `0-0-0`), possibly after a move number.
    if let Some(i) = find_zero_castling(word) {
        let mut fixed = word.to_vec();
        for b in &mut fixed[i..] {
            if *b == b'0' {
                *b = b'O';
            }
        }
        return classify_owned(&fixed);
    }
    // Strip a move number prefix: digits followed by dots, or leading dots.
    let digits = word.iter().take_while(|b| b.is_ascii_digit()).count();
    let rest = &word[digits..];
    let dots = rest.iter().take_while(|b| **b == b'.').count();
    let rest = &rest[dots..];
    if rest.is_empty() {
        return if digits > 0 || dots > 0 {
            Word::MoveNumber
        } else {
            Word::Unknown
        };
    }
    if digits > 0 && dots == 0 {
        return Word::Unknown;
    }
    classify_san(rest)
}

/// Index where a zero-castling (`0-0`) starts: at 0 or after `<digits>.`.
fn find_zero_castling(word: &[u8]) -> Option<usize> {
    if word.starts_with(b"0-0") && !matches!(word, b"0-1") {
        return Some(0);
    }
    let digits = word.iter().take_while(|b| b.is_ascii_digit()).count();
    let dots = word[digits..].iter().take_while(|b| **b == b'.').count();
    (dots > 0 && word[digits + dots..].starts_with(b"0-0")).then_some(digits + dots)
}

fn classify_owned(word: &[u8]) -> Word {
    // Same as `classify` without the zero-castling rewrite (already applied).
    let digits = word.iter().take_while(|b| b.is_ascii_digit()).count();
    let rest = &word[digits..];
    let dots = rest.iter().take_while(|b| **b == b'.').count();
    classify_san(&rest[dots..])
}

fn classify_san(rest: &[u8]) -> Word {
    match SanPlus::from_ascii_prefix(rest) {
        Ok((san, n)) => {
            let suffix = &rest[n..];
            if suffix.is_empty() {
                return Word::Move { san, nag: None };
            }
            match std::str::from_utf8(suffix).ok().and_then(Nag::from_symbol) {
                Some(nag) if nag.is_move_assessment() => Word::Move {
                    san,
                    nag: Some(nag),
                },
                _ => Word::Unknown,
            }
        }
        Err(_) => Word::Unknown,
    }
}

impl Builder<'_> {
    fn loc(tok: &Token) -> Option<SourceLocation> {
        Some(SourceLocation {
            byte_offset: tok.start,
            line: tok.line,
        })
    }

    fn text(&mut self, start: usize, end: usize) -> String {
        let (t, err) = decode_text(self.encoding, self.bytes.get(start..end).unwrap_or(&[]));
        self.decode_errors |= err;
        t
    }

    fn node_mut(&mut self, id: NodeId) -> &mut crate::game::Node {
        // `cur` and stack entries are always valid ids created by this builder.
        let len = self.game.nodes.len();
        &mut self.game.nodes[id.index().min(len - 1)]
    }

    fn place_comment(&mut self, text: String) {
        let text = if self.repair_letters {
            let (fixed, counts) = repair_letters(&text);
            self.repaired.add(&counts);
            fixed
        } else {
            text
        };
        if self.fresh {
            self.pending.push(text);
        } else {
            let cur = self.cur;
            self.node_mut(cur).comments_after.push(text);
        }
    }

    fn flush_unknown(&mut self) {
        if let Some((start, end, line)) = self.unknown.take() {
            let raw = self.text(start, end);
            let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            let preview: String = text
                .chars()
                .take(80)
                .collect::<String>()
                .trim_end()
                .to_owned();
            self.place_comment(text);
            let node = Some(self.cur);
            self.diags.push(diag(
                Severity::Warning,
                DiagnosticCode::UnrecognizedText,
                format!("text outside comment braces kept as a comment: {preview}"),
                Some(SourceLocation {
                    byte_offset: start,
                    line,
                }),
                node,
            ));
        }
    }

    fn run(&mut self, lx: &mut Lexer<'_>, first: Token) {
        let mut next = Some(first);
        while let Some(tok) = next.take().or_else(|| lx.next_token()) {
            if tok.kind != TokKind::Word && !matches!(tok.kind, TokKind::Tag { .. }) {
                self.flush_unknown();
            }
            match tok.kind {
                TokKind::Tag { .. } => self.add_unknown(&tok),
                TokKind::Comment { terminated } => {
                    let (s, e) = tok.comment_content();
                    let text = self.text(s, e).trim().to_owned();
                    if !terminated {
                        let node = Some(self.cur);
                        self.diags.push(diag(
                            Severity::Warning,
                            DiagnosticCode::UnterminatedComment,
                            "comment without closing brace; kept up to the end of the record"
                                .into(),
                            Self::loc(&tok),
                            node,
                        ));
                    }
                    self.place_comment(text);
                }
                TokKind::LineComment => {
                    let text = self.text(tok.start + 1, tok.end).trim().to_owned();
                    self.place_comment(text);
                }
                TokKind::Escape => {
                    let text = self.text(tok.start, tok.end);
                    self.diags.push(diag(
                        Severity::Info,
                        DiagnosticCode::EscapeLine,
                        format!("escape line ignored: {}", text.trim_end()),
                        Self::loc(&tok),
                        None,
                    ));
                }
                TokKind::StrayCloseBrace => {
                    let node = Some(self.cur);
                    self.diags.push(diag(
                        Severity::Warning,
                        DiagnosticCode::StrayClosingBrace,
                        "'}' outside a comment ignored; the comment was probably closed early"
                            .into(),
                        Self::loc(&tok),
                        node,
                    ));
                }
                TokKind::Nag => {
                    let digits = &self.bytes[tok.start + 1..tok.end];
                    match std::str::from_utf8(digits)
                        .ok()
                        .and_then(|s| s.parse::<u8>().ok())
                    {
                        Some(n) => {
                            let cur = self.cur;
                            self.node_mut(cur).nags.push(Nag(n));
                        }
                        None => self.add_unknown(&tok),
                    }
                }
                TokKind::Open => {
                    if self.line_moves == 0 {
                        // Nothing to vary: keep the whole parenthesized text raw.
                        let node = Some(self.cur);
                        self.diags.push(diag(
                            Severity::Error,
                            DiagnosticCode::VariationWithoutMove,
                            "variation before any move; kept as raw text".into(),
                            Self::loc(&tok),
                            node,
                        ));
                        self.skip_group(lx, tok);
                        continue;
                    }
                    let resume = self.cur;
                    let base = self.game.parent(resume).unwrap_or(NodeId::ROOT);
                    self.stack.push(Frame {
                        resume,
                        line_moves: self.line_moves,
                    });
                    self.cur = base;
                    self.line_moves = 0;
                    self.fresh = true;
                }
                TokKind::Close => self.close_variation(&tok),
                TokKind::Word => {
                    let class = classify(&self.bytes[tok.start..tok.end]);
                    if matches!(class, Word::Unknown) {
                        self.add_unknown(&tok);
                        continue;
                    }
                    self.flush_unknown();
                    match class {
                        Word::Result(r) => {
                            if self.stack.is_empty() {
                                if self.game.result.is_some() {
                                    self.diags.push(diag(
                                        Severity::Warning,
                                        DiagnosticCode::ResultMismatch,
                                        "second termination marker ignored".into(),
                                        Self::loc(&tok),
                                        None,
                                    ));
                                } else {
                                    self.game.result = Some(r);
                                }
                            } else {
                                self.diags.push(diag(
                                    Severity::Info,
                                    DiagnosticCode::ResultInVariation,
                                    format!("result {} inside a variation ignored", r.as_str()),
                                    Self::loc(&tok),
                                    Some(self.cur),
                                ));
                            }
                        }
                        Word::MoveNumber => {}
                        Word::SymbolNag(n) => {
                            let cur = self.cur;
                            self.node_mut(cur).nags.push(n);
                        }
                        Word::Move { san, nag } => {
                            if !self.play(san, nag, &tok)
                                && let Some(close) = self.skip_raw(lx, tok)
                            {
                                self.close_variation(&close);
                            }
                        }
                        Word::Unknown => {}
                    }
                }
            }
        }
    }

    fn add_unknown(&mut self, tok: &Token) {
        self.unknown = Some(match self.unknown {
            Some((s, _, line)) => (s, tok.end, line),
            None => (tok.start, tok.end, tok.line),
        });
    }

    fn close_variation(&mut self, tok: &Token) {
        let Some(frame) = self.stack.pop() else {
            self.diags.push(diag(
                Severity::Warning,
                DiagnosticCode::UnmatchedCloseParen,
                "')' without matching '(' ignored".into(),
                Self::loc(tok),
                Some(self.cur),
            ));
            return;
        };
        let target = if self.line_moves == 0 {
            self.diags.push(diag(
                Severity::Warning,
                DiagnosticCode::EmptyVariation,
                "empty variation".into(),
                Self::loc(tok),
                Some(frame.resume),
            ));
            frame.resume
        } else {
            self.cur
        };
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.diags.push(diag(
                Severity::Info,
                DiagnosticCode::CommentMoved,
                "comment after a nested variation at the end of a line attached to the last move"
                    .into(),
                Self::loc(tok),
                Some(target),
            ));
            self.node_mut(target).comments_after.extend(pending);
        }
        self.cur = frame.resume;
        self.line_moves = frame.line_moves;
        self.fresh = true;
    }

    /// Plays a SAN in the current position. Returns false if illegal.
    fn play(&mut self, san: SanPlus, nag: Option<Nag>, tok: &Token) -> bool {
        let pos: Chess = match self.game.position(self.cur) {
            Some(p) => p.clone(),
            None => return false,
        };
        let (kind, display, position) = if san.san == San::Null {
            match play_null(&pos) {
                Ok(p) => (MoveKind::Null, "--".to_owned(), p),
                Err(e) => {
                    self.report_illegal(tok, &format!("null move impossible ({e}):"));
                    return false;
                }
            }
        } else {
            match san.san.to_move(&pos) {
                Ok(m) => {
                    let display = SanPlus::from_move(pos.clone(), m).to_string();
                    let mut after = pos;
                    after.play_unchecked(m);
                    (MoveKind::Normal(m), display, after)
                }
                Err(err) => {
                    let why = match err {
                        SanError::AmbiguousSan => "ambiguous move",
                        _ => "illegal move",
                    };
                    self.report_illegal(tok, why);
                    return false;
                }
            }
        };
        let id = self
            .game
            .push_child(self.cur, GameMove { kind, san: display }, position);
        let pending = std::mem::take(&mut self.pending);
        let node = self.node_mut(id);
        node.comments_before = pending;
        if let Some(n) = nag {
            node.nags.push(n);
        }
        self.cur = id;
        self.line_moves += 1;
        self.fresh = false;
        true
    }

    fn report_illegal(&mut self, tok: &Token, why: &str) {
        let word = self.text(tok.start, tok.end);
        let fen = self.game.fen(self.cur).unwrap_or_default();
        self.diags.push(diag(
            Severity::Error,
            DiagnosticCode::IllegalMove,
            format!("{why} {word} in position {fen}; rest of the line kept as raw text"),
            Self::loc(tok),
            Some(self.cur),
        ));
    }

    /// For `(` without a move: keeps the balanced group including its `)` raw.
    fn skip_group(&mut self, lx: &mut Lexer<'_>, open: Token) {
        let mut depth = 0usize;
        let mut end = open.end;
        while let Some(t) = lx.next_token() {
            end = t.end;
            match t.kind {
                TokKind::Open => depth += 1,
                TokKind::Close => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let raw = self.text(open.start, end).trim().to_owned();
        let cur = self.cur;
        self.node_mut(cur).unparsed.push(raw);
    }

    /// Skips from `first` (inclusive) to the end of the current line, keeping
    /// the text verbatim in `cur.unparsed`. Returns the `)` that closes the
    /// current variation (not consumed into the raw text), if any. A trailing
    /// top-level result is not part of the raw text.
    fn skip_raw(&mut self, lx: &mut Lexer<'_>, first: Token) -> Option<Token> {
        let mut depth = 0usize;
        let mut end = first.end;
        let mut trailing_result: Option<(usize, GameResult)> = None;
        let mut close = None;
        let mut tok = Some(first);
        // `first` itself may be `(`/`)` only when called for an invalid FEN.
        while let Some(t) = tok.take().or_else(|| lx.next_token()) {
            match t.kind {
                TokKind::Open => depth += 1,
                TokKind::Close => {
                    if depth == 0 && !self.stack.is_empty() {
                        close = Some(t);
                        break;
                    }
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            trailing_result = None;
            if t.kind == TokKind::Word
                && depth == 0
                && self.stack.is_empty()
                && let Word::Result(r) = classify(&self.bytes[t.start..t.end])
            {
                trailing_result = Some((t.start, r));
            }
            end = t.end;
        }
        let raw_end = match trailing_result {
            Some((start, r)) => {
                if self.game.result.is_none() {
                    self.game.result = Some(r);
                }
                start
            }
            None => end,
        };
        let raw = self.text(first.start, raw_end).trim().to_owned();
        if !raw.is_empty() {
            let cur = self.cur;
            self.node_mut(cur).unparsed.push(raw);
        }
        close
    }

    fn finish(mut self, end: usize, line: u32) -> ParsedGame {
        self.flush_unknown();
        let loc = Some(SourceLocation {
            byte_offset: end,
            line,
        });
        while !self.stack.is_empty() {
            if !self.unclosed_reported {
                self.unclosed_reported = true;
                self.diags.push(diag(
                    Severity::Warning,
                    DiagnosticCode::UnclosedVariation,
                    "variation not closed before the end of the record".into(),
                    loc,
                    Some(self.cur),
                ));
            }
            let tok = Token {
                kind: TokKind::Close,
                start: end,
                end,
                line,
            };
            self.close_variation(&tok);
        }
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let cur = self.cur;
            self.diags.push(diag(
                Severity::Info,
                DiagnosticCode::CommentMoved,
                "comment after the last variation of the game attached to the last move".into(),
                loc,
                Some(cur),
            ));
            self.node_mut(cur).comments_after.extend(pending);
        }
        if self.repaired.total() > 0 {
            let c = self.repaired;
            self.diags.push(diag(
                Severity::Info,
                DiagnosticCode::LettersRestored,
                format!(
                    "restored letters replaced in the source: {} (ч {}, Ч {}, ю {}, Ю {}); original kept in raw text",
                    c.total(),
                    c.ch_lower,
                    c.ch_upper,
                    c.yu_lower,
                    c.yu_upper
                ),
                None,
                None,
            ));
        }
        if self.decode_errors {
            self.diags.push(diag(
                Severity::Warning,
                DiagnosticCode::DecodeError,
                format!(
                    "text is not valid {}; replacement characters inserted",
                    self.encoding.label()
                ),
                None,
                None,
            ));
        }
        let tag_result = self.game.tag("Result").map(str::to_owned);
        match (
            self.game.result,
            tag_result.as_deref().and_then(GameResult::from_pgn),
        ) {
            (Some(m), Some(t)) if m != t => self.diags.push(diag(
                Severity::Warning,
                DiagnosticCode::ResultMismatch,
                format!(
                    "Result tag {} differs from termination marker {}",
                    t.as_str(),
                    m.as_str()
                ),
                loc,
                None,
            )),
            (None, _) => self.diags.push(diag(
                Severity::Info,
                DiagnosticCode::MissingResult,
                "no termination marker".into(),
                loc,
                None,
            )),
            _ => {}
        }
        ParsedGame {
            game: self.game,
            diagnostics: self.diags,
        }
    }
}
