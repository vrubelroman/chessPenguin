use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 24 * 60 * 60;
static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cp-learning-session-{}-{}-{name}",
            std::process::id(),
            sequence
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn store(&self) -> LearningStore {
        LearningStore::new(self.0.join("learning.json"))
    }

    fn path(&self) -> PathBuf {
        self.0.join("learning.json")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn library(pgn: &str) -> (Library, RepertoireId) {
    let mut library = Library::default();
    let id = library
        .create("Test", &chess_core::parse_str(pgn).game)
        .unwrap();
    (library, id)
}

fn scope(id: RepertoireId, side: LearningSide) -> StatsScope {
    StatsScope::for_repertoire(id, side, NOW)
}

fn key_for(library: &Library, scope: &StatsScope, expected_uci: &str) -> LearningItemKey {
    active_learning_memberships(library, scope)
        .unwrap()
        .into_iter()
        .find(|membership| membership.key.expected_uci == expected_uci)
        .unwrap()
        .key
}

fn introduce_at(learning: &mut LearningData, key: LearningItemKey, next_review_at: i64) {
    let item = learning.ensure_item(key);
    item.introduced = true;
    item.next_review_at = Some(next_review_at);
}

fn due_session(
    id: &str,
    library: &Library,
    learning: &LearningData,
    scope: StatsScope,
) -> LearningSessionSnapshot {
    LearningSessionSnapshot::new(id, library, learning, scope, SrsSettings::default()).unwrap()
}

fn seed_store(dir: &TestDir, learning: &LearningData) {
    let data = LearningStoreData {
        learning: learning.clone(),
        ..LearningStoreData::default()
    };
    let bytes = serde_json::to_vec_pretty(&data).unwrap();
    std::fs::write(dir.path(), bytes).unwrap();
}

fn commit(
    store: &LearningStore,
    session: &mut LearningSessionSnapshot,
    attempt: TrainingAttempt,
) -> PersistedSessionAttempt {
    let persisted = store.apply_attempt_and_session(attempt, session).unwrap();
    *session = persisted.session.clone();
    persisted
}

fn play(
    store: &LearningStore,
    library: &Library,
    session: &mut LearningSessionSnapshot,
    uci: &str,
    id: &str,
    at: i64,
    response_time_ms: Option<u64>,
) -> PersistedSessionAttempt {
    let attempt = session
        .play_uci(library, uci, id, at, response_time_ms)
        .unwrap();
    commit(store, session, attempt)
}

fn advance(
    store: &LearningStore,
    library: &Library,
    session: &mut LearningSessionSnapshot,
    id: &str,
    at: i64,
) -> PersistedSessionAttempt {
    let attempt = session.advance_context(library, id, at).unwrap();
    commit(store, session, attempt)
}

fn restore(store: &LearningStore, library: &Library, id: &str) -> LearningSessionSnapshot {
    store
        .load()
        .unwrap()
        .session_snapshot(id)
        .cloned()
        .unwrap()
        .resume(library)
        .unwrap()
}

#[cfg(unix)]
fn file_inode(path: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn initial_queue_is_due_only_and_zero_due_finishes_without_unseen() {
    let (library, id) = library("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let nf3 = key_for(&library, &scope, "g1f3");
    let bb5 = key_for(&library, &scope, "f1b5");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, e4.clone(), NOW);
    introduce_at(&mut learning, nf3, NOW + DAY);

    let session = due_session("due", &library, &learning, scope.clone());
    assert_eq!(session.queue.len(), 1);
    assert_eq!(session.current_target().unwrap().key, e4);
    assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);
    assert!(!session.queue.iter().any(|target| target.key == bb5));

    let empty = due_session("empty", &library, &LearningData::default(), scope);
    assert_eq!(empty.phase, LearningSessionPhase::Finished);
    assert!(empty.queue.is_empty());
    assert_eq!(empty.remaining(), 0);
}

#[test]
fn introduction_is_explicit_and_does_not_introduce_until_an_answer() {
    let (library, id) = library("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *");
    let scope = scope(id, LearningSide::White);
    let learning = LearningData::default();
    let mut session = due_session("introduce", &library, &learning, scope.clone());
    assert_eq!(session.phase, LearningSessionPhase::Finished);

    assert_eq!(session.introduce_new(&library, &learning).unwrap(), 3);
    assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);
    assert!(learning.items.is_empty());
    assert!(session.completed_keys.is_empty());

    let store = TestDir::new("introduce");
    let store = store.store();
    let persisted = play(
        &store,
        &library,
        &mut session,
        "e2e4",
        "first-new",
        NOW,
        Some(100),
    );
    let first = persisted
        .learning
        .item(&key_for(&library, &scope, "e2e4"))
        .unwrap();
    assert!(first.introduced);
    assert_eq!(first.correct_streak, 1);
    assert_eq!(session.completed_keys.len(), 1);
}

