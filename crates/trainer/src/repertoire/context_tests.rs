use super::*;
use crate::repertoire::learning::AttemptApplication;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 24 * 60 * 60;

struct Fixture {
    library: Library,
    scope: StatsScope,
    learning: LearningData,
    session: LearningSessionSnapshot,
}

fn key_for(library: &Library, scope: &StatsScope, expected_uci: &str) -> LearningItemKey {
    active_learning_memberships(library, scope)
        .unwrap()
        .into_iter()
        .find(|membership| membership.key.expected_uci == expected_uci)
        .unwrap()
        .key
}

fn seed(
    learning: &mut LearningData,
    key: LearningItemKey,
    streak: u32,
    next_review_at: Option<i64>,
) {
    let item = learning.ensure_item(key);
    item.introduced = true;
    item.correct_streak = streak;
    item.correct_count = u64::from(streak);
    item.last_review_at = (streak > 0).then_some(NOW - DAY);
    item.last_correct_at = (streak > 0).then_some(NOW - DAY);
    item.next_review_at = next_review_at;
}

fn fixture(
    pgn: &str,
    side: LearningSide,
    mode: QueueMode,
    policy: LearningContextPolicy,
    items: &[(&str, u32, Option<i64>)],
) -> Fixture {
    let mut library = Library::default();
    let id = library
        .create("Test", &chess_core::parse_str(pgn).game)
        .unwrap();
    let scope = StatsScope::for_repertoire(id, side, NOW);
    let mut learning = LearningData::default();
    for (expected, streak, next) in items {
        let key = key_for(&library, &scope, expected);
        seed(&mut learning, key, *streak, *next);
    }
    let mut session = LearningSessionSnapshot::new_with_mode(
        "context",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
        mode,
    )
    .unwrap();
    session.context_policy = policy;
    Fixture {
        library,
        scope,
        learning,
        session,
    }
}

fn reinforcing_policy() -> LearningContextPolicy {
    LearningContextPolicy {
        consider_leadup_moves: true,
        ..LearningContextPolicy::default()
    }
}

fn apply(learning: &mut LearningData, attempt: &TrainingAttempt) {
    assert_eq!(
        learning.apply_attempt(attempt.clone(), &SrsSettings::default()),
        Ok(AttemptApplication::Applied)
    );
}

fn play(fixture: &mut Fixture, uci: &str, id: &str) -> TrainingAttempt {
    let attempt = fixture
        .session
        .play_uci_with_learning(&fixture.library, &fixture.learning, uci, id, NOW, Some(200))
        .unwrap();
    apply(&mut fixture.learning, &attempt);
    attempt
}

fn advance(fixture: &mut Fixture, id: &str) -> TrainingAttempt {
    let attempt = fixture
        .session
        .advance_context_with_learning(&fixture.library, &fixture.learning, id, NOW)
        .unwrap();
    apply(&mut fixture.learning, &attempt);
    attempt
}

fn automatic_uci(presentation: &LearningPresentation) -> Vec<&str> {
    presentation
        .automatic_context
        .iter()
        .map(|step| step.uci.as_str())
        .collect()
}

#[test]
fn weak_context_is_reinforced_as_an_independent_answer() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        reinforcing_policy(),
        &[("e2e4", 2, Some(NOW + 10 * DAY)), ("g1f3", 0, Some(NOW))],
    );
    let e4 = key_for(&f.library, &f.scope, "e2e4");
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(
        presentation.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    assert!(presentation.automatic_context.is_empty());
    assert_eq!(f.session.current_expected_uci(), Some("e2e4"));

    let answer = play(&mut f, "e2e4", "weak-e4");
    assert_eq!(answer.event_type, TrainingEventType::CorrectIndependent);
    assert!(f.session.completed_keys.contains(&e4));
    assert_eq!(f.session.phase, LearningSessionPhase::AdvancingContext);
    assert_eq!(
        advance(&mut f, "weak-e5").event_type,
        TrainingEventType::AutoContext
    );
    assert_eq!(f.session.phase, LearningSessionPhase::AwaitingIndependent);
    play(&mut f, "g1f3", "weak-target");
    assert_eq!(f.session.phase, LearningSessionPhase::Finished);
    assert_eq!(
        f.learning
            .attempts
            .iter()
            .filter(|attempt| attempt.item_key() == e4 && attempt.event_type.is_independent())
            .count(),
        1
    );
}

#[test]
fn due_soon_policy_reinforces_even_a_learnt_context_move() {
    let mut policy = reinforcing_policy();
    policy.reinforce_minimum_days = Some(3);
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        policy,
        &[("e2e4", 3, Some(NOW + 2 * DAY)), ("g1f3", 0, Some(NOW))],
    );
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(
        presentation.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    assert_eq!(f.session.current_expected_uci(), Some("e2e4"));
}

#[test]
fn well_known_context_outside_due_window_is_not_reinforced() {
    let mut policy = reinforcing_policy();
    policy.reinforce_minimum_days = Some(3);
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        policy,
        &[("e2e4", 3, Some(NOW + 10 * DAY)), ("g1f3", 0, Some(NOW))],
    );
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(presentation.phase, LearningSessionPhase::AdvancingContext);
    assert_eq!(automatic_uci(&presentation), ["e2e4", "e7e5"]);
}

