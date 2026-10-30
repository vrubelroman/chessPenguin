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

    /// Whether the engine can be started: ChessPenguin's Stockfish is there (in a
    /// browser: downloaded into it), or the file of another engine.
    pub fn present(&self) -> bool {
        if self.builtin {
            crate::engines::own_stockfish().is_some()
        } else {
            self.path.is_file()
        }
    }
}

/// Piece animation, with Lichess's durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Animation {
    Off,
    Fast,
    #[default]
    Normal,
    Slow,
}

impl Animation {
    pub const ALL: [Animation; 4] = [Self::Off, Self::Fast, Self::Normal, Self::Slow];

    pub fn seconds(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Fast => 0.12,
            Self::Normal => 0.25,
            Self::Slow => 0.5,
        }
    }

    pub fn label(self) -> &'static str {
        crate::i18n::tr(match self {
            Self::Off => "Off",
            Self::Fast => "Fast",
            Self::Normal => "Normal",
            Self::Slow => "Slow",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub piece_set: PieceSet,
    pub board_theme: BoardThemeId,
    pub show_coords: bool,
    /// How fast moves slide on the board.
    pub animation: Animation,
    /// A pawn reaching the last rank becomes a queen without the choice of a piece.
    pub auto_queen: bool,
    /// How moves are shown: Latin letters, figurines or the interface language's letters.
    pub move_notation: crate::san::MoveNotation,
    /// Translating game comments (docs/design/translation.md).
    pub translation: crate::translate::TranslationSettings,
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
    /// The first-start engine discovery has run (see `engines::first_run_setup`).
    pub engines_discovered: bool,
    /// The sample databases were put on the shelf (see `examples::setup`).
    pub examples_added: bool,
    /// Ask GitHub for a newer release at start (Settings → About).
    pub check_updates: bool,
    /// A version the user chose to skip (its button is not shown).
    pub skipped_version: Option<String>,
    /// The version that ran last (to say "updated to …" once).
    pub last_version: Option<String>,
    /// A random id of this installation, sent with the update check (the site counts
    /// installations that run; nothing else identifies the user).
    pub install_id: String,
    /// The database of the reference tab (`None`: the largest one on the shelf).
    pub reference_db: Option<PathBuf>,
    /// The user's name: the annotator of games they add (own-databases.md §7).
    pub your_name: String,
    /// Where new databases are made (`None`: Documents/ChessPenguin).
    pub new_database_dir: Option<PathBuf>,
    /// The database a game was last added to (preselected in the chooser).
    pub last_target: Option<PathBuf>,
    /// The user's rating (optional): the strength of Maia is chosen relative to it.
    pub rating: Option<u32>,
    /// The last choices of the play dialog.
    pub play: crate::play::PlayPrefs,
    /// Games against the computer go by themselves into the player's database.
    pub record_games: bool,
    /// Who played on this computer, the last first (`player_games.rs`).
    pub players: Vec<crate::player_games::Player>,
    /// Preparing against an opponent (docs/design/opponent-prep.md).
    pub prep: PrepSettings,
}

/// Options of the opponent preparation; its engine search is the game analysis's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrepSettings {
    /// The book (`None`: the largest database on the shelf; `Some("")`: no book).
    pub book: Option<PathBuf>,
    /// Plies of the opening the report looks at.
    pub plies: usize,
    /// Moves with fewer games fold into «other moves».
    pub branches_from: u32,
    /// The colour was chosen in the dialog once: the button starts the engine at once.
    pub chosen: bool,
    /// Inaccuracies (?!) are shown with the mistakes and blunders.
    pub inaccuracies: bool,
    /// The order of each table of the screen.
    pub sorts: crate::prep_ui::PrepSorts,
}

impl Default for PrepSettings {
    fn default() -> Self {
        Self {
            book: None,
            plies: 20,
            branches_from: 3,
            chosen: false,
            inaccuracies: true,
            sorts: Default::default(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            piece_set: PieceSet::default(),
            board_theme: BoardThemeId::default(),
            show_coords: true,
            animation: Animation::default(),
            auto_queen: false,
            move_notation: Default::default(),
            translation: Default::default(),
            sound_enabled: true,
            sound_set: SoundSet::default(),
            volume: 0.7,
            dark_mode: true,
            language: Lang::default(),
            engines: Vec::new(),
            active_engine: None,
            show_best_arrow: true,
            databases: Vec::new(),
            engines_discovered: false,
            examples_added: false,
            check_updates: true,
            skipped_version: None,
            last_version: None,
            install_id: String::new(),
            reference_db: None,
            your_name: String::new(),
            new_database_dir: None,
            last_target: None,
            record_games: true,
            players: Vec::new(),
            rating: None,
            play: Default::default(),
            prep: PrepSettings::default(),
        }
    }
}

impl Settings {
    pub fn config_dir() -> Option<PathBuf> {
        crate::platform::config_dir()
    }

    /// `~/.cache/chesspenguin` (derived data that can be deleted at any time).
    pub fn cache_dir() -> Option<PathBuf> {
        crate::platform::cache_dir()
    }

    fn file() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::file() else {
            return Self::first_start();
        };
        migrate_legacy_config(&path);
        match crate::platform::fs::read(&path) {
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
    if crate::platform::fs::exists(new_file) {
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
    crate::platform::fs::write_atomic(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings of 1.x kept the engine options of the game analysis and the
    /// quality of the preparation: they are read without them now.
    #[test]
    fn settings_with_the_old_analysis_options_are_read() {
        let old = r#"{
            "language": "Ru",
            "review": {"quality": 2, "threads": 12, "hash_mb": 1024, "chosen": true},
            "prep": {"quality": 1, "plies": 16, "chosen": true}
        }"#;
        let s: Settings = serde_json::from_str(old).expect("read");
        assert_eq!(s.language, Lang::Ru);
        assert_eq!(s.prep.plies, 16);
        assert!(s.prep.chosen);
        let json = serde_json::to_string(&s).expect("written");
        assert!(!json.contains("\"review\"") && !json.contains("quality"));
    }
}
