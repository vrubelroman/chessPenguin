//! Public data types shared by the parser and the supervisor.

use std::time::Duration;

use crate::error::EngineError;

/// Caller-assigned identifier of a search request.
///
/// UCI itself has no request identifiers; the supervisor attaches this local
/// id to every [`crate::EngineEvent`] that belongs to the search and
/// guarantees that output of a previous search is never reported under a newer
/// id (see the crate documentation for the boundary rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SearchId(pub u64);

/// Side to move. Kept local so that the crate does not depend on chess-core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    White,
    Black,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::White => Side::Black,
            Side::Black => Side::White,
        }
    }
}

/// Score exactly as the engine reported it: relative to the side to move in
/// the analysed (root) position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Score {
    /// Centipawns, positive = good for the side to move.
    Cp(i32),
    /// Mate in N moves (not plies). Positive: side to move mates;
    /// negative: side to move gets mated; `0`: side to move is already mated.
    Mate(i32),
}

/// `lowerbound` / `upperbound` qualifier of a score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScoreBound {
    #[default]
    Exact,
    /// The true score is at least the reported value.
    Lower,
    /// The true score is at most the reported value.
    Upper,
}

impl ScoreBound {
    fn flipped(self) -> ScoreBound {
        match self {
            ScoreBound::Exact => ScoreBound::Exact,
            ScoreBound::Lower => ScoreBound::Upper,
            ScoreBound::Upper => ScoreBound::Lower,
        }
    }
}

/// Engine score with its bound, relative to the side to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineScore {
    pub score: Score,
    pub bound: ScoreBound,
}

/// Evaluation from White's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteEval {
    /// Centipawns, positive = White is better.
    Cp(i32),
    /// Forced mate. `moves == 0` means the side to move is already mated.
    Mate { winner: Side, moves: u32 },
}

/// Score converted to White's point of view; the bound is flipped when the
/// sign of the score is flipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhiteScore {
    pub eval: WhiteEval,
    pub bound: ScoreBound,
}

impl EngineScore {
    /// Converts the engine score (relative to `side_to_move` of the analysed
    /// root position) into a White-relative score. Mate stays mate; it is
    /// never folded into centipawns.
    pub fn to_white(self, side_to_move: Side) -> WhiteScore {
        let flip = side_to_move == Side::Black;
        let eval = match self.score {
            Score::Cp(cp) => WhiteEval::Cp(if flip { cp.saturating_neg() } else { cp }),
            Score::Mate(n) if n > 0 => WhiteEval::Mate {
                winner: side_to_move,
                moves: n.unsigned_abs(),
            },
            Score::Mate(n) => WhiteEval::Mate {
                winner: side_to_move.opposite(),
                moves: n.unsigned_abs(),
            },
        };
        let bound = if flip {
            self.bound.flipped()
        } else {
            self.bound
        };
        WhiteScore { eval, bound }
    }
}

/// Parsed `info` line. Fields the engine did not send are `None` / empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchInfo {
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    /// MultiPV index (1-based). `None` when the engine omitted it, which means
    /// line 1.
    pub multipv: Option<u32>,
    pub score: Option<EngineScore>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub time_ms: Option<u64>,
    /// Hash usage in permille.
    pub hashfull: Option<u32>,
    pub tbhits: Option<u64>,
    /// Win/draw/loss in permille (Stockfish `UCI_ShowWDL`), side to move.
    pub wdl: Option<(u32, u32, u32)>,
    /// Principal variation in UCI move notation, as sent by the engine.
    pub pv: Vec<String>,
}

impl SearchInfo {
    /// MultiPV index, defaulting to 1.
    pub fn multipv_index(&self) -> u32 {
        self.multipv.unwrap_or(1)
    }
}

/// Typed `option` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineOption {
    /// Name as declared by the engine (UCI names are case-insensitive).
    pub name: String,
    pub kind: OptionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionKind {
    Check {
        default: bool,
    },
    Spin {
        default: i64,
        min: Option<i64>,
        max: Option<i64>,
    },
    Combo {
        default: String,
        vars: Vec<String>,
    },
    Button,
    /// Free text. The default is kept verbatim (Stockfish uses `<empty>`).
    String {
        default: String,
    },
}

/// Result of the `uci` handshake.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineInfo {
    pub name: Option<String>,
    pub author: Option<String>,
    pub options: Vec<EngineOption>,
}

impl EngineInfo {
    /// Case-insensitive option lookup (UCI option names are case-insensitive).
    pub fn option(&self, name: &str) -> Option<&EngineOption> {
        self.options
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
    }
}

/// Root of a position sent with `position`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionBase {
    StartPos,
    Fen(String),
}

/// Position to analyse: a base position plus the move history in UCI
/// notation. Prefer sending the real history instead of only the final FEN,
/// so the engine can detect repetitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePosition {
    pub base: PositionBase,
    pub moves: Vec<String>,
}

impl EnginePosition {
    pub fn startpos<I, S>(moves: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        EnginePosition {
            base: PositionBase::StartPos,
            moves: moves.into_iter().map(Into::into).collect(),
        }
    }

    pub fn fen<I, S>(fen: impl Into<String>, moves: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        EnginePosition {
            base: PositionBase::Fen(fen.into()),
            moves: moves.into_iter().map(Into::into).collect(),
        }
    }