#[test]
fn very_well_known_start_prefix_is_omitted_without_attempts() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        reinforcing_policy(),
        &[
            ("e2e4", 100, Some(NOW + 10 * DAY)),
            ("g1f3", 100, Some(NOW + 10 * DAY)),
            ("f1b5", 0, Some(NOW)),
        ],
    );
    let before = f.learning.clone();
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(
        presentation.phase,
        LearningSessionPhase::AwaitingIndependent
    );
    assert!(presentation.automatic_context.is_empty());
    assert_eq!(f.session.context_index, 4);
    assert_eq!(f.session.current_expected_uci(), Some("f1b5"));
    assert_eq!(
        presentation.current_position.unwrap().fen4,
        key_for(&f.library, &f.scope, "f1b5").position_fen4
    );
    assert_eq!(f.learning, before);
}

#[test]
fn opponent_context_stays_automatic_even_with_weak_learning_data() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::Black,
        QueueMode::SpacedRepetition,
        reinforcing_policy(),
        &[("e7e5", 2, Some(NOW + 10 * DAY)), ("b8c6", 0, Some(NOW))],
    );
    let white_scope = StatsScope::for_repertoire(
        f.scope.repertoire_ids.iter().next().unwrap().clone(),
        LearningSide::White,
        NOW,
    );
    let opponent = key_for(&f.library, &white_scope, "e2e4");
    seed(&mut f.learning, opponent, 0, Some(NOW));

    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(presentation.phase, LearningSessionPhase::AdvancingContext);
    assert_eq!(automatic_uci(&presentation), ["e2e4"]);
    let automatic = advance(&mut f, "opponent-e4");
    assert_eq!(automatic.event_type, TrainingEventType::AutoContext);
    assert_eq!(
        f.session.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    assert_eq!(f.session.current_expected_uci(), Some("e7e5"));
}

#[test]
fn reinforcement_is_not_repeated_in_the_same_review_cycle() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::ReviewInOrder,
        reinforcing_policy(),
        &[("e2e4", 0, Some(NOW + DAY)), ("g1f3", 0, Some(NOW))],
    );
    f.session.queue.reverse();
    let e4 = key_for(&f.library, &f.scope, "e2e4");
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(
        presentation.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    play(&mut f, "e2e4", "cycle-context-e4");
    advance(&mut f, "cycle-e5");
    play(&mut f, "g1f3", "cycle-target-nf3");
    assert_eq!(f.session.cycle, 1);
    assert!(f.session.completed_keys.is_empty());
    assert_eq!(
        f.learning
            .attempts
            .iter()
            .filter(|attempt| attempt.item_key() == e4 && attempt.event_type.is_independent())
            .count(),
        1
    );

    let next_cycle = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(
        next_cycle.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
}

#[test]
fn wrong_context_replay_completes_context_without_success() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        reinforcing_policy(),
        &[("e2e4", 2, Some(NOW + 10 * DAY)), ("g1f3", 0, Some(NOW))],
    );
    let e4 = key_for(&f.library, &f.scope, "e2e4");
    f.session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    let wrong = play(&mut f, "d2d4", "context-wrong");
    assert_eq!(wrong.event_type, TrainingEventType::IncorrectIndependent);
    assert_eq!(
        f.session.phase,
        LearningSessionPhase::AwaitingReplayAfterError
    );
    let revealed = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(revealed.revealed_move.as_deref(), Some("e2e4"));
    assert!(revealed.automatic_context.is_empty());

    let replay = play(&mut f, "e2e4", "context-replay");
    assert_eq!(replay.event_type, TrainingEventType::ReplayAfterError);
    assert!(f.session.completed_keys.contains(&e4));
    assert_eq!(f.session.phase, LearningSessionPhase::AdvancingContext);
    let item = f.learning.item(&e4).unwrap();
    assert_eq!(item.correct_count, 2);
    assert_eq!(item.wrong_count, 1);
    assert_eq!(item.correct_streak, 0);
    assert_eq!(item.last_correct_at, None);
    assert_eq!(
        f.learning
            .attempts
            .iter()
            .filter(|attempt| attempt.item_key() == e4 && attempt.event_type.is_independent())
            .count(),
        1
    );
}

#[test]
fn leadup_off_and_legacy_methods_keep_context_automatic() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        LearningContextPolicy::default(),
        &[("e2e4", 0, Some(NOW + DAY)), ("g1f3", 0, Some(NOW))],
    );
    let e4 = key_for(&f.library, &f.scope, "e2e4");
    let before = f.learning.item(&e4).unwrap().clone();
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(presentation.phase, LearningSessionPhase::AdvancingContext);
    assert_eq!(automatic_uci(&presentation), ["e2e4", "e7e5"]);
    assert!(
        presentation
            .automatic_context
            .iter()
            .all(|step| step.speed_factor == 1.0)
    );

    let first = f
        .session
        .advance_context(&f.library, "legacy-e4", NOW)
        .unwrap();
    apply(&mut f.learning, &first);
    let second = f
        .session
        .advance_context(&f.library, "legacy-e5", NOW)
        .unwrap();
    apply(&mut f.learning, &second);
    assert_eq!(f.session.phase, LearningSessionPhase::AwaitingIndependent);
    assert_eq!(f.learning.item(&e4), Some(&before));
}