#[test]
fn illegal_input_and_unscheduled_repertoire_answer_do_not_change_learning() {
    let (library, id) = library("1. e4 (1. d4) e5 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, e4.clone(), NOW);
    let before = learning.item(&e4).unwrap().clone();
    let dir = TestDir::new("illegal");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("strict", &library, &learning, scope);

    let illegal = play(
        &store,
        &library,
        &mut session,
        "e2e5",
        "illegal",
        NOW,
        Some(9_999),
    );
    assert_eq!(
        illegal.learning.attempts[0].event_type,
        TrainingEventType::IllegalMove
    );
    assert_eq!(illegal.learning.attempts[0].response_time_ms, None);
    assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);
    assert_eq!(illegal.learning.item(&e4), Some(&before));

    let before_session = session.clone();
    assert!(matches!(
        session.play_uci(&library, "d2d4", "legal-alternative", NOW + 1, Some(250)),
        Err(LearningSessionError::RepertoireAlternative {
            already_completed: false
        })
    ));
    assert_eq!(session, before_session);
    let unchanged = store.load().unwrap().learning;
    assert_eq!(unchanged.attempts.len(), 1);
    assert_eq!(unchanged.item(&e4), Some(&before));
}

#[test]
fn at_05_wrong_then_repeated_replay_records_one_independent_wrong() {
    let (library, id) = library("1. e4 e5 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, e4.clone(), NOW);
    let dir = TestDir::new("wrong-replay");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("wrong-replay", &library, &learning, scope);

    let wrong = play(
        &store,
        &library,
        &mut session,
        "d2d4",
        "wrong",
        NOW,
        Some(500),
    );
    let after_wrong = wrong.learning.item(&e4).unwrap().clone();
    assert_eq!(after_wrong.correct_streak, 0);
    assert_eq!(after_wrong.wrong_count, 1);
    assert_eq!(after_wrong.next_review_at, Some(NOW + 30));
    assert_eq!(after_wrong.independent_response_times_ms, [500]);
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .revealed_move
            .as_deref(),
        Some("e2e4")
    );

    let repeated_wrong = play(
        &store,
        &library,
        &mut session,
        "c2c4",
        "replay-wrong",
        NOW + 1,
        Some(800),
    );
    assert_eq!(
        repeated_wrong.learning.attempts[1].event_type,
        TrainingEventType::ReplayAfterError
    );
    assert_eq!(repeated_wrong.learning.attempts[1].response_time_ms, None);
    assert_eq!(repeated_wrong.learning.item(&e4), Some(&after_wrong));
    assert_eq!(
        session.phase,
        LearningSessionPhase::AwaitingReplayAfterError
    );

    let replayed = play(
        &store,
        &library,
        &mut session,
        "e2e4",
        "replay-expected",
        NOW + 2,
        Some(900),
    );
    assert_eq!(
        replayed.learning.attempts[2].event_type,
        TrainingEventType::ReplayAfterError
    );
    assert_eq!(replayed.learning.attempts[2].response_time_ms, None);
    assert_eq!(replayed.learning.item(&e4), Some(&after_wrong));
    assert_eq!(
        replayed
            .learning
            .item(&e4)
            .unwrap()
            .independent_attempt_count(),
        1
    );
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    assert!(session.completed_keys.contains(&e4));
}

