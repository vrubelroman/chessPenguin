use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use super::*;
use crate::repertoire::learning::{LearningItemKey, TrainingEventType};
use crate::repertoire::learning_graph::StatsScope;
use crate::repertoire::model::{LearningSide, RepertoireId};
use crate::repertoire::stats::{MoveKey, MoveStats};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cp-learning-store-{}-{}-{name}",
            std::process::id(),
            sequence
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn legacy_move(repertoire: &str, mode: TrainingMode, expected_uci: &str) -> MoveStats {
    MoveStats::new(MoveKey {
        repertoire_id: RepertoireId::legacy(repertoire),
        mode,
        position_fen4: "fen-shared".into(),
        expected_uci: expected_uci.into(),
    })
}

fn attempt(id: &str, happened_at: i64) -> TrainingAttempt {
    TrainingAttempt {
        user_id: "local".into(),
        position_fen4: "fen-shared".into(),
        client_attempt_id: id.into(),
        session_id: "session-1".into(),
        event_type: TrainingEventType::CorrectIndependent,
        expected_uci: "e2e4".into(),
        actual_uci: Some("e2e4".into()),
        response_time_ms: Some(250),
        happened_at,
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn extension_marker() -> serde_json::Value {
    serde_json::json!({"preserved": [1, {"nested": true}]})
}

const V3_EXTENSION_POINTERS: &[&str] = &[
    "/future_top_level_field",
    "/learning/future_learning_field",
    "/learning/items/0/future_item_field",
    "/learning/attempts/0/future_attempt_field",
    "/sessions/0/future_session_field",
    "/session_snapshots/0/future_snapshot_field",
    "/session_snapshots/0/session/future_nested_session_field",
    "/session_snapshots/0/session/settings/future_settings_field",
    "/session_snapshots/0/stats_scope/future_scope_field",
    "/session_snapshots/0/queue/0/future_target_field",
];

fn assert_v3_extensions(value: &serde_json::Value) {
    for pointer in V3_EXTENSION_POINTERS {
        assert_eq!(
            value.pointer(pointer),
            Some(&extension_marker()),
            "{pointer}"
        );
    }
}

fn extension_fixture() -> (LearningStoreData, Library) {
    let mut library = Library::default();
    let repertoire_id = library
        .create("Test", &chess_core::parse_str("1. e4 *").game)
        .unwrap();
    let scope = StatsScope::for_repertoire(repertoire_id, LearningSide::White, 0);
    let key = crate::repertoire::learning_graph::active_learning_memberships(&library, &scope)
        .unwrap()
        .into_iter()
        .find(|membership| membership.key.expected_uci == "e2e4")
        .unwrap()
        .key;
    let mut learning = LearningData::default();
    let item = learning.ensure_item(key);
    item.introduced = true;
    item.next_review_at = Some(0);
    learning.attempts.push(attempt("existing", 0));
    let snapshot = LearningSessionSnapshot::new(
        "session-1",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(snapshot.queue.len(), 1);
    let data = LearningStoreData {
        learning,
        sessions: vec![snapshot.session.clone()],
        session_snapshots: vec![snapshot],
        ..LearningStoreData::default()
    };
    (data, library)
}

fn inject_v3_extensions(value: &mut serde_json::Value) {
    value["future_top_level_field"] = extension_marker();
    value["learning"]["future_learning_field"] = extension_marker();
    value["learning"]["items"][0]["future_item_field"] = extension_marker();
    value["learning"]["items"][0]["key"]["future_key_extension"] = serde_json::json!(true);
    value["learning"]["attempts"][0]["future_attempt_field"] = extension_marker();
    value["sessions"][0]["future_session_field"] = extension_marker();
    value["session_snapshots"][0]["future_snapshot_field"] = extension_marker();
    value["session_snapshots"][0]["session"]["future_nested_session_field"] = extension_marker();
    value["session_snapshots"][0]["session"]["settings"]["future_settings_field"] =
        extension_marker();
    value["session_snapshots"][0]["stats_scope"]["future_scope_field"] = extension_marker();
    value["session_snapshots"][0]["queue"][0]["future_target_field"] = extension_marker();
}

#[cfg(unix)]
fn file_inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn at_23_duplicate_attempt_has_one_effect_and_one_file_replacement() {
    let dir = TestDir::new("duplicate");
    let path = dir.join("repertoire-training.json");
    let store = LearningStore::new(&path);
    let first_attempt = attempt("attempt-1", 1_000);

    let first = store
        .apply_attempt(first_attempt.clone(), &SrsSettings::default())
        .unwrap();
    assert_eq!(first.application, AttemptApplication::Applied);
    assert_eq!(first.revision, 1);
    assert_eq!(first.learning.attempts.len(), 1);
    let key = LearningItemKey::local("fen-shared", "e2e4");
    assert_eq!(first.learning.item(&key).unwrap().correct_streak, 1);
    assert_eq!(first.learning.item(&key).unwrap().correct_count, 1);
    let bytes_after_first = std::fs::read(&path).unwrap();
    #[cfg(unix)]
    let inode_after_first = file_inode(&path);

    let duplicate = store
        .apply_attempt(first_attempt, &SrsSettings::default())
        .unwrap();
    assert_eq!(duplicate.application, AttemptApplication::Duplicate);
    assert_eq!(duplicate.revision, first.revision);
    assert_eq!(duplicate.learning, first.learning);
    assert_eq!(std::fs::read(&path).unwrap(), bytes_after_first);
    #[cfg(unix)]
    assert_eq!(file_inode(&path), inode_after_first);

    let different = store
        .apply_attempt(attempt("attempt-2", 2_000), &SrsSettings::default())
        .unwrap();
    assert_eq!(different.application, AttemptApplication::Applied);
    assert_eq!(different.revision, 2);
    assert_eq!(different.learning.attempts.len(), 2);
    assert_eq!(different.learning.item(&key).unwrap().correct_streak, 2);
    assert_eq!(different.learning.item(&key).unwrap().correct_count, 2);
}

#[test]
fn attempt_ids_are_scoped_to_user_and_different_ids_are_not_suppressed() {
    let dir = TestDir::new("attempt-scope");
    let store = LearningStore::new(dir.join("repertoire-training.json"));
    let mut other_user = attempt("shared-client-id", 10);
    other_user.user_id = "other-profile".into();
    assert_eq!(
        store
            .apply_attempt(other_user, &SrsSettings::default())
            .unwrap()
            .application,
        AttemptApplication::Applied
    );
    let local = store
        .apply_attempt(attempt("shared-client-id", 11), &SrsSettings::default())
        .unwrap();
    assert_eq!(local.application, AttemptApplication::Applied);
    assert_eq!(local.learning.attempts.len(), 2);
    assert_eq!(local.learning.items.len(), 2);
}

#[test]
fn two_stores_reload_latest_revision_instead_of_overwriting_stale_state() {
    let dir = TestDir::new("two-stores");
    let path = dir.join("repertoire-training.json");
    let first = LearningStore::new(&path);
    let second = LearningStore::new(&path);
    let stale_first = first.load().unwrap();
    let stale_second = second.load().unwrap();
    assert_eq!(stale_first.revision, stale_second.revision);

    let barrier = Arc::new(Barrier::new(3));
    let writers = [
        (first.clone(), "from-first"),
        (second.clone(), "from-second"),
    ]
    .map(|(store, id)| {
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store.apply_attempt(attempt(id, 100), &SrsSettings::default())
        })
    });
    barrier.wait();
    let mut revisions = writers
        .into_iter()
        .map(|writer| writer.join().unwrap().unwrap().revision)
        .collect::<Vec<_>>();
    revisions.sort_unstable();
    assert_eq!(revisions, [1, 2]);

    let third = first
        .apply_attempt(attempt("after-both", 200), &SrsSettings::default())
        .unwrap();
    assert_eq!(third.revision, 3);
    assert_eq!(third.learning.attempts.len(), 3);
    let persisted = second.load().unwrap();
    assert_eq!(persisted.revision, 3);
    assert_eq!(persisted.learning.attempts.len(), 3);
    assert!(stale_first.learning.attempts.is_empty());
    assert!(stale_second.learning.attempts.is_empty());
}

#[test]
fn process_attempt_writer_helper() {
    let (Some(path), Some(id)) = (
        std::env::var_os("CP_LEARNING_STORE_CHILD_PATH"),
        std::env::var_os("CP_LEARNING_STORE_CHILD_ID"),
    ) else {
        return;
    };
    let id = id.to_string_lossy().into_owned();
    let result = LearningStore::new(PathBuf::from(path))
        .apply_attempt(attempt(&id, 100), &SrsSettings::default())
        .unwrap();
    assert_eq!(result.application, AttemptApplication::Applied);
    std::process::exit(0);
}

#[test]
fn two_processes_and_stale_parent_snapshot_keep_every_attempt() {
    let dir = TestDir::new("two-processes");
    let path = dir.join("repertoire-training.json");
    let parent = LearningStore::new(&path);
    assert_eq!(parent.load().unwrap().revision, 0);
    let executable = std::env::current_exe().unwrap();
    let mut children = ["process-a", "process-b"]
        .into_iter()
        .map(|id| {
            std::process::Command::new(&executable)
                .arg("--exact")
                .arg("repertoire::store::store_tests::process_attempt_writer_helper")
                .env("CP_LEARNING_STORE_CHILD_PATH", &path)
                .env("CP_LEARNING_STORE_CHILD_ID", id)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    for child in &mut children {
        assert!(child.wait().unwrap().success());
    }

    let after_children = parent.load().unwrap();
    assert_eq!(after_children.revision, 2);
    assert_eq!(after_children.learning.attempts.len(), 2);
    let after_stale_parent = parent
        .apply_attempt(attempt("stale-parent", 200), &SrsSettings::default())
        .unwrap();
    assert_eq!(after_stale_parent.revision, 3);
    assert_eq!(after_stale_parent.learning.attempts.len(), 3);
}

#[test]
fn v2_to_v3_is_idempotent_and_backup_is_created_only_once() {
    let dir = TestDir::new("migration-idempotent");
    let path = dir.join("repertoire-training.json");
    let mut stats = StatsData::default();
    stats.revision = 4;
    stats.legacy_lines.insert(
        "old-line".into(),
        serde_json::json!({"runs": 3, "unknown": 7}),
    );
    let mut entry = legacy_move("repertoire-a", TrainingMode::White, "e2e4");
    entry.attempts = 5;
    entry.correct = 4;
    entry.wrong = 1;
    entry.streak = 2;
    entry.last_seen = Some(100);
    entry.due = Some(500);
    entry.interval_seconds = Some(400);
    stats.move_stats.push(entry);
    let original = write_json(&path, &stats);
    let store = LearningStore::new(&path);

    let (loaded, report) = store.load_with_report().unwrap();
    assert_eq!(loaded.version, LEARNING_STORE_VERSION);
    assert_eq!(loaded.revision, 4);
    assert_eq!(loaded.learning.items.len(), 1);
    assert_eq!(
        serde_json::to_value(&loaded.legacy_practice).unwrap(),
        serde_json::to_value(&stats).unwrap()
    );
    assert_eq!(report, MigrationReport::default());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let first_report = store.migrate().unwrap();
    assert_eq!(first_report, MigrationReport::default());
    let backup_path = dir.join("repertoire-training.json.before-learning-v3");
    assert_eq!(std::fs::read(&backup_path).unwrap(), original);
    let migrated = store.load().unwrap();
    assert_eq!(migrated.version, 3);
    assert_eq!(migrated.revision, 5);
    assert_eq!(
        serde_json::to_value(&migrated.legacy_practice).unwrap(),
        serde_json::to_value(&stats).unwrap()
    );
    let first_v3 = std::fs::read(&path).unwrap();

    assert_eq!(store.migrate().unwrap(), MigrationReport::default());
    assert_eq!(std::fs::read(&path).unwrap(), first_v3);
    assert_eq!(std::fs::read(&backup_path).unwrap(), original);

    store
        .apply_attempt(attempt("new-after-migration", 600), &SrsSettings::default())
        .unwrap();
    assert_eq!(store.load().unwrap().revision, 6);
    assert_eq!(std::fs::read(&backup_path).unwrap(), original);
    assert_eq!(
        store.load_with_report().unwrap().1,
        MigrationReport::default()
    );
}

#[test]
fn missing_legacy_interval_is_reconstructed_from_due_and_last_seen() {
    let dir = TestDir::new("legacy-interval-reconstruction");
    let path = dir.join("repertoire-training.json");
    let mut entry = legacy_move("repertoire-a", TrainingMode::White, "e2e4");
    entry.attempts = 1;
    entry.last_seen = Some(100);
    entry.due = Some(500);
    entry.interval_seconds = None;
    let mut stats = StatsData::default();
    stats.move_stats.push(entry);
    write_json(&path, &stats);

    let (loaded, _) = LearningStore::new(path).load_with_report().unwrap();
    let item = loaded
        .learning
        .item(&LearningItemKey::local("fen-shared", "e2e4"))
        .unwrap();
    assert_eq!(item.next_review_at, Some(500));
    assert_eq!(item.previous_interval_seconds, Some(400));
}

#[test]
fn overdue_legacy_interval_is_not_reconstructed_as_a_duration() {
    let dir = TestDir::new("legacy-overdue-interval");
    let path = dir.join("repertoire-training.json");
    for (due, last_seen) in [(100, 500), (100, 100)] {
        let mut entry = legacy_move("repertoire-a", TrainingMode::White, "e2e4");
        entry.attempts = 1;
        entry.last_seen = Some(last_seen);
        entry.due = Some(due);
        entry.interval_seconds = None;
        let mut stats = StatsData::default();
        stats.move_stats.push(entry);
        write_json(&path, &stats);

        let (loaded, _) = LearningStore::new(&path).load_with_report().unwrap();
        let item = loaded
            .learning
            .item(&LearningItemKey::local("fen-shared", "e2e4"))
            .unwrap();
        assert_eq!(item.previous_interval_seconds, None);
    }
}

#[test]
fn unusable_backup_path_blocks_migration_without_replacing_v2() {
    let dir = TestDir::new("backup-error");
    let path = dir.join("repertoire-training.json");
    let original = write_json(&path, &StatsData::default());
    let backup = dir.join("repertoire-training.json.before-learning-v3");
    std::fs::create_dir(&backup).unwrap();
    let store = LearningStore::new(&path);

    assert!(matches!(store.migrate(), Err(StoreError::Io { .. })));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(matches!(
        store.apply_attempt(attempt("not-persisted", 100), &SrsSettings::default()),
        Err(StoreError::Io { .. })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn collision_merge_uses_latest_streak_earliest_due_and_is_deterministic() {
    let dir = TestDir::new("collision");
    let mut older = legacy_move("repertoire-a", TrainingMode::White, "e2e4");
    older.attempts = 7;
    older.correct = 6;
    older.wrong = 1;
    older.streak = 2;
    older.last_seen = Some(100);
    older.due = Some(500);
    older.interval_seconds = Some(50);
    let mut latest = legacy_move("repertoire-b", TrainingMode::Black, "e2e4");
    latest.attempts = 3;
    latest.correct = 1;
    latest.wrong = 2;
    latest.streak = -3;
    latest.last_seen = Some(200);
    latest.due = Some(300);
    latest.interval_seconds = Some(40);

    let mut stats = StatsData::default();
    stats.move_stats = vec![older.clone(), latest.clone()];
    let mut reversed = StatsData::default();
    reversed.move_stats = vec![latest, older];
    let first_path = dir.join("first.json");
    let second_path = dir.join("second.json");
    write_json(&first_path, &stats);
    write_json(&second_path, &reversed);

    let (data, report) = LearningStore::new(first_path).load_with_report().unwrap();
    let (reversed_data, reversed_report) =
        LearningStore::new(second_path).load_with_report().unwrap();
    assert_eq!(data.learning, reversed_data.learning);
    assert_eq!(report, reversed_report);
    assert_eq!(report.merged, 1);
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.skipped_both, 0);
    assert_eq!(
        serde_json::to_value(&data.legacy_practice).unwrap(),
        serde_json::to_value(&stats).unwrap()
    );

    assert_eq!(data.learning.items.len(), 1);
    let item = &data.learning.items[0];
    assert_eq!(item.key, LearningItemKey::local("fen-shared", "e2e4"));
    assert_eq!(item.correct_streak, 0);
    assert_eq!(item.wrong_streak, 3);
    assert_eq!(item.last_review_at, Some(200));
    assert_eq!(item.next_review_at, Some(300));
    assert_eq!(item.previous_interval_seconds, Some(40));
    assert!(item.introduced);
    assert_eq!(item.correct_count, 0);
    assert_eq!(item.wrong_count, 0);
    assert_eq!(item.last_correct_at, None);
    assert_eq!(item.accuracy_percent(), None);
}

#[test]
fn both_without_uci_is_skipped_and_old_attempts_do_not_create_accuracy() {
    let dir = TestDir::new("conservative-migration");
    let mut skipped = legacy_move("repertoire-a", TrainingMode::Both, "");
    skipped.attempts = 20;
    skipped.correct = 15;
    skipped.wrong = 5;
    skipped.streak = 4;
    skipped.due = Some(100);
    let mut scheduled_only = legacy_move("repertoire-a", TrainingMode::White, "e2e4");
    scheduled_only.due = Some(200);
    scheduled_only.interval_seconds = Some(100);
    let mut attempted = legacy_move("repertoire-b", TrainingMode::Black, "d2d4");
    attempted.attempts = 8;
    attempted.correct = 7;
    attempted.wrong = 1;
    attempted.streak = 2;
    let mut stats = StatsData::default();
    stats.move_stats = vec![skipped, scheduled_only, attempted];
    let path = dir.join("stats.json");
    write_json(&path, &stats);

    let (data, report) = LearningStore::new(path).load_with_report().unwrap();
    assert_eq!(report.skipped_both, 1);
    assert_eq!(report.merged, 0);
    assert_eq!(report.conflicts, 0);
    assert_eq!(data.learning.items.len(), 2);
    assert_eq!(
        data.legacy_practice.unwrap()["move_stats"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let scheduled = data
        .learning
        .item(&LearningItemKey::local("fen-shared", "e2e4"))
        .unwrap();
    assert!(scheduled.introduced);
    assert_eq!(scheduled.next_review_at, Some(200));
    assert_eq!(scheduled.previous_interval_seconds, Some(100));
    let attempted = data
        .learning
        .item(&LearningItemKey::local("fen-shared", "d2d4"))
        .unwrap();
    assert!(attempted.introduced);
    assert_eq!(attempted.correct_streak, 2);
    assert_eq!(attempted.independent_attempt_count(), 0);
    assert_eq!(attempted.accuracy_percent(), None);
    assert_eq!(attempted.last_correct_at, None);
}

#[test]
fn legacy_lines_are_retained_and_training_json_is_never_changed() {
    let dir = TestDir::new("legacy-lines");
    let legacy_path = dir.join("training.json");
    let legacy_bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "line-a": {"runs": 4, "unknown": [1, 2, 3]},
        "line-b": {"runs": 2, "best": 1}
    }))
    .unwrap();
    std::fs::write(&legacy_path, &legacy_bytes).unwrap();
    let path = dir.join("repertoire-training.json");
    let store = LearningStore::with_legacy(&path, &legacy_path);

    let loaded = store.load().unwrap();
    let legacy = loaded.legacy_practice.as_ref().unwrap();
    assert_eq!(legacy["legacy_lines"].as_object().unwrap().len(), 2);
    assert_eq!(
        legacy["legacy_lines"]["line-a"]["unknown"],
        serde_json::json!([1, 2, 3])
    );
    store.migrate().unwrap();
    assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy_bytes);
    store
        .apply_attempt(attempt("new-event", 1_000), &SrsSettings::default())
        .unwrap();
    assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy_bytes);
    let persisted = store.load().unwrap();
    assert_eq!(
        persisted.legacy_practice.unwrap()["legacy_lines"],
        legacy["legacy_lines"]
    );

    let old_reader = StatsStore::with_legacy(dir.join("old-reader.json"), &legacy_path);
    assert_eq!(old_reader.load().unwrap().legacy_lines.len(), 2);
    assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy_bytes);
}

#[test]
fn v3_session_snapshot_survives_attempt_persistence() {
    let dir = TestDir::new("sessions");
    let path = dir.join("repertoire-training.json");
    let mut data = LearningStoreData {
        revision: 7,
        ..LearningStoreData::default()
    };
    let session = LearningSession {
        id: "session-1".into(),
        mode: QueueMode::SpacedRepetition,
        side: TrainingMode::White,
        scope: TrainingScopeKey::default(),
        settings: SrsSettings {
            growth_factor: 2.25,
            ..SrsSettings::default()
        },
    };
    data.sessions.push(session.clone());
    write_json(&path, &data);

    let store = LearningStore::new(&path);
    let loaded = store.load().unwrap();
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions.first(), Some(&session));
    let applied = store
        .apply_attempt(attempt("session-event", 100), &SrsSettings::default())
        .unwrap();
    assert_eq!(applied.revision, 8);
    assert_eq!(store.load().unwrap().sessions, [session]);
}