#[test]
fn fixed_and_adaptive_speed_factors_are_deterministic_and_clamped() {
    let mut policy = LearningContextPolicy {
        use_fixed_piece_speed: true,
        move_speed_adjustment: 3.0,
        ..LearningContextPolicy::default()
    };
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        policy,
        &[("e2e4", 3, Some(NOW + DAY)), ("g1f3", 0, Some(NOW))],
    );
    let fixed = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert!(
        fixed
            .automatic_context
            .iter()
            .all(|step| step.speed_factor == 1.0)
    );

    policy = f.session.context_policy;
    policy.use_fixed_piece_speed = false;
    policy.move_speed_adjustment = 1.5;
    f.session.context_policy = policy;
    let adaptive = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(adaptive.automatic_context[0].speed_factor, 3.0);
    assert_eq!(adaptive.automatic_context[1].speed_factor, 1.5);

    f.session.context_policy.move_speed_adjustment = 100.0;
    let high = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert!(
        high.automatic_context
            .iter()
            .all(|step| step.speed_factor == 4.0)
    );
    f.session.context_policy.move_speed_adjustment = 0.0;
    let low = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert!(
        low.automatic_context
            .iter()
            .all(|step| step.speed_factor == 0.25)
    );
}

#[test]
fn resume_keeps_partial_learning_context_position() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        reinforcing_policy(),
        &[
            ("e2e4", 2, Some(NOW + DAY)),
            ("g1f3", 3, Some(NOW + 10 * DAY)),
            ("f1b5", 0, Some(NOW)),
        ],
    );
    f.session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    play(&mut f, "e2e4", "resume-e4");
    advance(&mut f, "resume-e5");
    assert_eq!(f.session.context_index, 2);

    let json = serde_json::to_string(&f.session).unwrap();
    let restored = serde_json::from_str::<LearningSessionSnapshot>(&json)
        .unwrap()
        .resume(&f.library)
        .unwrap();
    f.session = restored;
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(f.session.context_index, 2);
    assert_eq!(automatic_uci(&presentation), ["g1f3", "b8c6"]);
    advance(&mut f, "resume-nf3");
    advance(&mut f, "resume-nc6");
    assert_eq!(f.session.phase, LearningSessionPhase::AwaitingIndependent);
    assert_eq!(f.session.current_expected_uci(), Some("f1b5"));
}

#[test]
fn old_json_unknown_fields_and_duplicate_attempts_remain_compatible() {
    let mut f = fixture(
        "1. e4 *",
        LearningSide::White,
        QueueMode::SpacedRepetition,
        LearningContextPolicy::default(),
        &[("e2e4", 1, Some(NOW))],
    );
    let mut raw = serde_json::to_value(&f.session).unwrap();
    raw.as_object_mut().unwrap().remove("context_policy");
    raw["future_snapshot"] = serde_json::json!({"preserved": true});
    let restored: LearningSessionSnapshot = serde_json::from_value(raw).unwrap();
    assert_eq!(restored.context_policy, LearningContextPolicy::default());
    restored.clone().resume(&f.library).unwrap();
    let policy: LearningContextPolicy = serde_json::from_value(serde_json::json!({
        "future_policy_field": true
    }))
    .unwrap();
    assert_eq!(policy, LearningContextPolicy::default());
    f.session = restored;

    let attempt = f
        .session
        .play_uci_with_learning(&f.library, &f.learning, "e2e4", "at-23", NOW, Some(100))
        .unwrap();
    apply(&mut f.learning, &attempt);
    assert_eq!(
        f.learning.apply_attempt(attempt, &SrsSettings::default()),
        Ok(AttemptApplication::Duplicate)
    );
    let e4 = key_for(&f.library, &f.scope, "e2e4");
    assert_eq!(f.learning.item(&e4).unwrap().correct_count, 2);
    assert_eq!(
        f.learning
            .attempts
            .iter()
            .filter(|attempt| attempt.client_attempt_id == "at-23")
            .count(),
        1
    );
}

#[test]
fn least_recent_keeps_legacy_context_checks_with_leadup_off() {
    let mut f = fixture(
        "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
        LearningSide::White,
        QueueMode::LeastRecent,
        LearningContextPolicy::default(),
        &[
            ("e2e4", 0, Some(NOW + DAY)),
            ("g1f3", 0, Some(NOW + DAY)),
            ("f1b5", 0, Some(NOW + DAY)),
        ],
    );
    f.session.queue.reverse();
    f.session.phase = LearningSessionPhase::AdvancingContext;
    let presentation = f
        .session
        .presentation_with_learning(&f.library, &f.learning)
        .unwrap();
    assert_eq!(
        presentation.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    assert_eq!(f.session.current_expected_uci(), Some("e2e4"));
    assert!(presentation.automatic_context.is_empty());
}
