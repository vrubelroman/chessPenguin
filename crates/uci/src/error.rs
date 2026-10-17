use std::path::PathBuf;
use std::process::ExitStatus;

/// Errors returned by [`crate::Engine::spawn`] and the handle methods.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine executable not found: {0}")]
    NotFound(PathBuf),
    #[error("not an executable file: {path}: {reason}")]
    NotExecutable { path: PathBuf, reason: String },
    #[error("failed to start engine: {0}")]
    Spawn(#[source] std::io::Error),
    /// The program answered but did not speak UCI before the `uciok` timeout.
    #[error("program does not look like a UCI engine")]
    NotUci {
        /// First lines of stdout, for diagnostics.
        output: Vec<String>,
    },
    #[error("engine did not answer `{waiting_for}` in time")]
    HandshakeTimeout { waiting_for: &'static str },
    #[error("engine process exited during handshake ({status:?})")]
    ProcessExited {
        status: Option<ExitStatus>,
        stderr: Vec<String>,
    },
    #[error("I/O error talking to engine: {0}")]
    Io(#[source] std::io::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("engine does not declare option {0:?}")]
    UnknownOption(String),
    #[error("invalid value for option {name:?}: {reason}")]
    InvalidOptionValue { name: String, reason: String },
    /// The engine process is no longer running (quit, crashed, killed as hung).
    #[error("engine is not running")]
    NotRunning,
}
