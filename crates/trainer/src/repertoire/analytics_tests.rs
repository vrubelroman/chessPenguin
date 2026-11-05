use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

const DAY: i64 = 24 * 60 * 60;
const BASE: i64 = 1_800_000_000;

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cp-learning-analytics-{}-{}-{name}",
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

fn item_key(index: usize) -> LearningItemKey {
    LearningItemKey::local(format!("position-{index} w - -"), "e2e4")
}

fn attempt(
    key: &LearningItemKey,
    id: impl Into<String>,
    event_type: TrainingEventType,
    happened_at: i64,
    response_time_ms: u64,
) -> TrainingAttempt {
    let actual_uci = match event_type {
        TrainingEventType::IncorrectIndependent => "d2d4",
        _ => "e2e4",
    };
    TrainingAttempt {
        user_id: key.user_id.clone(),
        position_fen4: key.position_fen4.clone(),
        client_attempt_id: id.into(),
        session_id: "analytics".into(),
        event_type,
        expected_uci: key.expected_uci.clone(),
        actual_uci: Some(actual_uci.into()),
        response_time_ms: Some(response_time_ms),
        happened_at,
    }
}

fn record(item: &mut LearningItem, id: &str, event_type: TrainingEventType, happened_at: i64) {
    apply_attempt(
        item,
        &attempt(&item.key, id, event_type, happened_at, 250),
        &SrsSettings::default(),
    )
    .unwrap();
}

fn learnt_item(index: usize, reviewed_at: i64, next_review_at: i64) -> LearningItem {
    let mut item = LearningItem::new(item_key(index));
    item.introduced = true;
    item.correct_streak = 3;
    item.correct_count = 3;
    item.last_review_at = Some(reviewed_at);
    item.last_correct_at = Some(reviewed_at);
    item.last_independent_at = Some(reviewed_at);
    item.last_independent_correct_at = Some(reviewed_at);
    item.last_independent_result = Some(IndependentResult::Correct);
    item.next_review_at = Some(next_review_at);
    item.previous_interval_seconds = Some(DAY);
    item
}

fn introduced_item(index: usize, next_review_at: Option<i64>) -> LearningItem {
    let mut item = LearningItem::new(item_key(index));
    item.introduced = true;
    item.next_review_at = next_review_at;
    item
}

#[test]
fn resistant_enters_on_recent_results_and_exits_after_three_independent_correct() {
    let mut item = LearningItem::new(item_key(0));
    for (index, event_type) in [
        TrainingEventType::IncorrectIndependent,
        TrainingEventType::CorrectIndependent,
        TrainingEventType::IncorrectIndependent,
        TrainingEventType::CorrectIndependent,
        TrainingEventType::IncorrectIndependent,
    ]
    .into_iter()
    .enumerate()
    {
        record(
            &mut item,
            &format!("enter-{index}"),
            event_type,
            BASE + index as i64,
        );
    }

    assert!(is_resistant(&item));
    assert!(item.is_learning());
    let summary = aggregate(
        std::slice::from_ref(&item),
        BASE + 5,
        &SrsSettings::default(),
    );
    assert_eq!((summary.total, summary.learning, summary.learnt), (1, 1, 0));

    for index in 0..2 {
        record(
            &mut item,
            &format!("exit-{index}"),
            TrainingEventType::CorrectIndependent,
            BASE + 5 + index,
        );
        assert!(item.is_resistant());
    }
    record(
        &mut item,
        "exit-2",
        TrainingEventType::CorrectIndependent,
        BASE + 7,
    );
    assert!(!item.is_resistant());
    assert!(item.is_learnt());
    record(
        &mut item,
        "exit-3",
        TrainingEventType::CorrectIndependent,
        BASE + 8,
    );
    assert!(!item.is_resistant());

    for index in 0..6 {
        record(
            &mut item,
            &format!("recent-{index}"),
            TrainingEventType::CorrectIndependent,
            BASE + 9 + index,
        );
    }
    assert_eq!(item.recent_independent_outcomes.len(), 10);
    assert!(
        item.recent_independent_outcomes
            .iter()
            .all(|outcome| outcome.result == IndependentResult::Correct)
    );
    assert!(!item.is_resistant());
    assert_eq!(item.wrong_count, 3);
}

