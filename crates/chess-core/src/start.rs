//! Tolerant start-position (FEN) handling.

use shakmaty::{CastlingMode, Chess, EnPassantMode, Position, fen::Fen};

/// Deviations from a strict FEN that were tolerated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FenRepair {
    /// Fullmove field was `0`; position uses 1.
    FullmoveZero,
    InvalidCastlingRightsDropped,
    InvalidEnPassantDropped,
    /// Side not to move is in check (kept; e.g. composed diagrams).
    ImpossibleCheckAccepted,
    TooMuchMaterialAccepted,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FenError {
    #[error("FEN syntax error: {0}")]
    Syntax(String),
    #[error("{0}")]
    IllegalPosition(String),
}

/// The game's initial position.
#[derive(Clone, Debug, Default)]
pub struct StartPosition {
    position: Chess,
    /// FEN text as given by the source (unchanged), `None` for the standard start.
    original_fen: Option<String>,
    repairs: Vec<FenRepair>,
}

impl StartPosition {
    pub fn standard() -> Self {
        Self::default()
    }

    /// Parses a FEN tolerantly (see [`FenRepair`]).
    pub fn from_fen(fen: &str) -> Result<Self, FenError> {
        let fen_trimmed = fen.trim();
        let mut repairs = Vec::new();
        let fields: Vec<&str> = fen_trimmed.split_whitespace().collect();
        if fields.get(5) == Some(&"0") {
            repairs.push(FenRepair::FullmoveZero);
        }
        let parsed =
            Fen::from_ascii(fen_trimmed.as_bytes()).map_err(|e| FenError::Syntax(e.to_string()))?;
        let position = match parsed.into_position::<Chess>(CastlingMode::Standard) {
            Ok(p) => p,
            Err(err) => {
                use shakmaty::PositionErrorKinds as K;
                let kinds = err.kinds();
                let mut state = Err(err);
                for (kind, repair) in [
                    (
                        K::INVALID_CASTLING_RIGHTS,
                        FenRepair::InvalidCastlingRightsDropped,
                    ),
                    (K::INVALID_EP_SQUARE, FenRepair::InvalidEnPassantDropped),
                    (K::IMPOSSIBLE_CHECK, FenRepair::ImpossibleCheckAccepted),
                    (K::TOO_MUCH_MATERIAL, FenRepair::TooMuchMaterialAccepted),
                ] {
                    let Err(err) = state else { break };
                    if !kinds.contains(kind) {
                        state = Err(err);
                        continue;
                    }
                    state = match repair {
                        FenRepair::InvalidCastlingRightsDropped => {
                            err.ignore_invalid_castling_rights()
                        }
                        FenRepair::InvalidEnPassantDropped => err.ignore_invalid_ep_square(),
                        FenRepair::ImpossibleCheckAccepted => err.ignore_impossible_check(),
                        _ => err.ignore_too_much_material(),
                    };
                    repairs.push(repair);
                }
                state.map_err(|e| FenError::IllegalPosition(e.to_string()))?
            }
        };
        let original_fen = if fen_trimmed.is_empty() {
            None
        } else {
            Some(fen_trimmed.to_owned())
        };
        Ok(StartPosition {
            position,
            original_fen,
            repairs,
        })
    }

    pub fn from_position(position: Chess) -> Self {
        let fen = Fen::from_position(&position, EnPassantMode::Legal).to_string();
        let original_fen = (position != Chess::default()).then_some(fen);
        StartPosition {
            position,
            original_fen,
            repairs: Vec::new(),
        }
    }

    pub fn position(&self) -> &Chess {
        &self.position
    }

    /// FEN as given by the source/user (`None` = standard initial position).
    pub fn original_fen(&self) -> Option<&str> {
        self.original_fen.as_deref()
    }

    /// Normalized FEN of the playable position (fullmove ≥ 1).
    pub fn fen(&self) -> String {
        Fen::from_position(&self.position, EnPassantMode::Legal).to_string()
    }

    pub fn is_standard(&self) -> bool {
        self.position == Chess::default() && self.position.fullmoves().get() == 1
    }

    pub fn repairs(&self) -> &[FenRepair] {
        &self.repairs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullmove_zero_is_tolerated() {
        let s = StartPosition::from_fen("8/3k4/8/3K4/3P4/8/8/8 w - - 0 0").expect("fen");
        assert_eq!(s.repairs(), &[FenRepair::FullmoveZero]);
        assert_eq!(s.position().fullmoves().get(), 1);
        assert_eq!(s.original_fen(), Some("8/3k4/8/3K4/3P4/8/8/8 w - - 0 0"));
    }

    #[test]
    fn bad_castling_is_repaired() {
        let s = StartPosition::from_fen("4k3/8/8/8/8/8/8/4K3 w KQkq - 0 1").expect("fen");
        assert!(
            s.repairs()
                .contains(&FenRepair::InvalidCastlingRightsDropped)
        );
    }

    #[test]
    fn missing_king_is_error() {
        assert!(matches!(
            StartPosition::from_fen("8/8/8/8/8/8/8/4K3 w - - 0 1"),
            Err(FenError::IllegalPosition(_))
        ));
        assert!(matches!(
            StartPosition::from_fen("garbage"),
            Err(FenError::Syntax(_))
        ));
    }
}
