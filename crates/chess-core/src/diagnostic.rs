//! Import diagnostics.

use std::fmt;

use crate::game::NodeId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Tolerated deviation, data fully preserved (e.g. FEN fullmove 0).
    Info,
    /// Data preserved but changed in form or placement (e.g. stray text kept as comment).
    Warning,
    /// Data could not be interpreted as chess (e.g. illegal move); raw text kept.
    Error,
}

/// Machine-readable diagnostic kind; UI localizes by code, `message` gives details.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    /// UTF-8 decoding produced replacement characters.
    DecodeError,
    MalformedTag,
    DuplicateTag,
    /// FEN with fullmove number 0, normalized to 1 (original tag kept).
    FenFullmoveZero,
    /// FEN tag without `SetUp "1"`; FEN used anyway.
    FenWithoutSetUp,
    /// `SetUp "1"` without FEN; standard start position used.
    SetUpWithoutFen,
    /// FEN repaired (invalid castling rights/en passant square dropped, etc.).
    FenRepaired,
    /// FEN cannot be used; movetext preserved as raw text.
    InvalidFen,
    UnsupportedVariant,
    /// Illegal or ambiguous SAN, or `--` while in check; the rest of the
    /// branch is kept as raw text.
    IllegalMove,
    /// Text outside comments that is not PGN; kept as a comment.
    UnrecognizedText,
    /// `}` outside a comment; dropped.
    StrayClosingBrace,
    UnterminatedComment,
    UnmatchedCloseParen,
    UnclosedVariation,
    /// `(` before any move; content kept as raw text.
    VariationWithoutMove,
    EmptyVariation,
    /// `%` escape line, ignored per PGN standard.
    EscapeLine,
    ResultInVariation,
    ResultMismatch,
    MissingResult,
    /// Letters replaced by ASCII sequences in the source (`~~`, `><`, `o^`)
    /// were restored in comments; counts in `message`.
    LettersRestored,
    /// Annotation could not be placed in its original position.
    CommentMoved,
}

/// Position in the source. Offsets are bytes of the original file.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    pub byte_offset: usize,
    /// 1-based line in the original file.
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub location: Option<SourceLocation>,
    /// Node the diagnostic is attached to (e.g. where recovered text was stored).
    pub node: Option<NodeId>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} {:?}", self.severity, self.code)?;
        if let Some(loc) = self.location {
            write!(f, " at line {} (byte {})", loc.line, loc.byte_offset)?;
        }
        write!(f, ": {}", self.message)
    }
}
