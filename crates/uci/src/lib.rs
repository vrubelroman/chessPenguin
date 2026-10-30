//! UCI engine supervisor.
//!
//! The protocol is a state machine without I/O ([`protocol`]); a transport drives
//! it. On a computer the engine is a child process started without a shell:
//! stdout and stderr are read by two dedicated threads, a supervisor thread
//! feeds the machine. The UI talks to it through the non-blocking
//! [`EngineHandle`] and polls [`EngineEvent`]s every frame; the [`Waker`]
//! callback is invoked after each new event. WebAssembly in a browser has no
//! processes, so there is no process transport there.
//!
//! Search boundary: UCI has no request ids. A new `position`/`go` is written
//! only after the previous search ended with `bestmove` and a subsequent
//! `isready`/`readyok` round trip, so output of an old search can never be
//! attributed to a newer [`SearchId`]. Rapid requests are coalesced: only the
//! latest pending request is started, the others are reported as
//! [`EngineEvent::Superseded`]. No dependency on egui or chess-core:
//! positions are strings (FEN / UCI moves).

mod error;
mod events;
pub mod parse;
#[cfg(not(target_family = "wasm"))]
mod process;
// The machine is built where a transport drives it.
#[cfg(not(target_family = "wasm"))]
mod protocol;
mod types;

pub use error::EngineError;
pub use events::{EngineEvent, TerminationReason, Waker};
#[cfg(not(target_family = "wasm"))]
pub use process::{Engine, EngineConfig, EngineHandle};
pub use types::{
    EngineInfo, EngineOption, EnginePosition, EngineScore, OptionKind, PositionBase, Score,
    ScoreBound, SearchId, SearchInfo, SearchLimits, Side, Timeouts, WhiteEval, WhiteScore,
};