#[test]
fn pending_alternative_credits_only_the_played_answer_and_keeps_original_pending() {
    let (library, id) = library("1. e4 (1. d4) *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let d4 = key_for(&library, &scope, "d2d4");
    let mut session = due_session("alternatives", &library, &LearningData::default(), scope);
    assert_eq!(
        session
            .introduce_new(&library, &LearningData::default())
            .unwrap(),
        2
    );
    assert_eq!(session.current_target().unwrap().key, e4);
    let dir = TestDir::new("alternatives");
    let store = dir.store();
    let first = play(
        &store,
        &library,
        &mut session,
        "d2d4",
        "d4-first",
        NOW,
        Some(123),
    );
    assert_eq!(first.learning.attempts[0].item_key(), d4);
    assert_eq!(first.learning.item(&d4).unwrap().correct_count, 1);
    assert!(first.learning.item(&e4).is_none());
    assert_eq!(session.current_target().unwrap().key, e4);
    assert_eq!(session.remaining(), 1);
    session = restore(&store, &library, "alternatives");
    let before = session.clone();
    assert!(matches!(
        session.play_uci(&library, "d2d4", "d4-again", NOW + 1, Some(12)),
        Err(LearningSessionError::RepertoireAlternative {
            already_completed: true
        })
    ));
    assert_eq!(session, before);
    assert_eq!(store.load().unwrap().learning.attempts.len(), 1);
    let second = play(
        &store,
        &library,
        &mut session,
        "e2e4",
        "e4-next",
        NOW + 2,
        Some(124),
    );
    assert_eq!(second.learning.item(&e4).unwrap().correct_count, 1);
    assert_eq!(second.learning.item(&d4).unwrap().correct_count, 1);
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    assert_eq!(session.completed_keys.len(), 2);
}

#[test]
fn prepared_answer_cache_preserves_live_classification_and_credit() {
    let (library, id) = library("1. e4 (1. d4) (1. c4) *");
    let selected = scope(id, LearningSide::White);
    let answers = active_learning_keys(&library, &selected).unwrap();
    let mut learning = LearningData::default();
    for uci in ["e2e4", "d2d4"] {
        introduce_at(&mut learning, key_for(&library, &selected, uci), NOW);
    }
    let original = due_session("answer-cache", &library, &learning, selected);
    for uci in ["e2e4", "d2d4", "c2c4", "a2a3", "e2e5"] {
        let mut direct = original.clone();
        let mut cached = original.clone();
        let first =
            direct.play_uci_with_learning(&library, &learning, uci, "answer", NOW, Some(123));
        let second = cached.play_uci_with_learning_answers(
            &library,
            &learning,
            &answers,
            uci,
            "answer",
            NOW,
            Some(123),
        );
        assert_eq!(first, second, "different event for {uci}");
        assert_eq!(direct, cached, "different session transition for {uci}");
    }
}

#[test]
fn alternative_during_context_does_not_continue_the_unplayed_branch() {
    let (library, id) = library("1. e4 (1. d4 d5 2. c4) e5 2. Nf3 (2. Bc4) Nc6 *");
    let scope = scope(id, LearningSide::White);
    let mut session = LearningSessionSnapshot::new_with_mode(
        "context-alternative",
        &library,
        &LearningData::default(),
        scope,
        SrsSettings::default(),
        QueueMode::LeastRecent,
    )
    .unwrap();
    let deep = session
        .queue
        .iter()
        .position(|target| target.key.expected_uci == "g1f3")
        .unwrap();
    session.queue.swap(0, deep);
    session.phase = LearningSessionPhase::AwaitingContextIndependent;
    session.context_index = 0;
    let original = session.current_target().unwrap().key.clone();
    session.validate(&library).unwrap();
    let dir = TestDir::new("context-alternative");
    let first = play(
        &dir.store(),
        &library,
        &mut session,
        "d2d4",
        "d4-context",
        NOW,
        Some(300),
    );
    assert_eq!(first.learning.attempts[0].expected_uci, "d2d4");
    assert_eq!(
        first.learning.attempts[0].event_type,
        TrainingEventType::CorrectIndependent
    );
    assert!(!session.completed_keys.contains(&original));
    assert!(
        session
            .queue
            .iter()
            .skip(session.current_index)
            .any(|target| target.key == original)
    );
    assert_eq!(
        session.context_index, 0,
        "do not increment the e4 path after playing d4"
    );
    session.validate(&library).unwrap();
    let restored = restore(&dir.store(), &library, "context-alternative");
    assert_eq!(restored, session);
}

#[test]
fn replay_stays_on_revealed_answer_and_disabled_alternative_is_not_accepted() {
    let (mut library, id) = library("1. e4 (1. d4) *");
    let selected = scope(id.clone(), LearningSide::White);
    let mut session = due_session(
        "replay-choices",
        &library,
        &LearningData::default(),
        selected.clone(),
    );
    session
        .introduce_new(&library, &LearningData::default())
        .unwrap();
    session.preview("preview", NOW).unwrap();
    let before = session.clone();
    assert!(matches!(
        session.play_uci(&library, "d2d4", "other-preview", NOW, None),
        Err(LearningSessionError::RepertoireAlternative {
            already_completed: false
        })
    ));
    assert_eq!(session, before);
    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&["d2d4".to_owned()], true)
        .unwrap();
    let mut session = due_session(
        "disabled-choice",
        &library,
        &LearningData::default(),
        selected,
    );
    session
        .introduce_new(&library, &LearningData::default())
        .unwrap();
    let attempt = session
        .play_uci(&library, "d2d4", "disabled", NOW, Some(123))
        .unwrap();
    assert_eq!(attempt.expected_uci, "e2e4");
    assert_eq!(attempt.event_type, TrainingEventType::IncorrectIndependent);
}