#[test]
fn non_independent_events_preserve_independent_timestamps_history_and_median() {
    let mut item = LearningItem::new(item_key(0));
    for (index, response_time_ms) in [400, 100, 300, 200].into_iter().enumerate() {
        let event_type = if index == 0 {
            TrainingEventType::IncorrectIndependent
        } else {
            TrainingEventType::CorrectIndependent
        };
        let training_attempt = attempt(
            &item.key,
            format!("independent-{index}"),
            event_type,
            BASE + index as i64,
            response_time_ms,
        );
        apply_attempt(&mut item, &training_attempt, &SrsSettings::default()).unwrap();
    }
    assert_eq!(item.response_time_median_ms(), Some(250.0));
    assert_eq!(item.last_independent_at, Some(BASE + 3));
    assert_eq!(item.last_independent_correct_at, Some(BASE + 3));
    assert_eq!(item.last_independent_wrong_at, Some(BASE));
    assert_eq!(
        item.last_independent_result,
        Some(IndependentResult::Correct)
    );

    for (index, event_type) in [
        TrainingEventType::Previewed,
        TrainingEventType::ReplayAfterError,
        TrainingEventType::AutoContext,
        TrainingEventType::IllegalMove,
    ]
    .into_iter()
    .enumerate()
    {
        let before_outcomes = item.recent_independent_outcomes.clone();
        let before_responses = item.independent_response_times_ms.clone();
        let training_attempt = attempt(
            &item.key,
            format!("context-{index}"),
            event_type,
            BASE + 10 + index as i64,
            99_999,
        );
        apply_attempt(&mut item, &training_attempt, &SrsSettings::default()).unwrap();
        assert_eq!(item.last_independent_at, Some(BASE + 3));
        assert_eq!(item.last_independent_correct_at, Some(BASE + 3));
        assert_eq!(item.last_independent_wrong_at, Some(BASE));
        assert_eq!(
            item.last_independent_result,
            Some(IndependentResult::Correct)
        );
        assert_eq!(item.recent_independent_outcomes, before_outcomes);
        assert_eq!(item.independent_response_times_ms, before_responses);
        assert_eq!(item.response_time_median_ms(), Some(250.0));
    }
}

#[test]
fn at_09_uncertain_is_subset_of_learnt_not_an_extra_total() {
    let item = learnt_item(0, BASE - DAY, BASE);
    let now = BASE + 7 * DAY + 1;
    assert!(is_uncertain(&item, now, &SrsSettings::default()));
    let summary = aggregate(std::slice::from_ref(&item), now, &SrsSettings::default());
    assert_eq!(summary.total, 1);
    assert_eq!(summary.learnt, 1);
    assert_eq!(summary.uncertain, 1);
    assert_eq!(
        summary.total,
        summary.unseen + summary.learning + summary.learnt
    );
    let details = move_learning_analytics(&item, now, &SrsSettings::default());
    assert_eq!(details.status, MoveLearningStatus::Uncertain);
    assert_eq!(details.uncertain_grace_seconds, Some(7 * DAY));
}

#[test]
fn at_10_independent_correct_clears_uncertain_and_stays_learnt() {
    let mut item = learnt_item(0, BASE - DAY, BASE);
    let now = BASE + 7 * DAY + 1;
    apply_independent_correct(&mut item, now, &SrsSettings::default());
    assert!(item.is_learnt());
    assert!(!is_uncertain(&item, now, &SrsSettings::default()));
    assert_eq!(item.last_correct_at, Some(now));
    assert_eq!(item.last_independent_correct_at, Some(now));
    assert!(item.next_review_at.is_some_and(|next| next > now));
}

#[test]
fn at_11_independent_wrong_clears_uncertain_by_returning_to_learning() {
    let mut item = learnt_item(0, BASE - DAY, BASE);
    let previous_independent_correct = item.last_independent_correct_at;
    let now = BASE + 7 * DAY + 1;
    apply_independent_wrong(&mut item, "d2d4", now, &SrsSettings::default());
    assert!(item.is_learning());
    assert!(!is_uncertain(&item, now, &SrsSettings::default()));
    assert_eq!(item.correct_streak, 0);
    assert_eq!(item.last_correct_at, None);
    assert_eq!(
        item.last_independent_correct_at,
        previous_independent_correct
    );
    assert_eq!(item.last_independent_wrong_at, Some(now));
    assert_eq!(item.last_independent_result, Some(IndependentResult::Wrong));
    assert_eq!(item.next_review_at, Some(now + MISTAKE_DELAY_SECONDS));
}

