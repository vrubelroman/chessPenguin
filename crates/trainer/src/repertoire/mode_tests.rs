use std::collections::BTreeSet;
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
            "cp-learning-modes-{}-{}-{name}",
            std::process::id(),
            sequence
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.join("learning.json")
    }

    fn store(&self) -> LearningStore {
        LearningStore::new(self.path())
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn add_repertoire(library: &mut Library, name: &str, pgn: &str) -> RepertoireId {
    library
        .create(name, &chess_core::parse_str(pgn).game)
        .unwrap()
}

fn single_repertoire(pgn: &str) -> (Library, RepertoireId) {
    let mut library = Library::default();
    let id = add_repertoire(&mut library, "Test", pgn);
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

fn mode_session(
    id: &str,
    library: &Library,
    learning: &LearningData,
    scope: StatsScope,
    mode: QueueMode,
) -> LearningSessionSnapshot {
    LearningSessionSnapshot::new_with_mode(
        id,
        library,
        learning,
        scope,
        SrsSettings::default(),
        mode,
    )
    .unwrap()
}

fn queue_labels(session: &LearningSessionSnapshot) -> Vec<String> {
    session
        .queue
        .iter()
        .map(|target| format!("{}@{}", target.key.expected_uci, target.min_depth))
        .collect()
}

fn apply(learning: &mut LearningData, attempt: TrainingAttempt) {
    assert_eq!(
        learning.apply_attempt(attempt, &SrsSettings::default()),
        Ok(AttemptApplication::Applied)
    );
}

fn complete_target(
    session: &mut LearningSessionSnapshot,
    learning: &mut LearningData,
    library: &Library,
    id_prefix: &str,
    happened_at: i64,
) {
    let original_index = session.current_index;
    let original_cycle = session.cycle;
    let original_key = session.current_target().unwrap().key.clone();
    for step in 0..32 {
        match session.phase {
            LearningSessionPhase::AdvancingContext => {
                let attempt = session
                    .advance_context(
                        library,
                        format!("{id_prefix}-{original_cycle}-{original_index}-context-{step}"),
                        happened_at,
                    )
                    .unwrap();
                apply(learning, attempt);
            }
            LearningSessionPhase::AwaitingIndependent
            | LearningSessionPhase::AwaitingContextIndependent => {
                let expected = session.current_expected_uci().unwrap().to_owned();
                let attempt = session
                    .play_uci(
                        library,
                        &expected,
                        format!("{id_prefix}-{original_cycle}-{original_index}-answer-{step}"),
                        happened_at,
                        Some(100 + step),
                    )
                    .unwrap();
                apply(learning, attempt);
            }
            phase => panic!("unexpected phase while completing target: {phase:?}"),
        }
        if session.phase == LearningSessionPhase::Finished
            || session.current_index != original_index
            || session.cycle != original_cycle
            || session
                .current_target()
                .is_none_or(|target| target.key != original_key)
        {
            return;
        }
    }
    panic!("target did not complete");
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

fn introduced_at(learning: &mut LearningData, key: LearningItemKey, last_review_at: i64) {
    let item = learning.ensure_item(key);
    item.introduced = true;
    item.correct_streak = 1;
    item.correct_count = 1;
    item.last_review_at = Some(last_review_at);
    item.last_correct_at = Some(last_review_at);
    item.next_review_at = Some(NOW + 10 * DAY);
    item.previous_interval_seconds = Some(DAY);
}

fn journal_attempt(key: &LearningItemKey, id: &str, happened_at: i64) -> TrainingAttempt {
    TrainingAttempt {
        user_id: key.user_id.clone(),
        position_fen4: key.position_fen4.clone(),
        client_attempt_id: id.into(),
        session_id: "at22".into(),
        event_type: TrainingEventType::CorrectIndependent,
        expected_uci: key.expected_uci.clone(),
        actual_uci: Some(key.expected_uci.clone()),
        response_time_ms: Some(200),
        happened_at,
    }
}

#[test]
fn learning_priorities_use_pgn_order_depth_rav_and_deterministic_bfs_fallback() {
    let (library, id) = single_repertoire(
        "1. e4 e5 (1... c5 2. c4 Nc6 (2... e6 3. g3)) (1... Nf6 2. d4) 2. Nf3 Nc6 *",
    );
    let selected = scope(id, LearningSide::White);
    let learning = LearningData::default();
    let order = |priority| {
        queue_labels(&mode_session(
            "priority",
            &library,
            &learning,
            selected.clone().with_learning_priority(priority),
            QueueMode::ReviewInOrder,
        ))
    };

    assert_eq!(
        order(LearningPriority::DepthFirst),
        ["e2e4@1", "g1f3@3", "c2c4@3", "g2g3@5", "d2d4@3"]
    );
    assert_eq!(
        order(LearningPriority::BreadthOverDepth),
        ["e2e4@1", "g1f3@3", "c2c4@3", "d2d4@3", "g2g3@5"]
    );
    assert_eq!(
        order(LearningPriority::MainLinesFirst),
        ["e2e4@1", "g1f3@3", "c2c4@3", "d2d4@3", "g2g3@5"]
    );
    assert_eq!(
        order(LearningPriority::CommonPositionsFirst),
        order(LearningPriority::BreadthOverDepth)
    );

    let memberships = active_learning_memberships(&library, &selected).unwrap();
    let nested = memberships
        .iter()
        .find(|membership| membership.key.expected_uci == "g2g3")
        .unwrap();
    assert_eq!(nested.min_depth, 5);
    assert_eq!(nested.variation_depth, 2);
    assert!(
        memberships
            .iter()
            .all(|membership| { membership.pgn_order > 0 })
    );
    assert!(matches!(
        LearningSessionSnapshot::new_with_mode(
            "practice",
            &library,
            &learning,
            selected,
            SrsSettings::default(),
            QueueMode::Practice,
        ),
        Err(LearningSessionError::UnsupportedMode(QueueMode::Practice))
    ));
}

#[test]
fn at_20_review_in_order_covers_each_key_once_updates_srs_and_wraps() {
    let (library, id) = single_repertoire("1. e4 e5 (1... c5 2. c4) 2. Nf3 Nc6 *");
    let selected = scope(id, LearningSide::White);
    let mut learning = LearningData::default();
    let mut session = mode_session(
        "at20",
        &library,
        &learning,
        selected,
        QueueMode::ReviewInOrder,
    );
    let expected = ["e2e4", "g1f3", "c2c4"];
    assert_eq!(
        session
            .queue
            .iter()
            .map(|target| target.key.expected_uci.as_str())
            .collect::<Vec<_>>(),
        expected
    );

    let mut first_cycle = Vec::new();
    for expected_uci in expected {
        let current = session.current_target().unwrap().key.clone();
        assert_eq!(current.expected_uci, expected_uci);
        assert!(!session.completed_keys.contains(&current));
        first_cycle.push(current.clone());
        complete_target(&mut session, &mut learning, &library, "at20-first", NOW);
        assert_eq!(learning.item(&current).unwrap().correct_count, 1);
        assert_eq!(learning.item(&current).unwrap().correct_streak, 1);
        assert_eq!(
            learning.item(&current).unwrap().next_review_at,
            Some(NOW + DAY)
        );
    }
    assert_eq!(
        first_cycle.len(),
        first_cycle.iter().collect::<BTreeSet<_>>().len()
    );
    assert_eq!(session.cycle, 1);
    assert!(session.completed_keys.is_empty());
    assert_eq!(session.current_target().unwrap().key.expected_uci, "e2e4");

    let e4 = session.current_target().unwrap().key.clone();
    complete_target(
        &mut session,
        &mut learning,
        &library,
        "at20-second",
        NOW + DAY,
    );
    assert_eq!(learning.item(&e4).unwrap().correct_count, 2);
    assert_eq!(learning.item(&e4).unwrap().correct_streak, 2);
    assert_eq!(session.cycle, 1);
    assert!(session.completed_keys.contains(&e4));
    assert!(
        !session
            .completed_keys
            .contains(&session.current_target().unwrap().key)
    );
}

#[test]
fn review_cursor_resumes_mid_cycle_and_duplicate_event_does_not_advance_twice() {
    let (library, id) = single_repertoire("1. e4 e5 2. Nf3 Nc6 *");
    let selected = scope(id, LearningSide::White);
    let learning = LearningData::default();
    let dir = TestDir::new("review-resume");
    let store = dir.store();
    let mut session = mode_session(
        "review-resume",
        &library,
        &learning,
        selected,
        QueueMode::ReviewInOrder,
    );
    let attempt = session
        .play_uci(&library, "e2e4", "review-first", NOW, Some(100))
        .unwrap();
    commit(&store, &mut session, attempt.clone());
    let bytes = std::fs::read(dir.path()).unwrap();

    let restored = store
        .load()
        .unwrap()
        .session_snapshot("review-resume")
        .cloned()
        .unwrap()
        .resume(&library)
        .unwrap();
    assert_eq!(restored.current_index, 1);
    assert_eq!(restored.cycle, 0);
    assert_eq!(restored.completed_keys, session.completed_keys);
    assert_eq!(restored.current_target().unwrap().key.expected_uci, "g1f3");

    let mut duplicate_next = restored.clone();
    duplicate_next.current_index = 0;
    duplicate_next.completed_keys.clear();
    duplicate_next.cycle = 9;
    let duplicate = store
        .apply_attempt_and_session(attempt, &mut duplicate_next)
        .unwrap();
    assert_eq!(duplicate.application, AttemptApplication::Duplicate);
    assert_eq!(duplicate.session, restored);
    assert_eq!(duplicate.learning.attempts.len(), 1);
    assert_eq!(std::fs::read(dir.path()).unwrap(), bytes);
}

#[test]
fn at_21_variation_wrong_waits_for_the_next_due_pass_not_less_than_thirty_seconds() {
    let (library, id) = single_repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *");
    let selected = scope(id, LearningSide::White);
    let mut learning = LearningData::default();
    for expected in ["e2e4", "g1f3", "f1b5"] {
        let key = key_for(&library, &selected, expected);
        introduced_at(&mut learning, key.clone(), NOW - DAY);
        learning.ensure_item(key).next_review_at = Some(NOW);
    }
    let e4 = key_for(&library, &selected, "e2e4");
    let mut session = LearningSessionSnapshot::new(
        "at21",
        &library,
        &learning,
        selected.clone(),
        SrsSettings::default(),
    )
    .unwrap();
    let original_queue = session.queue.clone();

    let wrong = session
        .play_uci(&library, "d2d4", "at21-wrong", NOW, Some(400))
        .unwrap();
    apply(&mut learning, wrong);
    assert_eq!(learning.item(&e4).unwrap().next_review_at, Some(NOW + 30));
    assert_eq!(session.queue, original_queue);
    assert_eq!(session.current_target().unwrap().key, e4);
    let replay = session
        .play_uci(&library, "e2e4", "at21-replay", NOW + 1, Some(900))
        .unwrap();
    apply(&mut learning, replay);
    assert_eq!(session.queue, original_queue);
    assert_eq!(session.current_target().unwrap().key.expected_uci, "g1f3");
    assert_eq!(
        session
            .queue
            .iter()
            .filter(|target| target.key == e4)
            .count(),
        1
    );

    complete_target(&mut session, &mut learning, &library, "at21-nf3", NOW + 2);
    complete_target(&mut session, &mut learning, &library, "at21-bb5", NOW + 3);
    assert_eq!(session.phase, LearningSessionPhase::Finished);

    session.stats_scope.as_of = NOW + 29;
    assert_eq!(session.start_next_pass(&library, &learning).unwrap(), 0);
    session.stats_scope.as_of = NOW + 30;
    assert_eq!(session.start_next_pass(&library, &learning).unwrap(), 1);
    assert_eq!(session.current_target().unwrap().key, e4);
}

#[test]
fn least_recent_prioritizes_unseen_then_oldest_and_skips_early_checked_context() {
    let (library, id) = single_repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *");
    let selected = scope(id, LearningSide::White);
    let e4 = key_for(&library, &selected, "e2e4");
    let nf3 = key_for(&library, &selected, "g1f3");
    let bb5 = key_for(&library, &selected, "f1b5");
    let mut learning = LearningData::default();
    introduced_at(&mut learning, e4.clone(), NOW - 10);
    introduced_at(&mut learning, nf3.clone(), NOW - 100);

    let unseen_first = mode_session(
        "least-unseen",
        &library,
        &learning,
        selected.clone(),
        QueueMode::LeastRecent,
    );
    assert_eq!(
        unseen_first
            .queue
            .iter()
            .map(|target| target.key.expected_uci.as_str())
            .collect::<Vec<_>>(),
        ["f1b5", "g1f3", "e2e4"]
    );

    introduced_at(&mut learning, bb5.clone(), NOW - 50);
    let mut session = mode_session(
        "least-context",
        &library,
        &learning,
        selected,
        QueueMode::LeastRecent,
    );
    assert_eq!(
        session
            .queue
            .iter()
            .map(|target| target.key.expected_uci.as_str())
            .collect::<Vec<_>>(),
        ["g1f3", "f1b5", "e2e4"]
    );
    assert_eq!(
        session.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    assert_eq!(session.current_expected_uci(), Some("e2e4"));

    complete_target(&mut session, &mut learning, &library, "least-nf3", NOW);
    assert!(session.completed_keys.contains(&e4));
    assert!(session.completed_keys.contains(&nf3));
    complete_target(&mut session, &mut learning, &library, "least-bb5", NOW + 1);
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    assert_eq!(session.completed_keys.len(), 3);
    for key in [e4, nf3, bb5] {
        let independent = learning
            .attempts
            .iter()
            .filter(|attempt| attempt.item_key() == key && attempt.event_type.is_independent())
            .count();
        assert_eq!(independent, 1, "duplicate review for {key:?}");
    }
}

#[test]
fn at_22_shared_reset_preserves_cursor_journal_extensions_and_review_reset_preserves_srs() {
    let mut library = Library::default();
    let first = add_repertoire(&mut library, "First", "1. e4 e5 2. Nf3 *");
    let second = add_repertoire(&mut library, "Second", "1. e4 c5 2. c4 *");
    let first_scope = scope(first, LearningSide::White);
    let second_scope = scope(second, LearningSide::White);
    let shared = key_for(&library, &first_scope, "e2e4");
    let nf3 = key_for(&library, &first_scope, "g1f3");
    let second_only = key_for(&library, &second_scope, "c2c4");
    let mut learning = LearningData::default();
    introduced_at(&mut learning, shared.clone(), NOW - 3 * DAY);
    introduced_at(&mut learning, nf3.clone(), NOW - 2 * DAY);
    introduced_at(&mut learning, second_only.clone(), NOW - DAY);
    learning
        .ensure_item(shared.clone())
        .independent_response_times_ms
        .push(500);
    learning
        .attempts
        .push(journal_attempt(&shared, "at22-old", NOW - DAY));
    let second_before = learning.item(&second_only).unwrap().clone();
    let pgn_before: Vec<_> = library
        .repertoires
        .iter()
        .map(|repertoire| repertoire.pgn.clone())
        .collect();

    let impact = reset_impact(&library, &first_scope).unwrap();
    assert_eq!((impact.reset_count, impact.shared_count), (2, 1));
    assert_eq!(
        impact
            .affected_repertoires
            .iter()
            .map(|repertoire| repertoire.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["First", "Second"])
    );

    let dir = TestDir::new("shared-reset");
    let store = dir.store();
    std::fs::write(
        dir.path(),
        serde_json::to_vec_pretty(&LearningStoreData {
            learning,
            ..LearningStoreData::default()
        })
        .unwrap(),
    )
    .unwrap();
    let mut review = mode_session(
        "at22",
        &library,
        &store.load().unwrap().learning,
        first_scope.clone(),
        QueueMode::ReviewInOrder,
    );
    let review_attempt = review
        .play_uci(&library, "e2e4", "at22-review", NOW, Some(250))
        .unwrap();
    commit(&store, &mut review, review_attempt);
    let cursor_before = store
        .load()
        .unwrap()
        .session_snapshot("at22")
        .cloned()
        .unwrap();
    assert_eq!(cursor_before.current_index, 1);
    assert_eq!(cursor_before.completed_keys.len(), 1);

    let marker = serde_json::json!({"preserved": [1, {"nested": true}]});
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path()).unwrap()).unwrap();
    raw["future_root"] = marker.clone();
    raw["learning"]["future_learning"] = marker.clone();
    raw["learning"]["attempts"][0]["future_attempt"] = marker.clone();
    raw["session_snapshots"][0]["future_snapshot"] = marker.clone();
    raw["session_snapshots"][0]["queue"][0]["future_target"] = marker.clone();
    raw["legacy_practice"] = serde_json::json!({"raw_legacy": marker.clone()});
    let shared_index = raw["learning"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .position(|item| {
            item["key"]["position_fen4"] == serde_json::json!(shared.position_fen4.clone())
                && item["key"]["expected_uci"] == serde_json::json!("e2e4")
        })
        .unwrap();
    raw["learning"]["items"][shared_index]["future_item"] = marker.clone();
    std::fs::write(dir.path(), serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    #[cfg(unix)]
    let old_path = dir.0.join("before-reset.json");
    #[cfg(unix)]
    std::fs::hard_link(dir.path(), &old_path).unwrap();

    let reset = store
        .reset_learning(active_learning_keys(&library, &first_scope).unwrap())
        .unwrap();
    assert_eq!(reset.reset_count, 2);
    assert_eq!(
        reset.learning.item(&shared),
        Some(&LearningItem::new(shared.clone()))
    );
    assert_eq!(
        reset.learning.item(&nf3),
        Some(&LearningItem::new(nf3.clone()))
    );
    assert_eq!(reset.learning.item(&second_only), Some(&second_before));
    assert!(reset.learning.item(&shared).unwrap().is_unseen());
    assert!(reset.learning.item(&nf3).unwrap().is_unseen());
    assert_eq!(reset.learning.attempts.len(), 2);
    assert_eq!(
        store.load().unwrap().session_snapshot("at22").unwrap(),
        &cursor_before
    );
    let second_summary = aggregate_scope(
        &library,
        &reset.learning,
        &second_scope,
        &SrsSettings::default(),
    )
    .unwrap();
    assert!(
        second_summary
            .items
            .iter()
            .find(|item| item.key == shared)
            .unwrap()
            .is_unseen()
    );
    assert_eq!(
        library
            .repertoires
            .iter()
            .map(|repertoire| repertoire.pgn.clone())
            .collect::<Vec<_>>(),
        pgn_before
    );

    let persisted_raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path()).unwrap()).unwrap();
    for pointer in [
        "/future_root",
        "/learning/future_learning",
        "/learning/attempts/0/future_attempt",
        "/session_snapshots/0/future_snapshot",
        "/session_snapshots/0/queue/0/future_target",
        "/legacy_practice/raw_legacy",
    ] {
        assert_eq!(persisted_raw.pointer(pointer), Some(&marker), "{pointer}");
    }
    assert_eq!(
        persisted_raw["learning"]["items"][shared_index]["future_item"],
        marker
    );
    #[cfg(unix)]
    {
        let old = LearningStore::new(old_path).load().unwrap();
        assert!(old.learning.item(&shared).unwrap().introduced);
        assert_eq!(old.session_snapshot("at22"), Some(&cursor_before));
    }

    let duplicate = store
        .apply_attempt(
            journal_attempt(&shared, "at22-review", NOW),
            &SrsSettings::default(),
        )
        .unwrap();
    assert_eq!(duplicate.application, AttemptApplication::Duplicate);
    assert!(duplicate.learning.item(&shared).unwrap().is_unseen());
    let fresh = store
        .apply_attempt(
            journal_attempt(&shared, "at22-fresh", NOW + DAY),
            &SrsSettings::default(),
        )
        .unwrap();
    let fresh_shared = fresh.learning.item(&shared).unwrap();
    assert_eq!(fresh_shared.correct_streak, 1);
    assert_eq!(fresh_shared.correct_count, 1);
    assert_eq!(fresh_shared.wrong_count, 0);
    assert_eq!(fresh_shared.accuracy_percent(), Some(100.0));
    assert_eq!(fresh_shared.independent_response_times_ms, [200]);
    assert_eq!(fresh.learning.attempts.len(), 3);

    let before_review_reset = store.load().unwrap().learning;
    store.reset_review_order(&mut review).unwrap();
    assert_eq!(review.current_index, 0);
    assert_eq!(review.cycle, 0);
    assert!(review.completed_keys.is_empty());
    assert_eq!(store.load().unwrap().learning, before_review_reset);
}

#[test]
fn old_scope_and_mode_snapshot_defaults_remain_deserializable() {
    let (library, id) = single_repertoire("1. e4 *");
    let selected = scope(id, LearningSide::White);
    let session = mode_session(
        "defaults",
        &library,
        &LearningData::default(),
        selected,
        QueueMode::ReviewInOrder,
    );
    let mut raw = serde_json::to_value(session).unwrap();
    raw.as_object_mut().unwrap().remove("cycle");
    raw["stats_scope"]
        .as_object_mut()
        .unwrap()
        .remove("learning_priority");
    for field in ["pgn_order", "variation_depth"] {
        raw["queue"][0].as_object_mut().unwrap().remove(field);
    }

    let restored: LearningSessionSnapshot = serde_json::from_value(raw).unwrap();
    assert_eq!(restored.cycle, 0);
    assert_eq!(
        restored.stats_scope.learning_priority,
        LearningPriority::DepthFirst
    );
    assert_eq!(restored.queue[0].pgn_order, 0);
    assert_eq!(restored.queue[0].variation_depth, 0);
    restored.resume(&library).unwrap();
}