#[test]
fn at_06_preview_then_played_move_has_no_streak_accuracy_or_extra_preview() {
    let (library, id) = library("1. e4 e5 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let mut learning = LearningData::default();
    let item = learning.ensure_item(e4.clone());
    item.introduced = true;
    item.correct_streak = 2;
    item.wrong_streak = 1;
    item.correct_count = 4;
    item.wrong_count = 1;
    item.last_correct_at = Some(NOW - DAY);
    item.last_review_at = Some(NOW - DAY);
    item.next_review_at = Some(NOW);
    item.previous_interval_seconds = Some(DAY);
    let dir = TestDir::new("preview");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("preview", &library, &learning, scope);

    let preview = session.preview("preview", NOW).unwrap().unwrap();
    assert_eq!(preview.event_type, TrainingEventType::Previewed);
    assert_eq!(preview.response_time_ms, None);
    let previewed = commit(&store, &mut session, preview);
    let after_preview = previewed.learning.item(&e4).unwrap().clone();
    assert_eq!(after_preview.next_review_at, Some(NOW + 60));
    assert_eq!(after_preview.correct_streak, 2);
    assert_eq!(after_preview.wrong_streak, 1);
    assert_eq!(after_preview.correct_count, 4);
    assert_eq!(after_preview.wrong_count, 1);
    assert_eq!(after_preview.last_correct_at, Some(NOW - DAY));
    assert!(after_preview.independent_response_times_ms.is_empty());
    assert!(
        session
            .preview("duplicate-hint", NOW + 1)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.load().unwrap().learning.attempts.len(), 1);

    let played = play(
        &store,
        &library,
        &mut session,
        "e2e4",
        "preview-played",
        NOW + 2,
        Some(1_000),
    );
    assert_eq!(
        played.learning.attempts[1].event_type,
        TrainingEventType::AutoContext
    );
    assert_eq!(played.learning.attempts[1].response_time_ms, None);
    assert_eq!(played.learning.item(&e4), Some(&after_preview));
    assert_eq!(
        played.learning.item(&e4).unwrap().accuracy_percent(),
        Some(80.0)
    );
    assert_eq!(session.phase, LearningSessionPhase::Finished);
}

#[test]
fn at_23_duplicate_does_not_move_session_or_rewrite_file() {
    let (library, id) = library("1. e4 e5 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, e4.clone(), NOW);
    let dir = TestDir::new("duplicate");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("duplicate", &library, &learning, scope);
    store.save_session(&mut session).unwrap();
    let attempt = session
        .play_uci(&library, "e2e4", "same-response", NOW, Some(100))
        .unwrap();
    let first = commit(&store, &mut session, attempt.clone());
    assert_eq!(first.application, AttemptApplication::Applied);
    let bytes = std::fs::read(dir.path()).unwrap();
    #[cfg(unix)]
    let inode = file_inode(&dir.path());

    let mut duplicate_next = session.clone();
    duplicate_next.completed_keys.clear();
    duplicate_next.current_index = 99;
    let duplicate = store
        .apply_attempt_and_session(attempt, &mut duplicate_next)
        .unwrap();
    assert_eq!(duplicate.application, AttemptApplication::Duplicate);
    assert_eq!(duplicate.revision, first.revision);
    assert_eq!(duplicate.session, session);
    assert_eq!(duplicate.learning.attempts.len(), 1);
    assert_eq!(duplicate.learning.item(&e4).unwrap().correct_count, 1);
    assert_eq!(std::fs::read(dir.path()).unwrap(), bytes);
    #[cfg(unix)]
    assert_eq!(file_inode(&dir.path()), inode);
}

#[cfg(unix)]
#[test]
fn attempt_and_session_replace_the_old_inode_once_with_both_changes() {
    let (library, id) = library("1. e4 e5 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, e4, NOW);
    let dir = TestDir::new("atomic");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("atomic", &library, &learning, scope);
    store.save_session(&mut session).unwrap();
    let old_link = dir.0.join("old-inode.json");
    std::fs::hard_link(dir.path(), &old_link).unwrap();
    let old_inode = file_inode(&dir.path());
    let old_revision = store.load().unwrap().revision;

    let attempt = session
        .play_uci(&library, "d2d4", "atomic-wrong", NOW, Some(100))
        .unwrap();
    let persisted = commit(&store, &mut session, attempt);
    assert_eq!(persisted.revision, old_revision + 1);
    assert_ne!(file_inode(&dir.path()), old_inode);
    assert_eq!(file_inode(&old_link), old_inode);

    let old = LearningStore::new(&old_link).load().unwrap();
    assert!(old.learning.attempts.is_empty());
    assert_eq!(
        old.session_snapshot("atomic").unwrap().phase,
        LearningSessionPhase::AwaitingIndependent
    );
    let new = store.load().unwrap();
    assert_eq!(new.learning.attempts.len(), 1);
    assert_eq!(
        new.session_snapshot("atomic").unwrap().phase,
        LearningSessionPhase::AwaitingReplayAfterError
    );
}

#[test]
fn same_session_conflict_is_explicit_and_different_sessions_keep_both_events() {
    let (library, id) = library("1. e4 e5 *");
    let scope = scope(id, LearningSide::White);
    let e4 = key_for(&library, &scope, "e2e4");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, e4.clone(), NOW);
    let dir = TestDir::new("conflict");
    let first_store = dir.store();
    let second_store = LearningStore::new(dir.path());
    seed_store(&dir, &learning);
    let original = due_session("shared-session", &library, &learning, scope.clone());
    let mut left = original.clone();
    let mut right = original;
    let left_attempt = left
        .play_uci(&library, "d2d4", "left", NOW, Some(100))
        .unwrap();
    let right_attempt = right
        .play_uci(&library, "e2e4", "right", NOW, Some(100))
        .unwrap();
    commit(&first_store, &mut left, left_attempt);
    let before_conflict = std::fs::read(dir.path()).unwrap();

    let conflict = second_store.apply_attempt_and_session(right_attempt, &mut right);
    assert!(matches!(
        conflict,
        Err(StoreError::SessionConflict {
            expected: 0,
            actual: 1,
            ..
        })
    ));
    assert_eq!(std::fs::read(dir.path()).unwrap(), before_conflict);
    assert_eq!(first_store.load().unwrap().learning.attempts.len(), 1);

    let mut restored = restore(&first_store, &library, "shared-session");
    play(
        &first_store,
        &library,
        &mut restored,
        "e2e4",
        "after-reload",
        NOW + 1,
        None,
    );
    assert_eq!(first_store.load().unwrap().learning.attempts.len(), 2);

    let mut other_a = due_session("session-a", &library, &learning, scope.clone());
    let mut other_b = due_session("session-b", &library, &learning, scope);
    let a = other_a
        .play_uci(&library, "e2e4", "session-a-answer", NOW, Some(100))
        .unwrap();
    let b = other_b
        .play_uci(&library, "d2d4", "session-b-answer", NOW, Some(200))
        .unwrap();
    commit(&first_store, &mut other_a, a);
    commit(&second_store, &mut other_b, b);
    let persisted = first_store.load().unwrap();
    let shared = persisted.learning.item(&e4).unwrap();
    assert_eq!(shared.correct_count, 1);
    assert_eq!(shared.wrong_count, 2);
    assert!(persisted.session_snapshot("session-a").is_some());
    assert!(persisted.session_snapshot("session-b").is_some());
    assert!(
        persisted
            .learning
            .attempts
            .iter()
            .any(|attempt| attempt.client_attempt_id == "session-a-answer")
    );
    assert!(
        persisted
            .learning
            .attempts
            .iter()
            .any(|attempt| attempt.client_attempt_id == "session-b-answer")
    );
}

#[test]
fn snapshot_resumes_every_phase_with_deterministic_context_and_positions() {
    let (library, id) = library("1. e4 e5 2. Nf3 Nc6 *");
    let scope = scope(id, LearningSide::Black);
    let mut learning = LearningData::default();
    for uci in ["e7e5", "b8c6"] {
        introduce_at(&mut learning, key_for(&library, &scope, uci), NOW);
    }
    let dir = TestDir::new("resume");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("resume", &library, &learning, scope.clone());

    assert_eq!(session.phase, LearningSessionPhase::AdvancingContext);
    store.save_session(&mut session).unwrap();
    session = restore(&store, &library, "resume");
    let presentation = session.presentation(&library).unwrap();
    assert_eq!(presentation.target_path, ["e2e4", "e7e5"]);
    assert_eq!(presentation.automatic_context.len(), 1);
    assert_eq!(presentation.automatic_context[0].uci, "e2e4");
    assert_eq!(
        presentation.current_position.unwrap().fen4,
        presentation.automatic_context[0].from.fen4
    );

    advance(&store, &library, &mut session, "context-e4", NOW);
    session = restore(&store, &library, "resume");
    assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .current_position
            .unwrap()
            .fen4,
        key_for(&library, &scope, "e7e5").position_fen4
    );

    play(
        &store,
        &library,
        &mut session,
        "e7e6",
        "wrong-e6",
        NOW,
        Some(450),
    );
    session = restore(&store, &library, "resume");
    assert_eq!(
        session.phase,
        LearningSessionPhase::AwaitingReplayAfterError
    );
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .revealed_move
            .as_deref(),
        Some("e7e5")
    );

    play(
        &store,
        &library,
        &mut session,
        "e7e5",
        "replay-e5",
        NOW + 1,
        Some(999),
    );
    session = restore(&store, &library, "resume");
    assert_eq!(session.phase, LearningSessionPhase::AdvancingContext);
    assert_eq!(session.current_target().unwrap().key.expected_uci, "b8c6");
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .automatic_context
            .len(),
        3
    );

    for (index, uci) in ["e2e4", "e7e5", "g1f3"].into_iter().enumerate() {
        advance(
            &store,
            &library,
            &mut session,
            &format!("next-context-{index}"),
            NOW + 2,
        );
        session = restore(&store, &library, "resume");
        if index < 2 {
            assert_eq!(session.phase, LearningSessionPhase::AdvancingContext);
        }
        assert_eq!(session.current_target().unwrap().path[index], uci);
    }
    assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);

    let preview = session.preview("preview-nc6", NOW + 3).unwrap().unwrap();
    commit(&store, &mut session, preview);
    session = restore(&store, &library, "resume");
    assert_eq!(session.phase, LearningSessionPhase::PreviewedReplay);
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .revealed_move
            .as_deref(),
        Some("b8c6")
    );

    play(
        &store,
        &library,
        &mut session,
        "b8c6",
        "played-preview-nc6",
        NOW + 4,
        Some(888),
    );
    session = restore(&store, &library, "resume");
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    assert_eq!(session.completed_keys.len(), 2);
    assert!(
        session
            .presentation(&library)
            .unwrap()
            .current_position
            .is_none()
    );

    let data = store.load().unwrap();
    let e5 = data
        .learning
        .item(&key_for(&library, &scope, "e7e5"))
        .unwrap();
    assert_eq!(e5.independent_response_times_ms, [450]);
    let nc6 = data
        .learning
        .item(&key_for(&library, &scope, "b8c6"))
        .unwrap();
    assert!(nc6.independent_response_times_ms.is_empty());
    assert!(data.learning.attempts.iter().all(|attempt| {
        attempt.event_type.is_independent() || attempt.response_time_ms.is_none()
    }));
}

