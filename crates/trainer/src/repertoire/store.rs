use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use chess_core::pgn::store::write_atomic;
use serde::de::DeserializeOwned;

use super::model::{LIBRARY_VERSION, Library, RepertoireError};
use super::stats::{STATS_VERSION, StatsData};

#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsupportedVersion {
        kind: &'static str,
        version: u32,
        path: PathBuf,
    },
    MissingVersion {
        kind: &'static str,
        path: PathBuf,
    },
    DuplicateStats(PathBuf),
    Conflict {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    LegacyPath(PathBuf),
    Repertoire(RepertoireError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "I/O error at {}: {source}", path.display()),
            Self::Json { path, source } => {
                write!(f, "invalid JSON at {}: {source}", path.display())
            }
            Self::UnsupportedVersion {
                kind,
                version,
                path,
            } => write!(
                f,
                "unsupported {kind} schema version {version} at {}",
                path.display()
            ),
            Self::MissingVersion { kind, path } => {
                write!(f, "{kind} at {} has no schema version", path.display())
            }
            Self::DuplicateStats(path) => {
                write!(f, "duplicate training keys in {}", path.display())
            }
            Self::Conflict {
                path,
                expected,
                actual,
            } => write!(
                f,
                "concurrent modification of {}: loaded revision {expected}, disk revision {actual}; reload and retry",
                path.display()
            ),
            Self::LegacyPath(path) => write!(
                f,
                "legacy training file must be different from the new statistics file: {}",
                path.display()
            ),
            Self::Repertoire(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Repertoire(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RepertoireError> for StoreError {
    fn from(error: RepertoireError) -> Self {
        Self::Repertoire(error)
    }
}

fn io(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

fn json(path: &Path, source: serde_json::Error) -> StoreError {
    StoreError::Json {
        path: path.to_owned(),
        source,
    }
}

// Lock a stable sidecar: the data file itself is replaced by atomic rename. Keeping
// the lock file avoids unlink/open races and the OS releases its lock after a crash.
fn lock_store(path: &Path) -> Result<std::fs::File, StoreError> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let mut lock_name = target.file_name().unwrap_or_default().to_os_string();
    lock_name.push(".lock");
    let lock_path = target.with_file_name(lock_name);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&lock_path)
        .map_err(|e| io(&lock_path, e))?;
    lock.lock().map_err(|e| io(&lock_path, e))?;
    Ok(lock)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io(path, error)),
    }
}

fn decode_versioned<T: DeserializeOwned>(
    path: &Path,
    bytes: &[u8],
    kind: &'static str,
    supported: u32,
) -> Result<T, StoreError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| json(path, e))?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| StoreError::MissingVersion {
            kind,
            path: path.to_owned(),
        })?;
    let version = u32::try_from(version).unwrap_or(u32::MAX);
    if version != supported {
        return Err(StoreError::UnsupportedVersion {
            kind,
            version,
            path: path.to_owned(),
        });
    }
    serde_json::from_value(value).map_err(|e| json(path, e))
}

#[derive(Clone, Debug)]
pub struct LibraryStore {
    path: PathBuf,
}

