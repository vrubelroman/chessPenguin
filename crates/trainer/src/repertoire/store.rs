use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use chess_core::pgn::store::write_atomic;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::learning::{
    AttemptApplication, LearningData, LearningError, LearningItem, LearningItemKey, TrainingAttempt,
};
use super::learning_analytics::{LearningSnapshot, LocalDatePolicy, same_snapshot_slot};
use super::learning_graph::{LearningGraphError, StatsScope};
use super::learning_session::{LearningSessionError, LearningSessionSnapshot};
use super::model::{LIBRARY_VERSION, Library, RepertoireError};
use super::scope::TrainingScopeKey;
use super::srs::SrsSettings;
use super::stats::{QueueMode, STATS_VERSION, StatsData, TrainingMode};

#[path = "migration.rs"]
mod migration;
pub use migration::MigrationReport;

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
    SessionConflict {
        session_id: String,
        expected: u64,
        actual: u64,
    },
    SessionMismatch {
        attempt_session_id: String,
        session_id: String,
    },
    Repertoire(RepertoireError),
    Learning(LearningError),
    LearningGraph(LearningGraphError),
    Session(LearningSessionError),
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
            Self::SessionConflict {
                session_id,
                expected,
                actual,
            } => write!(
                f,
                "concurrent modification of learning session {session_id}: loaded revision {expected}, disk revision {actual}; reload and retry"
            ),
            Self::SessionMismatch {
                attempt_session_id,
                session_id,
            } => write!(
                f,
                "training attempt belongs to session {attempt_session_id}, not {session_id}"
            ),
            Self::Repertoire(error) => error.fmt(f),
            Self::Learning(error) => write!(f, "cannot apply training attempt: {error}"),
            Self::LearningGraph(error) => write!(f, "cannot capture learning statistics: {error}"),
            Self::Session(error) => write!(f, "invalid learning session: {error}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Repertoire(error) => Some(error),
            Self::Learning(error) => Some(error),
            Self::LearningGraph(error) => Some(error),
            Self::Session(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RepertoireError> for StoreError {
    fn from(error: RepertoireError) -> Self {
        Self::Repertoire(error)
    }
}

impl From<LearningError> for StoreError {
    fn from(error: LearningError) -> Self {
        Self::Learning(error)
    }
}

impl From<LearningGraphError> for StoreError {
    fn from(error: LearningGraphError) -> Self {
        Self::LearningGraph(error)
    }
}

impl From<LearningSessionError> for StoreError {
    fn from(error: LearningSessionError) -> Self {
        Self::Session(error)
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

#[allow(dead_code)]
pub const LEARNING_STORE_VERSION: u32 = 3;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearningSession {
    pub id: String,
    pub mode: QueueMode,
    pub side: TrainingMode,
    pub scope: TrainingScopeKey,
    pub settings: SrsSettings,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LearningStoreData {
    pub version: u32,
    #[serde(default)]
    pub revision: u64,
    pub learning: LearningData,
    #[serde(default)]
    pub sessions: Vec<LearningSession>,
    #[serde(default)]
    pub session_snapshots: Vec<LearningSessionSnapshot>,
    #[serde(default)]
    pub snapshots: Vec<LearningSnapshot>,
    /// Original v2 JSON, including unknown root and nested extension fields.
    #[serde(default)]
    pub legacy_practice: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extensions: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for LearningStoreData {
    fn default() -> Self {
        Self {
            version: LEARNING_STORE_VERSION,
            revision: 0,
            learning: LearningData::default(),
            sessions: Vec::new(),
            session_snapshots: Vec::new(),
            snapshots: Vec::new(),
            legacy_practice: None,
            extensions: std::collections::BTreeMap::new(),
        }
    }
}

impl LearningStoreData {
    pub fn session_snapshot(&self, id: &str) -> Option<&LearningSessionSnapshot> {
        self.session_snapshots
            .iter()
            .find(|snapshot| snapshot.id() == id)
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedAttempt {
    pub application: AttemptApplication,
    pub revision: u64,
    pub learning: LearningData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedLearningReset {
    pub revision: u64,
    pub learning: LearningData,
    pub reset_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedLearningSnapshot {
    pub snapshot: LearningSnapshot,
    pub revision: u64,
    pub recorded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersistedSessionAttempt {
    pub application: AttemptApplication,
    pub revision: u64,
    pub learning: LearningData,
    pub session: LearningSessionSnapshot,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct LearningStore {
    path: PathBuf,
    legacy_path: Option<PathBuf>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
enum LearningSource {
    Missing,
    V2(Vec<u8>),
    V3(serde_json::Value),
    LegacyLines,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct LoadedLearning {
    data: LearningStoreData,
    report: MigrationReport,
    source: LearningSource,
}

#[allow(dead_code)]
impl LearningStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            legacy_path: None,
        }
    }

    /// The legacy line file is read only. All v3 writes target `path`.
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

    pub fn load(&self) -> Result<LearningStoreData, StoreError> {
        self.load_with_report().map(|(data, _)| data)
    }

    pub fn load_with_report(&self) -> Result<(LearningStoreData, MigrationReport), StoreError> {
        let loaded = self.load_latest()?;
        Ok((loaded.data, loaded.report))
    }

    /// Writes an existing v2 or legacy-line source as v3. An already current or missing
    /// store is left untouched, which makes repeated migration idempotent.
    pub fn migrate(&self) -> Result<MigrationReport, StoreError> {
        self.check_legacy_path()?;
        let _lock = lock_store(&self.path)?;
        let mut loaded = self.load_latest()?;
        if matches!(
            loaded.source,
            LearningSource::Missing | LearningSource::V3(_)
        ) {
            return Ok(loaded.report);
        }
        let report = loaded.report;
        self.write_latest(&mut loaded)?;
        Ok(report)
    }

    /// Reloads under the cross-process lock, then persists the event and its learning
    /// effect in one atomic replacement. Duplicate delivery never rewrites the file.
    pub fn apply_attempt(
        &self,
        attempt: TrainingAttempt,
        settings: &SrsSettings,
    ) -> Result<PersistedAttempt, StoreError> {
        self.check_legacy_path()?;
        let _lock = lock_store(&self.path)?;
        let mut loaded = self.load_latest()?;
        let application = loaded.data.learning.apply_attempt(attempt, settings)?;
        if application == AttemptApplication::Duplicate {
            return Ok(PersistedAttempt {
                application,
                revision: loaded.data.revision,
                learning: loaded.data.learning,
            });
        }
        self.write_latest(&mut loaded)?;
        Ok(PersistedAttempt {
            application,
            revision: loaded.data.revision,
            learning: loaded.data.learning,
        })
    }

    pub fn record_snapshot(
        &self,
        library: &Library,
        scope: &StatsScope,
        settings: &SrsSettings,
        timezone: LocalDatePolicy,
    ) -> Result<PersistedLearningSnapshot, StoreError> {
        self.check_legacy_path()?;
        let _lock = lock_store(&self.path)?;
        let mut loaded = self.load_latest()?;
        let snapshot =
            LearningSnapshot::capture(library, &loaded.data.learning, scope, settings, timezone)?;
        if let Some(existing) = loaded
            .data
            .snapshots
            .iter()
            .find(|existing| same_snapshot_slot(existing, &snapshot))
        {
            return Ok(PersistedLearningSnapshot {
                snapshot: existing.clone(),
                revision: loaded.data.revision,
                recorded: false,
            });
        }
        loaded.data.snapshots.push(snapshot.clone());
        self.write_latest(&mut loaded)?;
        Ok(PersistedLearningSnapshot {
            snapshot,
            revision: loaded.data.revision,
            recorded: true,
        })
    }

    pub fn reset_learning<K>(
        &self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<PersistedLearningReset, StoreError>
    where
        K: std::borrow::Borrow<LearningItemKey>,
    {
        self.check_legacy_path()?;
        let keys: BTreeSet<LearningItemKey> =
            keys.into_iter().map(|key| key.borrow().clone()).collect();
        let _lock = lock_store(&self.path)?;
        let mut loaded = self.load_latest()?;
        let mut reset_count = 0;
        for item in &mut loaded.data.learning.items {
            if keys.contains(&item.key) {
                *item = LearningItem::new(item.key.clone());
                reset_count += 1;
            }
        }
        if reset_count > 0 {
            self.write_latest(&mut loaded)?;
        }
        Ok(PersistedLearningReset {
            revision: loaded.data.revision,
            learning: loaded.data.learning,
            reset_count,
        })
    }

    /// Persists a queue-only lifecycle change without replacing learning history.
    pub fn save_session(&self, session: &mut LearningSessionSnapshot) -> Result<u64, StoreError> {
        self.check_legacy_path()?;
        session.validate_configuration()?;
        let _lock = lock_store(&self.path)?;
        let mut loaded = self.load_latest()?;
        check_session_revision(&loaded.data, session)?;
        let previous = session.revision;
        session.revision = session.revision.saturating_add(1);
        upsert_session(&mut loaded.data, session);
        if let Err(error) = self.write_latest(&mut loaded) {
            session.revision = previous;
            return Err(error);
        }
        Ok(loaded.data.revision)
    }

    pub fn reset_review_order(
        &self,
        session: &mut LearningSessionSnapshot,
    ) -> Result<u64, StoreError> {
        let mut reset = session.clone();
        reset.reset_review_order()?;
        let revision = self.save_session(&mut reset)?;
        *session = reset;
        Ok(revision)
    }

    /// Applies one event and its session transition in the same atomic replacement. Both
    /// are based on the latest store contents loaded under the cross-process lock.
    pub fn apply_attempt_and_session(
        &self,
        attempt: TrainingAttempt,
        session: &mut LearningSessionSnapshot,
    ) -> Result<PersistedSessionAttempt, StoreError> {
        self.check_legacy_path()?;
        if attempt.session_id != session.session.id {
            return Err(StoreError::SessionMismatch {
                attempt_session_id: attempt.session_id,
                session_id: session.session.id.clone(),
            });
        }
        let _lock = lock_store(&self.path)?;
        let mut loaded = self.load_latest()?;
        let duplicate = loaded.data.learning.attempts.iter().any(|existing| {
            existing.user_id == attempt.user_id
                && existing.client_attempt_id == attempt.client_attempt_id
        });
        if duplicate {
            let persisted_session = loaded
                .data
                .session_snapshot(&attempt.session_id)
                .cloned()
                .unwrap_or_else(|| session.clone());
            return Ok(PersistedSessionAttempt {
                application: AttemptApplication::Duplicate,
                revision: loaded.data.revision,
                learning: loaded.data.learning,
                session: persisted_session,
            });
        }

        session.validate_configuration()?;
        check_session_revision(&loaded.data, session)?;
        let application = loaded
            .data
            .learning
            .apply_attempt(attempt, &session.session.settings)?;
        let previous = session.revision;
        session.revision = session.revision.saturating_add(1);
        upsert_session(&mut loaded.data, session);
        if let Err(error) = self.write_latest(&mut loaded) {
            session.revision = previous;
            return Err(error);
        }
        Ok(PersistedSessionAttempt {
            application,
            revision: loaded.data.revision,
            learning: loaded.data.learning,
            session: session.clone(),
        })
    }

    fn check_legacy_path(&self) -> Result<(), StoreError> {
        if self.legacy_path.as_ref() == Some(&self.path) {
            Err(StoreError::LegacyPath(self.path.clone()))
        } else {
            Ok(())
        }
    }

    fn load_latest(&self) -> Result<LoadedLearning, StoreError> {
        if let Some(bytes) = read_optional(&self.path)? {
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| json(&self.path, e))?;
            let version = value
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| StoreError::MissingVersion {
                    kind: "repertoire learning",
                    path: self.path.clone(),
                })?;
            let version = u32::try_from(version).unwrap_or(u32::MAX);
            return match version {
                STATS_VERSION => {
                    let legacy_practice = value.clone();
                    let stats: StatsData =
                        serde_json::from_value(value).map_err(|e| json(&self.path, e))?;
                    validate_stats(&self.path, &stats)?;
                    let (learning, report) = migration::migrate_v2(&stats);
                    Ok(LoadedLearning {
                        data: LearningStoreData {
                            revision: stats.revision,
                            learning,
                            legacy_practice: Some(legacy_practice),
                            ..LearningStoreData::default()
                        },
                        report,
                        source: LearningSource::V2(bytes),
                    })
                }
                LEARNING_STORE_VERSION => {
                    let schema_error = |message| {
                        json(
                            &self.path,
                            <serde_json::Error as serde::de::Error>::custom(message),
                        )
                    };
                    let learning = value
                        .get("learning")
                        .and_then(serde_json::Value::as_object)
                        .ok_or_else(|| schema_error("v3 learning must be an object"))?;
                    for field in ["items", "attempts"] {
                        if learning.get(field).is_some_and(|value| !value.is_array()) {
                            return Err(schema_error("v3 learning items/attempts must be arrays"));
                        }
                    }
                    if value
                        .get("legacy_practice")
                        .is_some_and(|value| !value.is_null() && !value.is_object())
                    {
                        return Err(schema_error("v3 legacy_practice must be an object or null"));
                    }
                    let data: LearningStoreData =
                        serde_json::from_value(value.clone()).map_err(|e| json(&self.path, e))?;
                    Ok(LoadedLearning {
                        data,
                        report: MigrationReport::default(),
                        source: LearningSource::V3(value),
                    })
                }
                _ => Err(StoreError::UnsupportedVersion {
                    kind: "repertoire learning",
                    version,
                    path: self.path.clone(),
                }),
            };
        }

        let Some(legacy_path) = self.legacy_path.as_deref() else {
            return Ok(LoadedLearning {
                data: LearningStoreData::default(),
                report: MigrationReport::default(),
                source: LearningSource::Missing,
            });
        };
        let Some(bytes) = read_optional(legacy_path)? else {
            return Ok(LoadedLearning {
                data: LearningStoreData::default(),
                report: MigrationReport::default(),
                source: LearningSource::Missing,
            });
        };
        let legacy_lines: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|e| json(legacy_path, e))?;
        let mut legacy = StatsData::default();
        legacy.legacy_lines = legacy_lines;
        let legacy_practice = serde_json::to_value(legacy).map_err(|e| json(legacy_path, e))?;
        Ok(LoadedLearning {
            data: LearningStoreData {
                legacy_practice: Some(legacy_practice),
                ..LearningStoreData::default()
            },
            report: MigrationReport::default(),
            source: LearningSource::LegacyLines,
        })
    }

    fn write_latest(&self, loaded: &mut LoadedLearning) -> Result<(), StoreError> {
        if let LearningSource::V2(bytes) = &loaded.source {
            self.backup_v2_once(bytes)?;
        }
        loaded.data.version = LEARNING_STORE_VERSION;
        loaded.data.revision = loaded.data.revision.saturating_add(1);
        let current = serde_json::to_value(&loaded.data).map_err(|e| json(&self.path, e))?;
        let value = match &loaded.source {
            LearningSource::V3(raw) => preserve_extensions(raw, current),
            _ => current,
        };
        let bytes = serde_json::to_vec_pretty(&value).map_err(|e| json(&self.path, e))?;
        write_atomic(&self.path, &bytes).map_err(|e| io(&self.path, e))
    }

    fn backup_v2_once(&self, bytes: &[u8]) -> Result<(), StoreError> {
        let backup = learning_backup_path(&self.path);
        match std::fs::metadata(&backup) {
            Ok(metadata) if metadata.is_file() => return Ok(()),
            Ok(_) => {
                return Err(io(
                    &backup,
                    io::Error::new(io::ErrorKind::AlreadyExists, "backup path is not a file"),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(&backup, error)),
        }
        write_atomic(&backup, bytes).map_err(|e| io(&backup, e))
    }
}

/// Known values come from current state; unmodelled object fields remain from raw JSON.
fn preserve_extensions(raw: &serde_json::Value, current: serde_json::Value) -> serde_json::Value {
    if current.as_object().is_some_and(|object| object.len() == 3)
        && learning_item_key_fields(&current).is_some()
    {
        return current;
    }
    match (raw, current) {
        (serde_json::Value::Object(raw), serde_json::Value::Object(current)) => {
            let mut merged = raw.clone();
            for (field, value) in current {
                let value = match raw.get(&field) {
                    Some(raw) => preserve_extensions(raw, value),
                    None => value,
                };
                merged.insert(field, value);
            }
            serde_json::Value::Object(merged)
        }
        (serde_json::Value::Array(raw), serde_json::Value::Array(current)) => {
            let current_len = current.len();
            let merged = current
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    let previous = match stable_json_identity(&value) {
                        Some(identity) => raw.iter().find(|candidate| {
                            stable_json_identity(candidate)
                                .is_some_and(|candidate| candidate == identity)
                        }),
                        None if raw.len() == current_len => raw.get(index),
                        None => None,
                    };
                    match previous {
                        Some(raw) => preserve_extensions(raw, value),
                        None => value,
                    }
                })
                .collect();
            serde_json::Value::Array(merged)
        }
        (_, current) => current,
    }
}

fn stable_json_identity(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    if let (Some(user_id), Some(client_attempt_id)) = (
        object.get("user_id").and_then(serde_json::Value::as_str),
        object
            .get("client_attempt_id")
            .and_then(serde_json::Value::as_str),
    ) {
        return Some(json_identity(&["attempt", user_id, client_attempt_id]));
    }
    if let Some((user_id, position_fen4, expected_uci)) = learning_item_key_fields(value) {
        return Some(json_identity(&[
            "key",
            user_id,
            position_fen4,
            expected_uci,
        ]));
    }
    if let Some(key) = object.get("key")
        && let Some((user_id, position_fen4, expected_uci)) = learning_item_key_fields(key)
    {
        let repertoire_id = object
            .get("repertoire_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return Some(json_identity(&[
            "nested-key",
            user_id,
            position_fen4,
            expected_uci,
            repertoire_id,
        ]));
    }
    if let (Some(date), Some(scope)) = (
        object.get("date").and_then(serde_json::Value::as_str),
        object.get("scope"),
    ) {
        let mut scope = scope.clone();
        if let Some(object) = scope.as_object_mut() {
            object.retain(|field, _| {
                matches!(
                    field.as_str(),
                    "repertoire_ids"
                        | "side"
                        | "max_depth"
                        | "only_active"
                        | "branch_root"
                        | "as_of"
                        | "learning_priority"
                )
            });
        }
        let scope = scope.to_string();
        return Some(json_identity(&["learning-snapshot", date, &scope]));
    }
    if let Some(id) = object
        .get("session")
        .and_then(|session| session.get("id"))
        .and_then(serde_json::Value::as_str)
    {
        return Some(json_identity(&["snapshot", id]));
    }
    object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(|id| json_identity(&["id", id]))
}

fn learning_item_key_fields(value: &serde_json::Value) -> Option<(&str, &str, &str)> {
    let object = value.as_object()?;
    if object.contains_key("client_attempt_id") {
        return None;
    }
    Some((
        object.get("user_id")?.as_str()?,
        object.get("position_fen4")?.as_str()?,
        object.get("expected_uci")?.as_str()?,
    ))
}

fn json_identity(parts: &[&str]) -> String {
    serde_json::Value::Array(
        parts
            .iter()
            .map(|part| serde_json::Value::String((*part).to_owned()))
            .collect(),
    )
    .to_string()
}

fn check_session_revision(
    data: &LearningStoreData,
    session: &LearningSessionSnapshot,
) -> Result<(), StoreError> {
    let actual = data
        .session_snapshot(&session.session.id)
        .map_or(0, |persisted| persisted.revision);
    if session.revision == actual {
        Ok(())
    } else {
        Err(StoreError::SessionConflict {
            session_id: session.session.id.clone(),
            expected: session.revision,
            actual,
        })
    }
}

fn upsert_session(data: &mut LearningStoreData, session: &LearningSessionSnapshot) {
    if let Some(existing) = data
        .sessions
        .iter_mut()
        .find(|existing| existing.id == session.session.id)
    {
        *existing = session.session.clone();
    } else {
        data.sessions.push(session.session.clone());
    }
    if let Some(existing) = data
        .session_snapshots
        .iter_mut()
        .find(|existing| existing.session.id == session.session.id)
    {
        *existing = session.clone();
    } else {
        data.session_snapshots.push(session.clone());
    }
}

fn learning_backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".before-learning-v3");
    path.with_file_name(name)
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

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;