#[test]
fn both_and_legacy_queue_modes_are_rejected_on_resume() {
    let (library, id) = library("1. e4 *");
    let selected = scope(id, LearningSide::White);
    let mut session = due_session("invalid", &library, &LearningData::default(), selected);
    session.session.side = TrainingMode::Both;
    assert!(matches!(
        session.clone().resume(&library),
        Err(LearningSessionError::SessionSideMismatch)
    ));
    session.session.side = TrainingMode::White;
    session.session.mode = QueueMode::Practice;
    assert!(matches!(
        session.resume(&library),
        Err(LearningSessionError::UnsupportedMode(QueueMode::Practice))
    ));
}

#[test]
fn transposition_target_replays_its_saved_membership_path_after_reload() {
    let (library, id) =
        library("1. e4 d5 (1... Nf6 2. d4 d5) 2. d4 Nh6 3. Nf3 Ng8 4. Ng1 Nf6 5. c4 *");
    let scope = scope(id, LearningSide::White);
    let c4 = key_for(&library, &scope, "c2c4");
    let mut learning = LearningData::default();
    introduce_at(&mut learning, c4.clone(), NOW);
    let dir = TestDir::new("transposition");
    let store = dir.store();
    seed_store(&dir, &learning);
    let mut session = due_session("transposition", &library, &learning, scope);
    assert_eq!(
        session.current_target().unwrap().path,
        [
            "e2e4", "d7d5", "d2d4", "g8h6", "g1f3", "h6g8", "f3g1", "g8f6", "c2c4"
        ]
    );
    store.save_session(&mut session).unwrap();
    session = restore(&store, &library, "transposition");
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .automatic_context
            .len(),
        8
    );

    for index in 0..8 {
        advance(
            &store,
            &library,
            &mut session,
            &format!("transposition-context-{index}"),
            NOW,
        );
    }
    session = restore(&store, &library, "transposition");
    assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);
    assert_eq!(
        session
            .presentation(&library)
            .unwrap()
            .current_position
            .unwrap()
            .fen4,
        c4.position_fen4
    );
    play(
        &store,
        &library,
        &mut session,
        "c2c4",
        "transposition-answer",
        NOW,
        Some(100),
    );
    assert_eq!(session.phase, LearningSessionPhase::Finished);
}

