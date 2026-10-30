//! Finding engines without a file dialog: ChessPenguin's Stockfish (bundled in the
//! Flatpak, otherwise downloaded on request, see `engine_download`) and a `stockfish`
//! installed by the system package manager.

use std::path::{Path, PathBuf};

use crate::i18n::tr;
use crate::settings::{EngineConfig, Settings};

#[cfg(windows)]
const STOCKFISH: &str = "stockfish.exe";
#[cfg(not(windows))]
const STOCKFISH: &str = "stockfish";

/// The Stockfish shipped next to the application:
/// `<prefix>/lib/chesspenguin/stockfish` for `<prefix>/bin/chesspenguin` (deb, rpm, Arch,
/// AppImage, Flatpak) or `stockfish/stockfish[.exe]` beside the executable (Windows zip,
/// Linux tarball). Resolved at every start: the AppImage mount point changes between runs.
pub fn bundled_stockfish() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        dir.parent()
            .map(|prefix| prefix.join("lib").join("chesspenguin").join(STOCKFISH)),
        Some(dir.join("stockfish").join(STOCKFISH)),
    ];
    candidates.into_iter().flatten().find(|p| is_executable(p))
}

/// Where an engine starts from: its program on a computer; in a browser, which runs no
/// programs, the address of its Web Worker's script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineLocation {
    #[cfg(not(web))]
    Program(PathBuf),
    #[cfg(web)]
    Worker(String),
}

impl EngineLocation {
    /// Whether the engine is there to start (its program's file; a worker always).
    pub fn present(&self) -> bool {
        match self {
            #[cfg(not(web))]
            EngineLocation::Program(p) => p.is_file(),
            #[cfg(web)]
            EngineLocation::Worker(_) => true,
        }
    }
}

impl std::fmt::Display for EngineLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(not(web))]
            EngineLocation::Program(p) => write!(f, "{}", p.display()),
            #[cfg(web)]
            EngineLocation::Worker(script) => write!(f, "{script}"),
        }
    }
}

/// ChessPenguin's Stockfish: the bundled one, or else the downloaded one.
pub fn own_stockfish() -> Option<EngineLocation> {
    #[cfg(not(web))]
    return bundled_stockfish()
        .map(EngineLocation::Program)
        .or_else(crate::engine_download::installed);
    #[cfg(web)]
    crate::engine_download::installed()
}

/// `stockfish` from `PATH` (for example, the distribution package). On macOS an app
/// started from the Finder does not get the shell's `PATH`: Homebrew's directories are
/// looked at too.
#[cfg(not(web))]
pub fn system_stockfish() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let extra: &[&str] = if cfg!(target_os = "macos") {
        &["/opt/homebrew/bin", "/usr/local/bin"]
    } else {
        &[]
    };
    std::env::split_paths(&path)
        .chain(extra.iter().map(PathBuf::from))
        .map(|dir| dir.join(STOCKFISH))
        .find(|p| is_executable(p))
}

/// A browser has no programs of the system to find.
#[cfg(web)]
pub fn system_stockfish() -> Option<PathBuf> {
    None
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

pub fn builtin_config() -> EngineConfig {
    let mut cfg = EngineConfig::new("Stockfish 19".into(), PathBuf::new());
    cfg.builtin = true;
    cfg
}

/// First start: if no engine is configured, adds the bundled Stockfish (or else the system
/// one) and makes it active, so analysis works right after installation. Runs once.
pub fn first_run_setup(settings: &mut Settings) -> bool {
    if settings.engines_discovered {
        return false;
    }
    settings.engines_discovered = true;
    if settings.engines.is_empty() {
        let found = if own_stockfish().is_some() {
            Some(builtin_config())
        } else {
            system_stockfish().map(|p| EngineConfig::new("Stockfish".into(), p))
        };
        if let Some(cfg) = found {
            settings.engines.push(cfg);
            settings.active_engine = Some(0);
        }
    }
    true
}

/// Engines found on this system that are not in the list yet (offered in the settings).
pub fn suggestions(settings: &Settings) -> Vec<EngineConfig> {
    let mut out = Vec::new();
    if own_stockfish().is_some() && !settings.engines.iter().any(|e| e.builtin) {
        out.push(builtin_config());
    }
    if let Some(p) = system_stockfish() {
        let known = |e: &EngineConfig| {
            !e.builtin && (e.path == p || e.path.canonicalize().ok() == p.canonicalize().ok())
        };
        if !settings.engines.iter().any(known) {
            out.push(EngineConfig::new(tr("Stockfish (system)").into(), p));
        }
    }
    out
}