#[test]
fn at_26_scheduled_boundaries_are_disjoint_and_cover_every_introduced_item() {
    let offset = LocalDatePolicy::fixed(2 * 60 * 60);
    let end_today = offset.end_of_local_day_exclusive(BASE);
    assert!(BASE + 1 < end_today);
    let end_week = BASE + SCHEDULED_WEEK_SECONDS;
    let end_month = BASE + SCHEDULED_MONTH_SECONDS;
    let end_year = BASE + SCHEDULED_YEAR_SECONDS;
    let mut items = vec![
        introduced_item(0, Some(BASE)),
        introduced_item(1, Some(BASE + 1)),
        introduced_item(2, Some(end_today)),
        introduced_item(3, Some(end_week - 1)),
        introduced_item(4, Some(end_week)),
        introduced_item(5, Some(end_month)),
        introduced_item(6, Some(end_year)),
        introduced_item(7, None),
        LearningItem::new(item_key(8)),
    ];
    items[8].next_review_at = Some(BASE);

    let buckets = scheduled_buckets(items.iter(), BASE, offset);
    assert_eq!(buckets.overdue, 2);
    assert_eq!(buckets.today, 1);
    assert_eq!(buckets.week, 2);
    assert_eq!(buckets.month, 1);
    assert_eq!(buckets.year, 1);
    assert_eq!(buckets.later, 1);
    assert_eq!(buckets.introduced(), 8);
}

#[test]
fn local_date_policy_makes_offset_and_midnight_explicit() {
    assert_eq!(LocalDatePolicy::UTC.date(0), "1970-01-01");
    let east = LocalDatePolicy::fixed(3_600);
    assert_eq!(east.date(0), "1970-01-01");
    assert_eq!(east.end_of_local_day_exclusive(0), 23 * 3_600);
    let west = LocalDatePolicy::fixed(-3_600);
    assert_eq!(west.date(0), "1969-12-31");
    assert_eq!(west.end_of_local_day_exclusive(0), 3_600);
}

#[test]
fn sequences_include_zero_and_attempt_histogram_sums_match_total() {
    let unseen = LearningItem::new(item_key(0));
    let mut learning = introduced_item(1, Some(BASE));
    learning.correct_streak = 2;
    learning.correct_count = 2;
    let mut wrong = introduced_item(2, Some(BASE));
    wrong.wrong_streak = 3;
    wrong.wrong_count = 3;
    let mut learnt = learnt_item(3, BASE - DAY, BASE);
    learnt.correct_streak = 5;
    learnt.correct_count = 5;
    learnt.wrong_count = 2;
    let items = [unseen, learning, wrong, learnt];

    let histograms = sequence_histograms(items.iter());
    assert_eq!(histograms.correct_streak.get(&0), Some(&2));
    assert_eq!(histograms.correct_streak.get(&2), Some(&1));
    assert_eq!(histograms.correct_streak.get(&5), Some(&1));
    assert_eq!(histograms.wrong_streak.get(&0), Some(&3));
    assert_eq!(histograms.wrong_streak.get(&3), Some(&1));
    assert_eq!(histograms.attempts.get(&0), Some(&1));
    assert_eq!(histograms.attempts.get(&2), Some(&1));
    assert_eq!(histograms.attempts.get(&3), Some(&1));
    assert_eq!(histograms.attempts.get(&7), Some(&1));
    assert_eq!(
        histograms.correct_streak.values().sum::<usize>(),
        items.len()
    );
    assert_eq!(histograms.wrong_streak.values().sum::<usize>(), items.len());
    assert_eq!(histograms.attempts.values().sum::<usize>(), items.len());
}

#[test]
fn per_move_analytics_include_independent_details_and_scope_membership() {
    let mut library = Library::default();
    let first = library
        .create("First", &chess_core::parse_str("1. e4 e5 2. Nf3 *").game)
        .unwrap();
    let second = library
        .create("Second", &chess_core::parse_str("1. e4 c5 *").game)
        .unwrap();
    let first_scope = StatsScope::for_repertoire(first.clone(), LearningSide::White, BASE);
    let key = active_learning_memberships(&library, &first_scope)
        .unwrap()
        .into_iter()
        .find(|membership| membership.key.expected_uci == "e2e4")
        .unwrap()
        .key;
    let mut learning = LearningData::default();
    for (index, response_time_ms) in [400, 100, 300, 200].into_iter().enumerate() {
        let event_type = if index == 0 {
            TrainingEventType::IncorrectIndependent
        } else {
            TrainingEventType::CorrectIndependent
        };
        learning
            .apply_attempt(
                attempt(
                    &key,
                    format!("details-{index}"),
                    event_type,
                    BASE - 3 + index as i64,
                    response_time_ms,
                ),
                &SrsSettings::default(),
            )
            .unwrap();
    }
    let scope = StatsScope::new([first, second], LearningSide::White, BASE + 1);
    let details =
        scoped_move_learning_analytics(&library, &learning, &scope, &SrsSettings::default())
            .unwrap();
    let e4 = details
        .iter()
        .find(|details| details.key.expected_uci == "e2e4")
        .unwrap();
    assert_eq!(e4.status, MoveLearningStatus::LearntFresh);
    assert_eq!(e4.memberships.len(), 2);
    assert_eq!(e4.accuracy_percent, Some(75.0));
    assert_eq!(e4.independent_attempts, 4);
    assert_eq!(e4.response_time_median_ms, Some(250.0));
    assert_eq!(e4.last_review_at, Some(BASE));
    assert_eq!(e4.last_independent_at, Some(BASE));
    assert_eq!(e4.last_independent_wrong_at, Some(BASE - 3));
    assert_eq!(e4.last_independent_result, Some(IndependentResult::Correct));
    assert_eq!(e4.wrong_move_histogram["d2d4"].count, 1);
}