#[test]
fn learnt_counter_tracks_persisted_independent_answers_for_both_sides_and_shared_scopes() {
    for side in [LearningSide::White, LearningSide::Black] {
        let (mut library, first) = library("1. e4 e5 *");
        let second = library
            .create(
                "Shared continuation",
                &chess_core::parse_str("1. e4 e5 2. Nf3 *").game,
            )
            .unwrap();
        let expected = if side == LearningSide::White {
            "e2e4"
        } else {
            "e7e5"
        };
        let wrong = if side == LearningSide::White {
            "d2d4"
        } else {
            "c7c5"
        };
        let key = key_for(&library, &scope(first.clone(), side), expected);
        let dir = TestDir::new("learnt-counter-lifecycle");
        let store = dir.store();
        let mut at = NOW;
        for (cycle, action) in ["correct", "correct", "preview", "correct", "wrong"]
            .into_iter()
            .enumerate()
        {
            let learning = store.load().unwrap().learning;
            let mut selected = scope(first.clone(), side);
            selected.as_of = at;
            let mut session = due_session(
                &format!("counter-{cycle}"),
                &library,
                &learning,
                selected.clone(),
            );
            if cycle == 0 {
                assert_eq!(session.introduce_new(&library, &learning).unwrap(), 1);
            }
            while session.phase == LearningSessionPhase::AdvancingContext {
                let event_id = format!("counter-{cycle}-auto-{}", session.context_index);
                advance(&store, &library, &mut session, &event_id, at);
            }
            assert_eq!(session.phase, LearningSessionPhase::AwaitingIndependent);
            if action == "preview" {
                let preview = session
                    .preview(format!("counter-{cycle}-preview"), at)
                    .unwrap()
                    .unwrap();
                commit(&store, &mut session, preview);
            }
            let played = if action == "wrong" { wrong } else { expected };
            let attempt = session
                .play_uci(
                    &library,
                    played,
                    format!("counter-{cycle}-answer"),
                    at,
                    Some(100),
                )
                .unwrap();
            let result = commit(&store, &mut session, attempt.clone());
            let duplicate = commit(&store, &mut session, attempt);
            assert_eq!(duplicate.application, AttemptApplication::Duplicate);
            assert_eq!(duplicate.learning, result.learning);
            if action == "wrong" {
                play(
                    &store,
                    &library,
                    &mut session,
                    expected,
                    "counter-replay",
                    at,
                    None,
                );
            }
            // Reload exactly what the UI uses after leaving a training session.
            let learning = store.load().unwrap().learning;
            let expected_streak = [1, 2, 2, 3, 0][cycle];
            let learnt = usize::from(expected_streak >= 3);
            assert_eq!(learning.item(&key).unwrap().correct_streak, expected_streak);
            let first_summary =
                aggregate_scope(&library, &learning, &selected, &SrsSettings::default())
                    .unwrap()
                    .summary;
            assert_eq!((first_summary.learnt, first_summary.total), (learnt, 1));
            let shared_scope = StatsScope::new([first.clone(), second.clone()], side, at);
            let shared =
                aggregate_scope(&library, &learning, &shared_scope, &SrsSettings::default())
                    .unwrap()
                    .summary;
            let expected_total = if side == LearningSide::White { 2 } else { 1 };
            assert_eq!((shared.learnt, shared.total), (learnt, expected_total));
            let second_summary = aggregate_scope(
                &library,
                &learning,
                &scope(second.clone(), side),
                &SrsSettings::default(),
            )
            .unwrap()
            .summary;
            assert_eq!(second_summary.learnt, learnt);
            assert_eq!(
                first_summary.total,
                first_summary.unseen + first_summary.learning + first_summary.learnt
            );
            assert_eq!(first_summary.accuracy_correct, [1, 2, 2, 3, 3][cycle]);
            assert_eq!(first_summary.accuracy_wrong, u64::from(action == "wrong"));
            at = learning.item(&key).unwrap().next_review_at.unwrap();
        }
    }
}

