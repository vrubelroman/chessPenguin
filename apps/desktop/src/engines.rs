//! Finding engines without a file dialog: the Stockfish shipped with the packages and a
//! `stockfish` installed by the system package manager.

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

/// `stockfish` from `PATH` (for example, the distribution package).
pub fn system_stockfish() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(STOCKFISH))
        .find(|p| is_executable(p))
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
    let mut cfg = EngineConfig::new(tr("Stockfish (built-in)").into(), PathBuf::new());
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
        let found = if bundled_stockfish().is_some() {
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
    if bundled_stockfish().is_some() && !settings.engines.iter().any(|e| e.builtin) {
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