#[test]
fn v3_unknown_fields_survive_plain_and_session_attempt_roundtrips() {
    let dir = TestDir::new("v3-extensions");
    let path = dir.join("repertoire-training.json");
    let (data, library) = extension_fixture();
    let mut raw = serde_json::to_value(data).unwrap();
    inject_v3_extensions(&mut raw);
    let key_shaped = |object: &mut serde_json::Value| {
        object["user_id"] = serde_json::json!("local");
        object["position_fen4"] = serde_json::json!("fen-shared");
        object["expected_uci"] = serde_json::json!("e2e4");
    };
    key_shaped(&mut raw);
    key_shaped(&mut raw["learning"]);
    let original = write_json(&path, &raw);
    let store = LearningStore::new(&path);
    assert_eq!(store.load().unwrap().revision, 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    store
        .apply_attempt(attempt("plain-event", 100), &SrsSettings::default())
        .unwrap();
    let saved = read_json(&path);
    assert_eq!(saved["revision"], serde_json::json!(1));
    assert_eq!(saved["learning"]["attempts"].as_array().unwrap().len(), 2);
    assert_v3_extensions(&saved);
    assert!(
        saved["learning"]["items"][0]["key"]
            .get("future_key_extension")
            .is_none()
    );

    let loaded = store.load().unwrap();
    assert_eq!(loaded.revision, 1);
    assert_eq!(
        loaded.extensions["future_top_level_field"],
        extension_marker()
    );
    assert_eq!(
        loaded.learning.extensions["future_learning_field"],
        extension_marker()
    );
    let mut session = loaded.session_snapshot("session-1").cloned().unwrap();
    let session_attempt = session
        .play_uci(&library, "e2e4", "session-event", 200, Some(250))
        .unwrap();
    let persisted = store
        .apply_attempt_and_session(session_attempt, &mut session)
        .unwrap();
    assert_eq!(persisted.application, AttemptApplication::Applied);
    assert_eq!(persisted.revision, 2);
    assert_eq!(persisted.session.revision, 1);

    let saved = read_json(&path);
    assert_eq!(saved["revision"], serde_json::json!(2));
    assert_eq!(saved["learning"]["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(
        saved["session_snapshots"][0]["revision"],
        serde_json::json!(1)
    );
    assert_v3_extensions(&saved);
}

#[test]
fn v2_unknown_root_and_nested_fields_survive_migration_and_later_writes() {
    let dir = TestDir::new("v2-raw-extensions");
    let path = dir.join("repertoire-training.json");
    let mut stats = StatsData::default();
    stats.revision = 2;
    stats
        .legacy_lines
        .insert("old-line".into(), serde_json::json!({"runs": 3}));
    let mut entry = legacy_move("repertoire-a", TrainingMode::White, "e2e4");
    entry.attempts = 1;
    entry.correct = 1;
    entry.last_seen = Some(100);
    entry.due = Some(500);
    entry.interval_seconds = Some(400);
    stats.move_stats.push(entry);
    let mut raw = serde_json::to_value(&stats).unwrap();
    raw["future_v2_root"] = extension_marker();
    raw["move_stats"][0]["future_move_stats_field"] = extension_marker();
    raw["move_stats"][0]["key"]["future_move_key_field"] = extension_marker();
    raw["move_stats"][0]["recent"] = serde_json::json!([{
        "at": 100,
        "correct": true,
        "hinted": false,
        "actual_uci": "e2e4",
        "future_recent_field": extension_marker()
    }]);
    raw["legacy_lines"]["old-line"]["future_line_field"] = extension_marker();
    let original = write_json(&path, &raw);
    let store = LearningStore::new(&path);
    let extension_pointers = [
        "/legacy_practice/future_v2_root",
        "/legacy_practice/move_stats/0/future_move_stats_field",
        "/legacy_practice/move_stats/0/key/future_move_key_field",
        "/legacy_practice/move_stats/0/recent/0/future_recent_field",
        "/legacy_practice/legacy_lines/old-line/future_line_field",
    ];

    let (loaded, _) = store.load_with_report().unwrap();
    assert_eq!(loaded.learning.items.len(), 1);
    assert_eq!(
        loaded.legacy_practice.as_ref().unwrap()["future_v2_root"],
        extension_marker()
    );
    store.migrate().unwrap();
    let backup = dir.join("repertoire-training.json.before-learning-v3");
    assert_eq!(std::fs::read(backup).unwrap(), original);
    let saved = read_json(&path);
    assert_eq!(saved["revision"], serde_json::json!(3));
    for pointer in extension_pointers {
        assert_eq!(
            saved.pointer(pointer),
            Some(&extension_marker()),
            "{pointer}"
        );
    }

    store
        .apply_attempt(attempt("after-raw-migration", 600), &SrsSettings::default())
        .unwrap();
    let saved = read_json(&path);
    assert_eq!(saved["revision"], serde_json::json!(4));
    for pointer in extension_pointers {
        assert_eq!(
            saved.pointer(pointer),
            Some(&extension_marker()),
            "{pointer}"
        );
    }
}

#[test]
fn unreadable_missing_and_unsupported_versions_are_errors_not_empty_data() {
    let dir = TestDir::new("read-errors");
    let path = dir.join("repertoire-training.json");
    let store = LearningStore::new(&path);

    std::fs::write(&path, b"{broken").unwrap();
    assert!(matches!(store.load(), Err(StoreError::Json { .. })));
    assert!(matches!(store.migrate(), Err(StoreError::Json { .. })));
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");

    std::fs::write(&path, br#"{"learning":{"items":[],"attempts":[]}}"#).unwrap();
    assert!(matches!(
        store.load(),
        Err(StoreError::MissingVersion { .. })
    ));

    std::fs::write(
        &path,
        br#"{"version":4,"learning":{"items":[],"attempts":[]}}"#,
    )
    .unwrap();
    assert!(matches!(
        store.load(),
        Err(StoreError::UnsupportedVersion { version: 4, .. })
    ));
    assert!(matches!(
        store.apply_attempt(attempt("must-not-write", 1), &SrsSettings::default()),
        Err(StoreError::UnsupportedVersion { version: 4, .. })
    ));

    std::fs::write(&path, br#"{"version":3,"learning":[]}"#).unwrap();
    let result = store.load();
    assert!(
        matches!(&result, Err(StoreError::Json { .. })),
        "unexpected result: {result:?}"
    );
}

#[test]
fn extension_merge_tracks_reordered_identities_and_keeps_first_duplicate() {
    let raw = serde_json::json!([
        {"user_id":"u", "client_attempt_id":"a", "known":0, "plugin":{"first":true}},
        {"user_id":"u", "client_attempt_id":"b", "known":0, "plugin":{"second":true}},
        {"user_id":"u", "client_attempt_id":"a", "known":0, "plugin":{"wrong":true}}
    ]);
    let current = serde_json::json!([
        {"user_id":"u", "client_attempt_id":"b", "known":2},
        {"user_id":"u", "client_attempt_id":"a", "known":1},
        {"user_id":"u", "client_attempt_id":"c", "known":3}
    ]);
    let merged = preserve_extensions(&raw, current);
    assert_eq!(
        merged,
        serde_json::json!([
            {"user_id":"u", "client_attempt_id":"b", "known":2, "plugin":{"second":true}},
            {"user_id":"u", "client_attempt_id":"a", "known":1, "plugin":{"first":true}},
            {"user_id":"u", "client_attempt_id":"c", "known":3}
        ])
    );
}

#[test]
fn context_batch_is_atomic_retryable_and_preserves_every_event() {
    let dir = TestDir::new("context-batch");
    let store = LearningStore::new(dir.join("learning.json"));
    let mut library = Library::default();
    let id = library
        .create(
            "Context",
            &chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 3. Bb5 *").game,
        )
        .unwrap();
    let scope = StatsScope::for_repertoire(id, LearningSide::White, 100);
    let mut learning = LearningData::default();
    let key = crate::repertoire::active_learning_memberships(&library, &scope)
        .unwrap()
        .into_iter()
        .find(|m| m.key.expected_uci == "f1b5")
        .unwrap()
        .key;
    let item = learning.ensure_item(key);
    item.introduced = true;
    item.next_review_at = Some(0);
    let mut session = LearningSessionSnapshot::new(
        "context-batch",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
    )
    .unwrap();
    store.save_session(&mut session).unwrap();
    let start = store.load().unwrap();
    let original = std::fs::read(store.path()).unwrap();
    let mut events = Vec::new();
    for i in 0..4 {
        events.push(
            session
                .advance_context_with_learning(&library, &learning, format!("context-{i}"), 100)
                .unwrap(),
        );
    }
    assert_eq!(session.context_index, 4);
    let retry = session.clone();
    assert!(
        store
            .apply_attempts_and_session(vec![], &mut session)
            .is_err()
    );
    let mut invalid = events.clone();
    invalid[3].client_attempt_id.clear();
    assert!(
        store
            .apply_attempts_and_session(invalid, &mut session)
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), original);
    assert_eq!(session, retry);
    assert!(
        store
            .apply_attempts_and_session(vec![events[0].clone(), events[0].clone()], &mut session)
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), original);

    let saved = store
        .apply_attempts_and_session(events.clone(), &mut session)
        .unwrap();
    assert_eq!(saved.application, AttemptApplication::Applied);
    assert_eq!(
        saved.revision,
        start.revision + 1,
        "one transaction for the whole prefix"
    );
    assert_eq!(saved.session.revision, retry.revision + 1);
    assert_eq!(saved.learning.attempts, events);
    assert!(
        saved
            .learning
            .items
            .iter()
            .all(|item| item.correct_streak == 0)
    );
    let bytes = std::fs::read(store.path()).unwrap();
    let mut stale = retry.clone();
    let duplicate = store
        .apply_attempts_and_session(events.clone(), &mut stale)
        .unwrap();
    assert_eq!(duplicate.application, AttemptApplication::Duplicate);
    assert_eq!(duplicate.session, saved.session);
    assert_eq!(std::fs::read(store.path()).unwrap(), bytes);

    let mut partial = events.clone();
    partial[3].client_attempt_id = "new-id".into();
    assert!(
        store
            .apply_attempts_and_session(partial, &mut session)
            .is_err()
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), bytes);
    let mut concurrent = events.clone();
    for event in &mut concurrent {
        event.client_attempt_id.push_str("-concurrent");
    }
    assert!(matches!(
        store.apply_attempts_and_session(concurrent, &mut stale),
        Err(StoreError::SessionConflict { .. })
    ));
    assert_eq!(std::fs::read(store.path()).unwrap(), bytes);
    let loaded = store.load().unwrap();
    loaded
        .session_snapshot("context-batch")
        .unwrap()
        .validate(&library)
        .unwrap();
    assert_eq!(loaded.learning.attempts, events);
}