#[test]
fn runtime_replay_cache_is_shared_but_never_persisted_or_stale_after_source_changes() {
    let (mut library, id) = library("1. e4 e5 2. Nf3 Nc6 3. Bb5 *");
    let selected = scope(id.clone(), LearningSide::White);
    let mut learning = LearningData::default();
    introduce_at(&mut learning, key_for(&library, &selected, "f1b5"), NOW);
    let mut session = due_session("runtime-cache", &library, &learning, selected);
    let initial = session
        .presentation_with_learning(&library, &learning)
        .unwrap();
    let (parses, replays, _) = session.runtime_work_counts();
    assert_eq!((parses, replays), (1, 1));
    let mut clone = session.clone();
    for _ in 0..8 {
        assert_eq!(
            clone
                .presentation_with_learning(&library, &learning)
                .unwrap(),
            initial
        );
    }
    assert_eq!(clone.runtime_work_counts().0, 1);
    assert_eq!(clone.runtime_work_counts().1, 1);
    let json = serde_json::to_value(&session).unwrap();
    assert!(json.get("runtime").is_none());
    let mut restored: LearningSessionSnapshot = serde_json::from_value(json).unwrap();
    assert_eq!(restored, session);
    assert_eq!(restored.runtime_work_counts(), (0, 0, 0));
    assert_eq!(
        restored
            .presentation_with_learning(&library, &learning)
            .unwrap(),
        initial
    );
    assert_eq!(restored.runtime_work_counts().0, 1);

    // Public PGN can change without a revision API. Compare exact source bytes;
    // a shared clone must not retain old positions after a source edit.
    let old_library = library.clone();
    library
        .repertoire_mut(&id)
        .unwrap()
        .pgn
        .push_str(" {edited}");
    assert_eq!(
        clone
            .presentation_with_learning(&library, &learning)
            .unwrap(),
        initial
    );
    assert_eq!(session.runtime_work_counts().0, 2);
    assert_eq!(
        session
            .presentation_with_learning(&old_library, &learning)
            .unwrap(),
        initial
    );
    assert_eq!(session.runtime_work_counts().0, 3);
    library.repertoire_mut(&id).unwrap().pgn =
        "[SetUp \"1\"]\n[FEN \"rnbqkbnr/pppppppp/8/8/8/8/PPPP1PPP/RNBQKBNR w KQkq - 0 1\"]\n\n*"
            .into();
    assert!(matches!(
        clone.presentation_with_learning(&library, &learning),
        Err(LearningSessionError::InvalidTargetPath { .. })
    ));
    assert_eq!(clone.runtime_work_counts().0, 4);
    assert_eq!(
        session
            .presentation_with_learning(&old_library, &learning)
            .unwrap(),
        initial
    );
}

