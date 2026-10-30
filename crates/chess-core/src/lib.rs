//! Chess model, game tree and PGN import/export.
//!
//! No UI dependencies. See `docs/dev/chess-core.md` for the API overview and
//! the compatibility policy for real-world PGN.

pub mod comment;
pub mod database;
pub mod diagnostic;
pub mod edit;
pub mod encoding;
pub mod game;
pub mod nag;
pub mod paste;
pub mod pgn;
pub mod repair;
pub mod start;

pub use comment::{
    Annotation, CommentPart, ShapeColor, comment_display_text, parse_comment, shapes, with_shapes,
    with_text,
};
pub use database::{OpenError, PgnDatabase, PgnIndex, SourceFingerprint};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity, SourceLocation};
pub use edit::{EditError, EditOutcome};
pub use encoding::{Confidence, EncodingGuess, TextEncoding, detect_encoding};
pub use game::{EngineHistory, Game, GameMove, GameResult, HistoryKind, MoveKind, Node, NodeId};
pub use nag::Nag;
pub use pgn::{
    Cancelled, EntryFlags, GameEntry, ParseOptions, ParsedGame, ScanProgress, WriteOptions,
    parse_game, parse_str, scan_games, write_game,
};
pub use repair::{LetterRepairCounts, LetterRepairDetection};
pub use start::{FenError, FenRepair, StartPosition};

/// Re-export of the rules library used in the public API.
pub use shakmaty;
