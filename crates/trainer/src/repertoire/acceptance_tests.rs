use super::*;

const DAY: i64 = 24 * 60 * 60;
const BASE: i64 = 1_800_000_000;

fn item_key(index: usize) -> LearningItemKey {
    LearningItemKey::local(format!("position-{index} w - -"), "e2e4")
}

fn attempt(
    key: &LearningItemKey,
    id: impl Into<String>,
    event_type: TrainingEventType,
    happened_at: i64,
) -> TrainingAttempt {
    let actual_uci = match event_type {
        TrainingEventType::IncorrectIndependent => "d2d4",
        _ => "e2e4",
    };
    TrainingAttempt {
        user_id: key.user_id.clone(),
        position_fen4: key.position_fen4.clone(),
        client_attempt_id: id.into(),
        session_id: "session-1".into(),
        event_type,
        expected_uci: key.expected_uci.clone(),
        actual_uci: Some(actual_uci.into()),
        response_time_ms: Some(750),
        happened_at,
    }
}

fn record(item: &mut LearningItem, id: &str, event_type: TrainingEventType, happened_at: i64) {
    apply_attempt(
        item,
        &attempt(&item.key, id, event_type, happened_at),
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
    item.next_review_at = Some(next_review_at);
    item.previous_interval_seconds = Some(DAY);
    item
}

#[test]
fn at_02_first_independent_correct_introduces_learning_for_one_day() {
    let mut item = LearningItem::new(item_key(0));
    record(
        &mut item,
        "at-02",
        TrainingEventType::CorrectIndependent,
        BASE,
    );

    assert!(item.introduced);
    assert!(item.is_learning());
    assert!(!item.is_learnt());
    assert_eq!(item.correct_streak, 1);
    assert_eq!(item.wrong_streak, 0);
    assert_eq!(item.correct_count, 1);
    assert_eq!(item.wrong_count, 0);
    assert_eq!(item.last_review_at, Some(BASE));
    assert_eq!(item.last_correct_at, Some(BASE));
    assert_eq!(item.previous_interval_seconds, Some(DAY));
    assert_eq!(item.next_review_at, Some(BASE + DAY));
}

#[test]
fn at_03_third_independent_correct_is_learnt_and_still_scheduled() {
    let mut item = LearningItem::new(item_key(0));
    for (id, at) in [("at-03-a", BASE), ("at-03-b", BASE + DAY)] {
        record(&mut item, id, TrainingEventType::CorrectIndependent, at);
    }
    let third = BASE + 3 * DAY;
    record(
        &mut item,
        "at-03-c",
        TrainingEventType::CorrectIndependent,
        third,
    );

    assert_eq!(item.correct_streak, 3);
    assert!(item.is_learnt());
    assert!(!item.is_learning());
    assert_eq!(item.previous_interval_seconds, Some(4 * DAY));
    assert_eq!(item.next_review_at, Some(third + 4 * DAY));
}

#[test]
fn at_04_wrong_after_learnt_resets_streak_and_schedules_thirty_seconds() {
    let mut item = LearningItem::new(item_key(0));
    for (id, at) in [
        ("at-04-a", BASE),
        ("at-04-b", BASE + DAY),
        ("at-04-c", BASE + 3 * DAY),
    ] {
        record(&mut item, id, TrainingEventType::CorrectIndependent, at);
    }
    let wrong_at = BASE + 7 * DAY;
    record(
        &mut item,
        "at-04-wrong",
        TrainingEventType::IncorrectIndependent,
        wrong_at,
    );

    assert_eq!(item.correct_streak, 0);
    assert_eq!(item.wrong_streak, 1);
    assert!(item.is_learning());
    assert_eq!(item.last_correct_at, None);
    assert_eq!(item.next_review_at, Some(wrong_at + 30));
    assert_eq!(item.last_wrong_move_uci.as_deref(), Some("d2d4"));
    assert_eq!(item.last_wrong_at, Some(wrong_at));
    assert_eq!(
        item.wrong_move_histogram.get("d2d4"),
        Some(&WrongMoveStats {
            count: 1,
            last_at: wrong_at,
        })
    );
}

#[test]
fn at_05_replay_after_error_does_not_change_knowledge_or_due() {
    let mut item = LearningItem::new(item_key(0));
    record(
        &mut item,
        "at-05-wrong",
        TrainingEventType::IncorrectIndependent,
        BASE,
    );
    let before = item.clone();
    record(
        &mut item,
        "at-05-replay",
        TrainingEventType::ReplayAfterError,
        BASE + 1,
    );

    assert_eq!(item, before);
    assert_eq!(item.independent_attempt_count(), 1);
    assert_eq!(item.correct_count, 0);
    assert_eq!(item.next_review_at, Some(BASE + 30));
    assert_eq!(item.independent_response_times_ms, [750]);
}

#[test]
fn at_06_preview_introduces_unseen_without_independent_success() {
    let mut item = LearningItem::new(item_key(0));
    record(&mut item, "at-06", TrainingEventType::Previewed, BASE);

    assert!(item.introduced);
    assert!(item.is_learning());
    assert_eq!(item.correct_streak, 0);
    assert_eq!(item.wrong_streak, 0);
    assert_eq!(item.independent_attempt_count(), 0);
    assert_eq!(item.accuracy_percent(), None);
    assert_eq!(item.last_correct_at, None);
    assert_eq!(item.last_review_at, Some(BASE));
    assert_eq!(item.next_review_at, Some(BASE + 60));
    assert!(item.independent_response_times_ms.is_empty());
}

#[test]
fn at_08_learnt_item_is_also_due() {
    let mut item = LearningItem::new(item_key(0));
    for (id, at) in [
        ("at-08-a", BASE),
        ("at-08-b", BASE + DAY),
        ("at-08-c", BASE + 3 * DAY),
    ] {
        record(&mut item, id, TrainingEventType::CorrectIndependent, at);
    }
    let now = item.next_review_at.unwrap();
    let summary = aggregate(std::slice::from_ref(&item), now, &SrsSettings::default());

    assert_eq!(summary.total, 1);
    assert_eq!(summary.learning, 0);
    assert_eq!(summary.learnt, 1);
    assert_eq!(summary.due, 1);
    assert_eq!(summary.uncertain, 0);
    assert_invariants(&summary);
}

#[test]
fn at_12_accuracy_is_weighted_independent_accuracy_not_learning_status() {
    let mut item = LearningItem::new(item_key(0));
    for index in 0..7 {
        record(
            &mut item,
            &format!("at-12-correct-{index}"),
            TrainingEventType::CorrectIndependent,
            BASE + index,
        );
    }
    for index in 0..3 {
        record(
            &mut item,
            &format!("at-12-wrong-{index}"),
            TrainingEventType::IncorrectIndependent,
            BASE + 7 + index,
        );
    }
    let summary = aggregate(
        std::slice::from_ref(&item),
        BASE + 10,
        &SrsSettings::default(),
    );

    assert_eq!(item.correct_count, 7);
    assert_eq!(item.wrong_count, 3);
    assert_eq!(item.independent_attempt_count(), 10);
    assert!((item.accuracy_percent().unwrap() - 70.0).abs() < f64::EPSILON);
    assert!((summary.accuracy_percent.unwrap() - 70.0).abs() < f64::EPSILON);
    assert_eq!(summary.accuracy_attempts, 10);
    assert!(item.is_learning());
}

#[test]
fn at_13_accuracy_is_absent_without_independent_attempts() {
    let item = LearningItem::new(item_key(0));
    let summary = aggregate(std::slice::from_ref(&item), BASE, &SrsSettings::default());

    assert_eq!(item.accuracy_percent(), None);
    assert_eq!(summary.accuracy_percent, None);
    assert_eq!(summary.accuracy_attempts, 0);
    assert_eq!(summary.repertoire_status, RepertoireStatus::NotStarted);
}

#[test]
fn at_16_empty_scope_has_empty_status_and_no_percentages() {
    let items: Vec<LearningItem> = Vec::new();
    let summary = aggregate(items.iter(), BASE, &SrsSettings::default());

    assert_eq!(summary.total, 0);
    assert_eq!(summary.repertoire_status, RepertoireStatus::Empty);
    assert_eq!(summary.next_due_at, None);
    for percentage in [
        summary.coverage_percent,
        summary.learnt_percent,
        summary.confident_percent,
        summary.learning_percent,
        summary.unseen_percent,
        summary.uncertain_of_learnt_percent,
        summary.due_of_introduced_percent,
        summary.accuracy_percent,
    ] {
        assert_eq!(percentage, None);
    }
    assert_invariants(&summary);
}

#[test]
fn at_28_all_learnt_with_due_is_not_confirmed_current() {
    let now = BASE + 20 * DAY;
    let items: Vec<_> = (0..100)
        .map(|index| {
            let next = if index < 3 { now } else { now + DAY };
            learnt_item(index, now - DAY, next)
        })
        .collect();
    let summary = aggregate(items.iter(), now, &SrsSettings::default());

    assert_eq!(summary.total, 100);
    assert_eq!(summary.learnt, 100);
    assert_eq!(summary.due, 3);
    assert_eq!(summary.next_due_at, None);
    assert_eq!(summary.repertoire_status, RepertoireStatus::AllLearnt);
    assert_invariants(&summary);
}

#[test]
fn at_29_all_learnt_without_due_or_uncertain_is_confirmed_current() {
    let now = BASE + 20 * DAY;
    let items: Vec<_> = (0..100)
        .map(|index| learnt_item(index, now - DAY, now + DAY))
        .collect();
    let summary = aggregate(items.iter(), now, &SrsSettings::default());

    assert_eq!(summary.total, 100);
    assert_eq!(summary.learnt, 100);
    assert_eq!(summary.due, 0);
    assert_eq!(summary.uncertain, 0);
    assert_eq!(summary.next_due_at, Some(now + DAY));
    assert_eq!(
        summary.repertoire_status,
        RepertoireStatus::ConfirmedCurrent
    );
    assert_invariants(&summary);
}

#[test]
fn correct_after_four_days_schedules_eight_days() {
    let settings = SrsSettings::default();
    let mut item = LearningItem::new(item_key(0));
    apply_independent_correct(&mut item, BASE, &settings);
    apply_independent_correct(&mut item, BASE + 4 * DAY, &settings);

    assert_eq!(item.previous_interval_seconds, Some(8 * DAY));
    assert_eq!(item.next_review_at, Some(BASE + 12 * DAY));
}

#[test]
fn growth_interval_has_explicit_minimum_and_maximum() {
    let settings = SrsSettings {
        min_practical_interval_seconds: 3_600,
        max_interval_seconds: 10 * DAY,
        ..SrsSettings::default()
    };
    let mut minimum = LearningItem::new(item_key(0));
    apply_independent_correct(&mut minimum, BASE, &settings);
    apply_independent_correct(&mut minimum, BASE + 1, &settings);
    assert_eq!(minimum.previous_interval_seconds, Some(3_600));
    assert_eq!(minimum.next_review_at, Some(BASE + 1 + 3_600));

    let mut maximum = LearningItem::new(item_key(1));
    apply_independent_correct(&mut maximum, BASE, &settings);
    apply_independent_correct(&mut maximum, BASE + 10 * DAY, &settings);
    assert_eq!(maximum.previous_interval_seconds, Some(10 * DAY));
    assert_eq!(maximum.next_review_at, Some(BASE + 20 * DAY));
}

#[test]
fn preview_preserves_two_correct_streak_and_growth_base() {
    let settings = SrsSettings::default();
    let mut item = LearningItem::new(item_key(0));
    apply_independent_correct(&mut item, BASE, &settings);
    apply_independent_correct(&mut item, BASE + DAY, &settings);
    let previous_interval = item.previous_interval_seconds;
    let preview_at = BASE + 2 * DAY;
    apply_preview(&mut item, preview_at, &settings);

    assert_eq!(item.correct_streak, 2);
    assert_eq!(item.correct_count, 2);
    assert_eq!(item.wrong_count, 0);
    assert_eq!(item.wrong_streak, 0);
    assert_eq!(item.last_correct_at, Some(BASE + DAY));
    assert_eq!(item.previous_interval_seconds, previous_interval);
    assert_eq!(item.last_review_at, Some(preview_at));
    assert_eq!(item.next_review_at, Some(preview_at + 60));
}

#[test]
fn auto_context_and_illegal_move_create_no_independent_attempt() {
    let settings = SrsSettings::default();
    let key = item_key(0);
    let mut data = LearningData::default();
    assert_eq!(
        data.apply_attempt(
            attempt(&key, "illegal", TrainingEventType::IllegalMove, BASE),
            &settings,
        ),
        Ok(AttemptApplication::Applied)
    );
    assert_eq!(
        data.apply_attempt(
            attempt(&key, "auto", TrainingEventType::AutoContext, BASE + 1),
            &settings,
        ),
        Ok(AttemptApplication::Applied)
    );

    let item = data.item(&key).unwrap();
    assert_eq!(item, &LearningItem::new(key));
    assert_eq!(item.independent_attempt_count(), 0);
    assert_eq!(item.accuracy_percent(), None);
    assert!(item.independent_response_times_ms.is_empty());
    assert_eq!(data.attempts.len(), 2);
}

#[test]
fn duplicate_client_attempt_id_changes_state_only_once() {
    let settings = SrsSettings::default();
    let key = item_key(0);
    let first = attempt(
        &key,
        "duplicate-id",
        TrainingEventType::CorrectIndependent,
        BASE,
    );
    let mut data = LearningData::default();
    assert_eq!(
        data.apply_attempt(first.clone(), &settings),
        Ok(AttemptApplication::Applied)
    );
    let before = data.item(&key).unwrap().clone();
    assert_eq!(
        data.apply_attempt(first, &settings),
        Ok(AttemptApplication::Duplicate)
    );

    assert_eq!(data.item(&key), Some(&before));
    assert_eq!(data.item(&key).unwrap().correct_count, 1);
    assert_eq!(data.item(&key).unwrap().correct_streak, 1);
    assert_eq!(data.attempts.len(), 1);
}

#[test]
fn uncertain_uses_strict_greater_of_grace_boundaries() {
    let settings = SrsSettings::default();
    let mut item = learnt_item(0, BASE - 4 * DAY, BASE);
    item.previous_interval_seconds = Some(4 * DAY);
    assert!(!is_uncertain(&item, BASE + 7 * DAY, &settings));
    assert!(is_uncertain(&item, BASE + 7 * DAY + 1, &settings));

    item.previous_interval_seconds = Some(40 * DAY);
    assert!(!is_uncertain(&item, BASE + 7 * DAY + 1, &settings));
    assert!(!is_uncertain(&item, BASE + 10 * DAY, &settings));
    assert!(is_uncertain(&item, BASE + 10 * DAY + 1, &settings));

    item.correct_streak = 2;
    assert!(!is_uncertain(&item, BASE + 100 * DAY, &settings));
}

#[test]
fn aggregate_deduplicates_keys_and_weights_accuracy_once() {
    let mut first = LearningItem::new(item_key(0));
    first.correct_count = 7;
    first.wrong_count = 3;
    let second = LearningItem::new(item_key(1));
    let items = [first.clone(), first, second];
    let summary = aggregate(items.iter(), BASE, &SrsSettings::default());

    assert_eq!(summary.total, 2);
    assert_eq!(summary.unseen, 2);
    assert_eq!(summary.accuracy_correct, 7);
    assert_eq!(summary.accuracy_wrong, 3);
    assert_eq!(summary.accuracy_attempts, 10);
    assert!((summary.accuracy_percent.unwrap() - 70.0).abs() < f64::EPSILON);
    assert_invariants(&summary);
}

#[test]
fn learning_data_json_round_trip_preserves_event_types_and_state() {
    let settings = SrsSettings::default();
    let key = item_key(0);
    let mut data = LearningData::default();
    data.apply_attempt(
        attempt(
            &key,
            "serde-correct",
            TrainingEventType::CorrectIndependent,
            BASE,
        ),
        &settings,
    )
    .unwrap();
    data.apply_attempt(
        attempt(
            &key,
            "serde-replay",
            TrainingEventType::ReplayAfterError,
            BASE + 1,
        ),
        &settings,
    )
    .unwrap();

    let json = serde_json::to_string(&data).unwrap();
    assert!(json.contains("CORRECT_INDEPENDENT"));
    assert!(json.contains("REPLAY_AFTER_ERROR"));
    let restored: LearningData = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, data);
}