#[test]
fn context_presentation_plans_once_per_call_instead_of_once_per_prefix_move() {
    // A long legal path with repeated positions; the saved PGN occurrence must
    // retain its full history, while planning work stays constant per call.
    let pgn = format!("{} e4 *", "Nf3 Nf6 Ng1 Ng8 ".repeat(64));
    let (library, id) = library(&pgn);
    let selected = scope(id, LearningSide::White);
    let mut learning = LearningData::default();
    introduce_at(&mut learning, key_for(&library, &selected, "e2e4"), NOW);
    let mut session = due_session("linear-context-plan", &library, &learning, selected);
    assert_eq!(session.current_target().unwrap().path.len(), 257);
    let before = session.runtime_work_counts();
    let presentation = session
        .presentation_with_learning(&library, &learning)
        .unwrap();
    assert_eq!(presentation.automatic_context.len(), 256);
    let after = session.runtime_work_counts();
    assert_eq!((after.0, after.1), (before.0, before.1));
    // One phase synchronisation plus one presentation, independent of path length.
    assert_eq!(after.2 - before.2, 2);
    let mut clone = session.clone();
    for index in 0..8 {
        let attempt = clone
            .advance_context_with_learning(
                &library,
                &learning,
                format!("cached-context-{index}"),
                NOW,
            )
            .unwrap();
        assert_eq!(attempt.event_type, TrainingEventType::AutoContext);
    }
    assert_eq!(clone.context_index, 8);
    assert_eq!(clone.runtime_work_counts().0, 1);
    assert_eq!(clone.runtime_work_counts().1, 1);
    // Policy remains live; no cached plan may hide a newly required question.
    clone.context_policy.consider_leadup_moves = true;
    assert_eq!(
        clone
            .presentation_with_learning(&library, &learning)
            .unwrap()
            .phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
}