impl LibraryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A missing new library is empty. Existing unreadable or unsupported data is an
    /// error, never silently replaced by an empty library.
    pub fn load(&self) -> Result<Library, StoreError> {
        let Some(bytes) = read_optional(&self.path)? else {
            return Ok(Library::default());
        };
        let library: Library =
            decode_versioned(&self.path, &bytes, "repertoire library", LIBRARY_VERSION)?;
        library.validate()?;
        Ok(library)
    }

    /// Optimistic concurrency: a stale in-memory snapshot is rejected rather than
    /// overwriting another process's additions. The caller reloads and reapplies its edit.
    pub fn save(&self, library: &mut Library) -> Result<(), StoreError> {
        library.validate()?;
        let _lock = lock_store(&self.path)?;
        let current = self.load()?;
        if current.revision != library.revision {
            return Err(StoreError::Conflict {
                path: self.path.clone(),
                expected: library.revision,
                actual: current.revision,
            });
        }
        let previous = library.revision;
        library.version = LIBRARY_VERSION;
        library.revision = current.revision.saturating_add(1);
        let bytes = serde_json::to_vec_pretty(library).map_err(|e| json(&self.path, e))?;
        if let Err(error) = write_atomic(&self.path, &bytes) {
            library.revision = previous;
            return Err(io(&self.path, error));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct StatsStore {
    path: PathBuf,
    legacy_path: Option<PathBuf>,
}

impl StatsStore {
    /// A store for the new versioned format. It does not implicitly read or replace the
    /// legacy global `training.json`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            legacy_path: None,
        }
    }

    /// A new file plus an optional legacy source. Saving always writes `path`; the legacy
    /// source is copied once to `.before-repertoire-v2` and is never renamed or overwritten.
    pub fn with_legacy(path: impl Into<PathBuf>, legacy_path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            legacy_path: Some(legacy_path.into()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn legacy_path(&self) -> Option<&Path> {
        self.legacy_path.as_deref()
    }

    pub fn load(&self) -> Result<StatsData, StoreError> {
        if let Some(bytes) = read_optional(&self.path)? {
            let mut stats: StatsData =
                decode_versioned(&self.path, &bytes, "repertoire statistics", STATS_VERSION)?;
            validate_stats(&self.path, &stats)?;
            stats.migrate_position_stats();
            return Ok(stats);
        }
        let Some(legacy_path) = self.legacy_path.as_deref() else {
            return Ok(StatsData::default());
        };
        let Some(bytes) = read_optional(legacy_path)? else {
            return Ok(StatsData::default());
        };
        let legacy_lines: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|e| json(legacy_path, e))?;
        let mut stats = StatsData::default();
        stats.legacy_lines = legacy_lines;
        Ok(stats)
    }

    pub fn save(&self, stats: &mut StatsData) -> Result<(), StoreError> {
        if self.legacy_path.as_ref() == Some(&self.path) {
            return Err(StoreError::LegacyPath(self.path.clone()));
        }
        validate_stats(&self.path, stats)?;
        let _lock = lock_store(&self.path)?;
        let current = self.load()?;
        if current.revision != stats.revision {
            return Err(StoreError::Conflict {
                path: self.path.clone(),
                expected: stats.revision,
                actual: current.revision,
            });
        }
        self.backup_legacy_once()?;
        let previous = stats.revision;
        stats.version = STATS_VERSION;
        stats.revision = current.revision.saturating_add(1);
        stats.normalize();
        let bytes = serde_json::to_vec_pretty(stats).map_err(|e| json(&self.path, e))?;
        if let Err(error) = write_atomic(&self.path, &bytes) {
            stats.revision = previous;
            return Err(io(&self.path, error));
        }
        Ok(())
    }

    fn backup_legacy_once(&self) -> Result<(), StoreError> {
        let Some(legacy) = self.legacy_path.as_deref() else {
            return Ok(());
        };
        let Some(bytes) = read_optional(legacy)? else {
            return Ok(());
        };
        let backup = legacy_backup_path(legacy);
        if backup.exists() {
            return Ok(());
        }
        write_atomic(&backup, &bytes).map_err(|e| io(&backup, e))
    }
}

fn legacy_backup_path(legacy: &Path) -> PathBuf {
    let mut name = legacy.file_name().unwrap_or_default().to_os_string();
    name.push(".before-repertoire-v2");
    legacy.with_file_name(name)
}

fn validate_stats(path: &Path, stats: &StatsData) -> Result<(), StoreError> {
    let mut moves = BTreeSet::new();
    let mut runs = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let valid = stats
        .move_stats
        .iter()
        .all(|entry| moves.insert(&entry.key))
        && stats
            .runs
            .iter()
            .all(|entry| runs.insert((&entry.repertoire_id, &entry.mode)))
        && stats
            .path_stats
            .iter()
            .all(|entry| paths.insert(&entry.key));
    if valid {
        Ok(())
    } else {
        Err(StoreError::DuplicateStats(path.to_owned()))
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn child_exits_while_holding_store_lock() {
        let Some(path) = std::env::var_os("CP_TRAINER_LOCK_EXIT_TEST") else {
            return;
        };
        let _lock = lock_store(Path::new(&path)).unwrap();
        // process::exit skips Rust destructors, as does abrupt process termination.
        std::process::exit(0);
    }

    #[test]
    fn store_lock_is_released_on_drop_and_process_exit() {
        let path =
            std::env::temp_dir().join(format!("cp-store-lock-exit-{}.json", std::process::id()));
        let lock_path = path.with_file_name(format!(
            "{}.lock",
            path.file_name().unwrap().to_string_lossy()
        ));
        drop(lock_store(&path).unwrap());
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        file.try_lock().unwrap();
        file.unlock().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("repertoire::store::lock_tests::child_exits_while_holding_store_lock")
            .env("CP_TRAINER_LOCK_EXIT_TEST", &path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(lock_path.exists());
        file.try_lock().unwrap();
        file.unlock().unwrap();
        std::fs::remove_file(lock_path).unwrap();
    }
}