    /// Checks that the position can be sent safely: no line breaks or control
    /// characters, no `moves` token inside the FEN, and every move is plain
    /// UCI coordinate notation. The null move `0000` is rejected: training
    /// `--` must be converted by the caller (e.g. by sending the resulting
    /// FEN), not passed as an ordinary move.
    pub fn validate(&self) -> Result<(), EngineError> {
        if let PositionBase::Fen(fen) = &self.base {
            if fen.trim().is_empty() {
                return Err(EngineError::InvalidInput("empty FEN".into()));
            }
            if fen.chars().any(char::is_control) {
                return Err(EngineError::InvalidInput(
                    "FEN contains control characters".into(),
                ));
            }
            if fen.split_ascii_whitespace().any(|t| t == "moves") {
                return Err(EngineError::InvalidInput(
                    "FEN contains the token `moves`".into(),
                ));
            }
        }
        for mv in &self.moves {
            if !is_uci_move(mv) {
                return Err(EngineError::InvalidInput(format!("not a UCI move: {mv:?}")));
            }
        }
        Ok(())
    }

    /// The `position …` line for this position.
    pub fn to_command(&self) -> String {
        let mut s = match &self.base {
            PositionBase::StartPos => "position startpos".to_string(),
            PositionBase::Fen(fen) => format!("position fen {}", fen.trim()),
        };
        if !self.moves.is_empty() {
            s.push_str(" moves");
            for m in &self.moves {
                s.push(' ');
                s.push_str(m);
            }
        }
        s
    }
}

pub(crate) fn is_uci_move(m: &str) -> bool {
    let b = m.as_bytes();
    let sq = |f: u8, r: u8| (b'a'..=b'h').contains(&f) && (b'1'..=b'8').contains(&r);
    match b.len() {
        4 => sq(b[0], b[1]) && sq(b[2], b[3]),
        5 => sq(b[0], b[1]) && sq(b[2], b[3]) && matches!(b[4], b'q' | b'r' | b'b' | b'n'),
        _ => false,
    }
}

/// Limits for `go`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchLimits {
    /// `go infinite`: runs until stopped or until the position changes.
    Infinite,
    Depth(u32),
    MoveTime(Duration),
    Nodes(u64),
}

impl SearchLimits {
    /// The `go …` line for these limits.
    pub fn to_command(self) -> String {
        match self {
            SearchLimits::Infinite => "go infinite".into(),
            SearchLimits::Depth(d) => format!("go depth {d}"),
            SearchLimits::MoveTime(t) => format!("go movetime {}", t.as_millis().max(1)),
            SearchLimits::Nodes(n) => format!("go nodes {n}"),
        }
    }
}

/// Timeouts used by the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// `uci` → `uciok`.
    pub uciok: Duration,
    /// `isready` → `readyok` (handshake and every sync before `go`).
    /// Large `Hash` values may need a long time to allocate.
    pub readyok: Duration,
    /// `stop` → `bestmove`. On expiry the engine is considered hung and killed.
    pub stop: Duration,
    /// `quit` → process exit. On expiry the process is killed.
    pub quit: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            uciok: Duration::from_secs(10),
            readyok: Duration::from_secs(10),
            stop: Duration::from_secs(3),
            quit: Duration::from_secs(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(score: Score, bound: ScoreBound) -> EngineScore {
        EngineScore { score, bound }
    }

    #[test]
    fn white_perspective_cp() {
        let w = s(Score::Cp(35), ScoreBound::Lower).to_white(Side::White);
        assert_eq!(w.eval, WhiteEval::Cp(35));
        assert_eq!(w.bound, ScoreBound::Lower);
        let b = s(Score::Cp(35), ScoreBound::Lower).to_white(Side::Black);
        assert_eq!(b.eval, WhiteEval::Cp(-35));
        assert_eq!(b.bound, ScoreBound::Upper);
        let n = s(Score::Cp(-120), ScoreBound::Exact).to_white(Side::Black);
        assert_eq!(n.eval, WhiteEval::Cp(120));
    }

    #[test]
    fn white_perspective_mate() {
        let m = |n, side| s(Score::Mate(n), ScoreBound::Exact).to_white(side).eval;
        assert_eq!(
            m(3, Side::Black),
            WhiteEval::Mate {
                winner: Side::Black,
                moves: 3
            }
        );
        assert_eq!(
            m(-2, Side::Black),
            WhiteEval::Mate {
                winner: Side::White,
                moves: 2
            }
        );
        assert_eq!(
            m(-2, Side::White),
            WhiteEval::Mate {
                winner: Side::Black,
                moves: 2
            }
        );
        // mate 0: side to move is mated.
        assert_eq!(
            m(0, Side::White),
            WhiteEval::Mate {
                winner: Side::Black,
                moves: 0
            }
        );
        assert_eq!(
            m(0, Side::Black),
            WhiteEval::Mate {
                winner: Side::White,
                moves: 0
            }
        );
    }

    #[test]
    fn position_validation_and_command() {
        let p = EnginePosition::startpos(["e2e4", "e7e5", "e7e8q"]);
        p.validate().unwrap();
        assert_eq!(p.to_command(), "position startpos moves e2e4 e7e5 e7e8q");
        assert!(EnginePosition::startpos(["0000"]).validate().is_err());
        assert!(EnginePosition::startpos(["e2e4\nquit"]).validate().is_err());
        assert!(EnginePosition::startpos(["E2E4"]).validate().is_err());
        let f = EnginePosition::fen("8/8/8/8/8/8/8/K6k w - - 0 1", Vec::<String>::new());
        f.validate().unwrap();
        assert_eq!(f.to_command(), "position fen 8/8/8/8/8/8/8/K6k w - - 0 1");
        assert!(
            EnginePosition::fen("x\ny", Vec::<String>::new())
                .validate()
                .is_err()
        );
        assert!(
            EnginePosition::fen("a moves b", Vec::<String>::new())
                .validate()
                .is_err()
        );
        assert!(
            EnginePosition::fen("  ", Vec::<String>::new())
                .validate()
                .is_err()
        );
    }
}
