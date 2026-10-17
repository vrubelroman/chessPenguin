//! UCI engine process supervisor.
//!
//! The engine runs as a separate child process started without a shell.
//! stdout and stderr are read by two dedicated threads; a supervisor thread
//! owns the protocol state machine. The UI talks to it through the
//! non-blocking [`EngineHandle`] and polls [`EngineEvent`]s every frame; the
//! [`Waker`] callback is invoked after each new event.
//!
//! Search boundary: UCI has no request ids. A new `position`/`go` is written
//! only after the previous search ended with `bestmove` and a subsequent
//! `isready`/`readyok` round trip, so output of an old search can never be
//! attributed to a newer [`SearchId`]. Rapid requests are coalesced: only the
//! latest pending request is started, the others are reported as
//! [`EngineEvent::Superseded`]. No dependency on egui or chess-core:
//! positions are strings (FEN / UCI moves).

mod engine;
mod error;
pub mod parse;
mod types;

pub use engine::{Engine, EngineConfig, EngineEvent, EngineHandle, TerminationReason, Waker};
pub use error::EngineError;
pub use types::{
    EngineInfo, EngineOption, EnginePosition, EngineScore, OptionKind, PositionBase, Score,
    ScoreBound, SearchId, SearchInfo, SearchLimits, Side, Timeouts, WhiteEval, WhiteScore,
};