#[test]
fn mixed_event_sequence_preserves_aggregate_invariants() {
    let events = [
        TrainingEventType::CorrectIndependent,
        TrainingEventType::IncorrectIndependent,
        TrainingEventType::Previewed,
        TrainingEventType::ReplayAfterError,
        TrainingEventType::AutoContext,
        TrainingEventType::IllegalMove,
    ];
    let keys: Vec<_> = (0..7).map(item_key).collect();
    let settings = SrsSettings::default();
    let mut data = LearningData::default();
    for key in &keys {
        data.ensure_item(key.clone());
    }

    for step in 0..150_i64 {
        let key = &keys[(step as usize * 3 + step as usize / 7) % keys.len()];
        let happened_at = BASE + step * 45;
        let event = attempt(
            key,
            format!("property-{step}"),
            events[step as usize % events.len()],
            happened_at,
        );
        assert_eq!(
            data.apply_attempt(event.clone(), &settings),
            Ok(AttemptApplication::Applied)
        );
        if step % 9 == 0 {
            assert_eq!(
                data.apply_attempt(event, &settings),
                Ok(AttemptApplication::Duplicate)
            );
        }

        let as_of = happened_at + (step % 4) * 8 * DAY;
        let global = aggregate(data.items.iter().chain(data.items.iter()), as_of, &settings);
        assert_invariants(&global);
        let left = aggregate(data.items.iter().take(4), as_of, &settings);
        let right = aggregate(data.items.iter().skip(3), as_of, &settings);
        assert!(global.total <= left.total + right.total);
    }
}

