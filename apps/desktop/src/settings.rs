//! Persistent user settings (JSON in the XDG config dir), written atomically.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::assets::{BoardThemeId, PieceSet, SoundSet};
use crate::i18n::Lang;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EngineConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default = "default_threads")]
    pub threads: u32,
    #[serde(default = "default_hash")]
    pub hash_mb: u32,
    #[serde(default = "default_multipv")]
    pub multipv: u32,
    /// Search time per position in seconds; `None` = infinite.
    #[serde(default)]
    pub search_secs: Option<u32>,
    /// ChessPenguin's Stockfish (bundled in the Flatpak, otherwise downloaded on
    /// request); `path` is unused and resolved at start.
    #[serde(default)]
    pub builtin: bool,
}

fn default_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() as u32 / 2).max(1))
        .unwrap_or(1)
}
fn default_hash() -> u32 {
    256
}
fn default_multipv() -> u32 {
    3
}

impl EngineConfig {
    pub fn new(name: String, path: PathBuf) -> Self {
        Self {
            name,
            path,
            threads: default_threads(),
            hash_mb: default_hash(),
            multipv: default_multipv(),
            search_secs: None,
            builtin: false,
        }
    }

    /// Executable to start; `None` if ChessPenguin's Stockfish is neither bundled nor
    /// downloaded yet (it is offered for download then).
    pub fn executable(&self) -> Option<PathBuf> {
        if self.builtin {
            crate::engines::own_stockfish()
        } else {
            Some(self.path.clone())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub piece_set: PieceSet,
    pub board_theme: BoardThemeId,
    pub show_coords: bool,
    pub sound_enabled: bool,
    pub sound_set: SoundSet,
    pub volume: f32,
    pub dark_mode: bool,
    pub language: Lang,
    pub engines: Vec<EngineConfig>,
    pub active_engine: Option<usize>,
    pub show_best_arrow: bool,
    /// Recently opened PGN files, most recent first.
    pub databases: Vec<PathBuf>,
    /// Whole-game analysis options.
    pub review: ReviewSettings,
    /// The first-start engine discovery has run (see `engines::first_run_setup`).
    pub engines_discovered: bool,
    /// The sample databases were put on the shelf (see `examples::first_run_setup`).
    pub examples_added: bool,
    /// The database of the reference tab (`None`: the largest one on the shelf).
    pub reference_db: Option<PathBuf>,
}

/// Options of the whole-game analysis (the engine is the active one).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewSettings {
    /// Index into [`REVIEW_QUALITY`].
    pub quality: usize,
    pub threads: u32,
    pub hash_mb: u32,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            quality: 1,
            threads: default_threads(),
            hash_mb: default_hash(),
        }
    }
}

/// Search effort per position: (label, nodes).
pub const REVIEW_QUALITY: [(&str, u64); 3] = [
    ("Fast", 250_000),
    ("Normal", 1_000_000),
    ("Deep", 3_000_000),
];

impl ReviewSettings {
    pub fn nodes(&self) -> u64 {
        REVIEW_QUALITY[self.quality.min(REVIEW_QUALITY.len() - 1)].1
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            piece_set: PieceSet::default(),
            board_theme: BoardThemeId::default(),
            show_coords: true,
            sound_enabled: true,
            sound_set: SoundSet::default(),
            volume: 0.7,
            dark_mode: true,
            language: Lang::default(),
            engines: Vec::new(),
            active_engine: None,
            show_best_arrow: true,
            databases: Vec::new(),
            review: ReviewSettings::default(),
            engines_discovered: false,
            examples_added: false,
            reference_db: None,
        }
    }
}

impl Settings {
    pub fn config_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
            .map(|d| d.config_dir().to_path_buf())
    }

    /// `~/.cache/chesspenguin` (derived data that can be deleted at any time).
    pub fn cache_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
            .map(|d| d.cache_dir().to_path_buf())
    }

    fn file() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::file() else {
            return Self::first_start();
        };
        migrate_legacy_config(&path);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!(
                    "settings: cannot parse {}: {e}; using defaults",
                    path.display()
                );
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::first_start(),
            Err(_) => Self::default(),
        }
    }

    /// Settings of the very first start: the defaults in the system's language (if it
    /// is one of ours, otherwise English).
    fn first_start() -> Self {
        Self {
            language: Lang::system(),
            ..Self::default()
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::file() else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(&path, &json)
    }

    pub fn active_engine(&self) -> Option<&EngineConfig> {
        self.active_engine.and_then(|i| self.engines.get(i))
    }

    pub fn touch_database(&mut self, path: &Path) {
        self.databases.retain(|p| p != path);
        self.databases.insert(0, path.to_path_buf());
    }
}

/// Copies settings of the pre-rename build (`chessbase-linux`) once, if the new file is absent.
fn migrate_legacy_config(new_file: &Path) {
    if new_file.exists() {
        return;
    }
    let legacy = directories::ProjectDirs::from("org", "chessbase-linux", "chessbase-linux")
        .map(|d| d.config_dir().join("settings.json"));
    if let Some(old) = legacy.filter(|p| p.is_file())
        && let Ok(bytes) = std::fs::read(&old)
    {
        let _ = write_atomic(new_file, &bytes);
    }
}

/// Writes via a temp file in the same directory and renames over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