#[test]
fn at_27_daily_snapshots_capture_time_dependent_uncertain_without_rewriting_history() {
    let dir = TestDir::new("snapshots");
    let path = dir.join("learning.json");
    let store = LearningStore::new(&path);
    let mut library = Library::default();
    let repertoire = library
        .create("Repertoire", &chess_core::parse_str("1. e4 *").game)
        .unwrap();
    let first_scope = StatsScope::for_repertoire(repertoire, LearningSide::White, BASE);
    let key = active_learning_keys(&library, &first_scope)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let mut learning = LearningData::default();
    let mut learnt = learnt_item(0, BASE - DAY, BASE);
    learnt.key = key.clone();
    *learning.ensure_item(key) = learnt;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&LearningStoreData {
            learning,
            ..LearningStoreData::default()
        })
        .unwrap(),
    )
    .unwrap();

    let first = store
        .record_snapshot(
            &library,
            &first_scope,
            &SrsSettings::default(),
            LocalDatePolicy::UTC,
        )
        .unwrap();
    assert!(first.recorded);
    assert_eq!(first.revision, 1);
    assert_eq!((first.snapshot.learnt, first.snapshot.uncertain), (1, 0));

    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    raw["snapshots"][0]["future_snapshot_extension"] = serde_json::json!({"keep": true});
    raw["snapshots"][0]["scope"]["future_scope_extension"] = serde_json::json!({"keep": true});
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let mut second_scope = first_scope.clone();
    second_scope.as_of = BASE + 7 * DAY + 1;
    let second = store
        .record_snapshot(
            &library,
            &second_scope,
            &SrsSettings::default(),
            LocalDatePolicy::UTC,
        )
        .unwrap();
    assert!(second.recorded);
    assert_eq!(second.revision, 2);
    assert_eq!((second.snapshot.learnt, second.snapshot.uncertain), (1, 1));
    assert_ne!(first.snapshot.date, second.snapshot.date);

    let bytes_after_second = std::fs::read(&path).unwrap();
    second_scope.as_of += 60;
    let repeated = store
        .record_snapshot(
            &library,
            &second_scope,
            &SrsSettings::default(),
            LocalDatePolicy::UTC,
        )
        .unwrap();
    assert!(!repeated.recorded);
    assert_eq!(repeated.revision, 2);
    assert_eq!(repeated.snapshot, second.snapshot);
    assert_eq!(std::fs::read(&path).unwrap(), bytes_after_second);

    let loaded = store.load().unwrap();
    assert_eq!(loaded.snapshots.len(), 2);
    assert_eq!(loaded.snapshots[0], first.snapshot);
    assert_eq!(loaded.snapshots[1], second.snapshot);
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["snapshots"][0]["future_snapshot_extension"],
        serde_json::json!({"keep": true})
    );
    assert_eq!(
        saved["snapshots"][0]["scope"]["future_scope_extension"],
        serde_json::json!({"keep": true})
    );
}

#[test]
fn old_item_and_store_json_default_new_analytics_fields() {
    let mut item_value = serde_json::to_value(LearningItem::new(item_key(0))).unwrap();
    let item_object = item_value.as_object_mut().unwrap();
    for field in [
        "last_independent_at",
        "last_independent_correct_at",
        "last_independent_wrong_at",
        "last_independent_result",
        "recent_independent_outcomes",
        "resistant",
    ] {
        item_object.remove(field);
    }
    let item: LearningItem = serde_json::from_value(item_value).unwrap();
    assert_eq!(item.last_independent_at, None);
    assert_eq!(item.last_independent_result, None);
    assert!(item.recent_independent_outcomes.is_empty());
    assert!(!item.resistant);

    let mut store_value = serde_json::to_value(LearningStoreData::default()).unwrap();
    store_value.as_object_mut().unwrap().remove("snapshots");
    let store: LearningStoreData = serde_json::from_value(store_value).unwrap();
    assert!(store.snapshots.is_empty());
}