fn assert_invariants(summary: &LearningAggregate) {
    assert_eq!(
        summary.total,
        summary.unseen + summary.learning + summary.learnt
    );
    assert!(summary.uncertain <= summary.learnt);
    assert!(summary.learnt_confident <= summary.learnt);
    assert_eq!(summary.learnt_confident, summary.learnt - summary.uncertain);
    assert!(summary.due <= summary.learning + summary.learnt);
    assert_eq!(
        summary.accuracy_attempts,
        summary.accuracy_correct + summary.accuracy_wrong
    );
    for percentage in [
        summary.coverage_percent,
        summary.learnt_percent,
        summary.confident_percent,
        summary.learning_percent,
        summary.unseen_percent,
        summary.uncertain_of_learnt_percent,
        summary.due_of_introduced_percent,
        summary.accuracy_percent,
    ]
    .into_iter()
    .flatten()
    {
        assert!(percentage.is_finite());
        assert!((0.0..=100.0).contains(&percentage));
    }
    if let (Some(coverage), Some(learnt), Some(confident)) = (
        summary.coverage_percent,
        summary.learnt_percent,
        summary.confident_percent,
    ) {
        assert!(coverage >= learnt);
        assert!(learnt >= confident);
    } else {
        assert_eq!(summary.total, 0);
    }
    if let Some(next) = summary.next_due_at {
        assert!(summary.due < summary.total);
        assert!(next > 0);
    }
}
