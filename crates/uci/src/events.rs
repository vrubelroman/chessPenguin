//! What a running engine tells the UI, whatever carries it.

use crate::types::{SearchId, SearchInfo};

/// Callback invoked after every new event (from the transport's thread on a
/// computer); the UI passes `ctx.request_repaint()` here.
pub type Waker = Box<dyn Fn() + Send + 'static>;

/// Event delivered to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// `position` + `go` for this id were sent to the engine.
    SearchStarted { search_id: SearchId },
    /// The request was replaced by a newer one (or cancelled by `stop`)
    /// before it was sent to the engine; no events will follow for this id.
    Superseded { search_id: SearchId },
    /// Search output. Only lines with a score or a PV are forwarded.
    Info {
        search_id: SearchId,
        info: SearchInfo,
    },
    /// Final `bestmove` of a started search. Exactly one per `SearchStarted`
    /// unless the engine terminates first.
    BestMove {
        search_id: SearchId,
        best: Option<String>,
        ponder: Option<String>,
    },
    /// `info string`, attributed to the running search if any.
    InfoString {
        search_id: Option<SearchId>,
        text: String,
    },
    /// Malformed or unexpected engine output; the line was skipped.
    Warning(String),
    /// The supervisor has stopped; the handle is no longer usable.
    Terminated(TerminationReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationReason {
    /// `quit()` or drop of the handle.
    Quit {
        status: Option<std::process::ExitStatus>,
    },
    /// The engine exited on its own (crash, end of its output, write failure).
    Exited {
        status: Option<std::process::ExitStatus>,
    },
    /// The engine did not answer `stop` or `isready` in time and was killed.
    Hung { waiting_for: &'static str },
}
