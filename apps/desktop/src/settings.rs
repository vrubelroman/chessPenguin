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
    /// Whole-game analysis options.
    pub review: ReviewSettings,
    /// The first-start engine discovery has run (see `engines::first_run_setup`).
    pub engines_discovered: bool,
    /// The sample databases were put on the shelf (see `examples::first_run_setup`).
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

/// Options of the opponent preparation; threads and memory are the game analysis's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrepSettings {
    /// Index into [`REVIEW_QUALITY`]; «Fast» is enough for opening errors.
    pub quality: usize,
    /// The book (`None`: the largest database on the shelf; `Some("")`: no book).
    pub book: Option<PathBuf>,
    /// Plies of the opening the report looks at.
    pub plies: usize,
    /// Moves with fewer games fold into «other moves».
    pub branches_from: u32,
    /// The engine was started from the dialog once: the button starts it at once.
    pub chosen: bool,
}

impl Default for PrepSettings {
    fn default() -> Self {
        Self {
            quality: 0,
            book: None,
            plies: 20,
            branches_from: 3,
            chosen: false,
        }
    }
}

impl PrepSettings {
    pub fn nodes(&self) -> u64 {
        REVIEW_QUALITY[self.quality.min(REVIEW_QUALITY.len() - 1)].1
    }
}

/// Options of the whole-game analysis (the engine is the active one).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewSettings {
    /// Index into [`REVIEW_QUALITY`].
    pub quality: usize,
    pub threads: u32,
    pub hash_mb: u32,
    /// An analysis was started from the dialog once: «Analyse» starts the next one with
    /// these options at once (the dialog stays for «Analyse again…»).
    pub chosen: bool,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            quality: 1,
            threads: default_threads(),
            hash_mb: default_hash(),
            chosen: false,
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
    /// Options other than the defaults were chosen in the dialog (before `chosen` was
    /// kept, 1.5): they count as chosen.
    pub fn note_earlier_choice(&mut self) {
        let default = ReviewSettings::default();
        if (self.quality, self.threads, self.hash_mb)
            != (default.quality, default.threads, default.hash_mb)
        {
            self.chosen = true;
        }
    }

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
            review: ReviewSettings::default(),
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
            Ok(bytes) => {
                let mut s: Settings = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                    eprintln!(
                        "settings: cannot parse {}: {e}; using defaults",
                        path.display()
                    );
                    Self::default()
                });
                s.review.note_earlier_choice();
                s
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_chosen_before_count_as_chosen() {
        let mut r = ReviewSettings::default();
        r.note_earlier_choice();
        assert!(!r.chosen, "the defaults: never chosen");
        r.quality = 2;
        r.note_earlier_choice();
        assert!(r.chosen);
    }
}
