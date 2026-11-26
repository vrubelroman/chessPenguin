//! Daily sessions (due + N new), mistakes coming back in the same session, one-pass
//! review and sessions limited to a branch.

use super::*;

const NOW: i64 = 1_800_000_000;
const PGN: &str = "1. e4 e5 2. Nf3 Nc6 3. Bb5 (3. Bc4 Bc5) a6 4. Ba4 *";

fn library(pgn: &str) -> (Library, RepertoireId) {
    let mut library = Library::default();
    let id = library
        .create("Test", &chess_core::parse_str(pgn).game)
        .unwrap();
    (library, id)
}

fn key_for(library: &Library, scope: &StatsScope, expected_uci: &str) -> LearningItemKey {
    active_learning_memberships(library, scope)
        .unwrap()
        .into_iter()
        .find(|membership| membership.key.expected_uci == expected_uci)
        .unwrap()
        .key
}

fn queue_moves(session: &LearningSessionSnapshot) -> Vec<&str> {
    session
        .queue
        .iter()
        .skip(session.current_index)
        .filter(|target| !session.completed_keys.contains(&target.key))
        .map(|target| target.key.expected_uci.as_str())
        .collect()
}

/// Plays the automatic context up to the question, applying every event.
fn to_question(
    session: &mut LearningSessionSnapshot,
    library: &Library,
    learning: &mut LearningData,
) {
    let mut step = 0;
    while session
        .presentation_with_learning(library, learning)
        .unwrap()
        .phase
        == LearningSessionPhase::AdvancingContext
    {
        step += 1;
        let attempt = session
            .advance_context_with_learning(
                library,
                learning,
                format!("ctx-{step}-{}", session.revision),
                NOW,
            )
            .unwrap();
        session.revision += 1;
        learning
            .apply_attempt(attempt, &SrsSettings::default())
            .unwrap();
    }
}

fn answer(
    session: &mut LearningSessionSnapshot,
    library: &Library,
    learning: &mut LearningData,
    uci: &str,
    id: &str,
) -> TrainingAttempt {
    let attempt = session
        .play_uci_with_learning(library, learning, uci, id, NOW, Some(1000))
        .unwrap();
    learning
        .apply_attempt(attempt.clone(), &SrsSettings::default())
        .unwrap();
    attempt
}

#[test]
fn daily_queue_takes_every_due_move_and_at_most_n_new_in_pgn_order() {
    let (library, id) = library(PGN);
    let scope = StatsScope::for_repertoire(id, LearningSide::White, NOW);
    let mut learning = LearningData::default();
    // Due deep in the line, known and not due early in it.
    for (uci, next) in [("f1b5", NOW - 10), ("e2e4", NOW + 3600)] {
        let item = learning.ensure_item(key_for(&library, &scope, uci));
        item.introduced = true;
        item.next_review_at = Some(next);
    }
    let session = LearningSessionSnapshot::new_daily(
        "daily",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
        2,
    )
    .unwrap();
    // Own moves in PGN order: e4 Nf3 Bb5 Ba4 Bc4. Due: Bb5. New: Nf3, Ba4 (2 of 3).
    assert_eq!(queue_moves(&session), ["g1f3", "f1b5", "b5a4"]);
    assert!(session.return_mistakes);
    assert_eq!(session.session.mode, QueueMode::SpacedRepetition);
    let all = LearningSessionSnapshot::new_daily(
        "daily-all",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
        10,
    )
    .unwrap();
    assert_eq!(queue_moves(&all), ["g1f3", "f1b5", "b5a4", "f1c4"]);
    let none = LearningSessionSnapshot::new_daily(
        "daily-none",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
        0,
    )
    .unwrap();
    assert_eq!(queue_moves(&none), ["f1b5"]);
}

#[test]
fn a_mistake_comes_back_in_the_same_session_and_a_shown_new_move_does_not() {
    let (library, id) = library("1. e4 e5 2. Nf3 Nc6 *");
    let scope = StatsScope::for_repertoire(id, LearningSide::White, NOW);
    let mut learning = LearningData::default();
    let e4 = key_for(&library, &scope, "e2e4");
    let item = learning.ensure_item(e4.clone());
    item.introduced = true;
    item.next_review_at = Some(NOW);
    let mut session = LearningSessionSnapshot::new_daily(
        "return",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
        10,
    )
    .unwrap();
    assert_eq!(queue_moves(&session), ["e2e4", "g1f3"]);
    assert_eq!(session.remaining(), 2);
    assert!(!session.current_is_new(&learning));

    let wrong = answer(&mut session, &library, &mut learning, "d2d4", "wrong");
    assert_eq!(wrong.event_type, TrainingEventType::IncorrectIndependent);
    assert_eq!(
        session.phase,
        LearningSessionPhase::AwaitingReplayAfterError
    );
    assert_eq!(session.remaining(), 2);
    let replay = answer(&mut session, &library, &mut learning, "e2e4", "replay");
    assert_eq!(replay.event_type, TrainingEventType::ReplayAfterError);
    // e4 is not done: it waits behind Nf3.
    assert_eq!(queue_moves(&session), ["g1f3", "e2e4"]);
    assert_eq!(session.remaining(), 2);
    assert!(!session.completed_keys.contains(&e4));
    session.validate(&library).unwrap();

    to_question(&mut session, &library, &mut learning);
    assert!(session.current_is_new(&learning));
    let shown = session.preview_new("show-nf3", NOW).unwrap().unwrap();
    assert_eq!(shown.event_type, TrainingEventType::Previewed);
    assert!(session.showing_new);
    learning
        .apply_attempt(shown, &SrsSettings::default())
        .unwrap();
    let played = answer(&mut session, &library, &mut learning, "g1f3", "play-nf3");
    assert_eq!(played.event_type, TrainingEventType::AutoContext);
    // A shown new move is played once and counts as done.
    assert_eq!(queue_moves(&session), ["e2e4"]);
    assert_eq!(session.remaining(), 1);
    assert!(!session.showing_new);

    let correct = answer(&mut session, &library, &mut learning, "e2e4", "second-try");
    assert_eq!(correct.event_type, TrainingEventType::CorrectIndependent);
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    assert_eq!(session.remaining(), 0);
    let item = learning.item(&e4).unwrap();
    assert_eq!((item.wrong_count, item.correct_count), (1, 1));
    session.validate(&library).unwrap();
}

#[test]
fn a_hint_comes_back_but_not_in_a_one_pass_review() {
    let (library, id) = library("1. e4 e5 2. Nf3 *");
    let scope = StatsScope::for_repertoire(id, LearningSide::White, NOW);
    let mut learning = LearningData::default();
    for uci in ["e2e4", "g1f3"] {
        let item = learning.ensure_item(key_for(&library, &scope, uci));
        item.introduced = true;
        item.next_review_at = Some(NOW);
    }
    let mut daily = LearningSessionSnapshot::new_daily(
        "hint-daily",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
        10,
    )
    .unwrap();
    daily.preview("hint", NOW).unwrap().unwrap();
    assert!(!daily.showing_new);
    answer(&mut daily, &library, &mut learning, "e2e4", "after-hint");
    assert_eq!(queue_moves(&daily), ["g1f3", "e2e4"]);

    let mut review = LearningSessionSnapshot::new_single_pass_review(
        "hint-review",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
    )
    .unwrap();
    assert!(!review.return_mistakes);
    answer(&mut review, &library, &mut learning, "d2d4", "review-wrong");
    answer(
        &mut review,
        &library,
        &mut learning,
        "e2e4",
        "review-replay",
    );
    assert_eq!(queue_moves(&review), ["g1f3"]);
}

#[test]
fn one_pass_review_finishes_instead_of_starting_a_new_cycle() {
    let (library, id) = library("1. e4 e5 2. Nf3 *");
    let scope = StatsScope::for_repertoire(id, LearningSide::White, NOW);
    let mut learning = LearningData::default();
    let mut review = LearningSessionSnapshot::new_single_pass_review(
        "single",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(review.session.mode, QueueMode::ReviewInOrder);
    assert_eq!(queue_moves(&review), ["e2e4", "g1f3"]);
    answer(&mut review, &library, &mut learning, "e2e4", "single-e4");
    to_question(&mut review, &library, &mut learning);
    answer(&mut review, &library, &mut learning, "g1f3", "single-nf3");
    assert_eq!(review.phase, LearningSessionPhase::Finished);
    assert_eq!(review.cycle, 0);
    review.validate(&library).unwrap();

    // The old looping review is unchanged.
    let mut looping = LearningSessionSnapshot::new_with_mode(
        "loop",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
        QueueMode::ReviewInOrder,
    )
    .unwrap();
    answer(&mut looping, &library, &mut learning, "e2e4", "loop-e4");
    to_question(&mut looping, &library, &mut learning);
    answer(&mut looping, &library, &mut learning, "g1f3", "loop-nf3");
    assert_eq!(looping.cycle, 1);
    assert_ne!(looping.phase, LearningSessionPhase::Finished);
}

#[test]
fn introduce_more_takes_the_next_unseen_moves_only() {
    let (library, id) = library(PGN);
    let scope = StatsScope::for_repertoire(id, LearningSide::White, NOW);
    let mut learning = LearningData::default();
    let mut session = LearningSessionSnapshot::new_daily(
        "more",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
        2,
    )
    .unwrap();
    assert_eq!(queue_moves(&session), ["e2e4", "g1f3"]);
    assert_eq!(
        session.introduce_more(&library, &learning, 2),
        Err(LearningSessionError::IntroductionRequiresFinished)
    );
    for uci in ["e2e4", "g1f3"] {
        to_question(&mut session, &library, &mut learning);
        let shown = session
            .preview_new(format!("show-{uci}"), NOW)
            .unwrap()
            .unwrap();
        learning
            .apply_attempt(shown, &SrsSettings::default())
            .unwrap();
        answer(
            &mut session,
            &library,
            &mut learning,
            uci,
            &format!("play-{uci}"),
        );
    }
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    // The two shown moves are due again in a minute, yet "more" means new ones only.
    assert_eq!(session.introduce_more(&library, &learning, 2), Ok(2));
    assert_eq!(queue_moves(&session), ["f1b5", "b5a4"]);
    assert_eq!(
        session.introduce_more(&library, &learning, 2),
        Err(LearningSessionError::IntroductionRequiresFinished)
    );
    let mut review = LearningSessionSnapshot::new_single_pass_review(
        "more-review",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
    )
    .unwrap();
    while review.current_target().is_some() {
        review.current_index += 1;
    }
    review.phase = LearningSessionPhase::Finished;
    assert_eq!(
        review.introduce_more(&library, &learning, 2),
        Err(LearningSessionError::UnsupportedMode(
            QueueMode::ReviewInOrder
        ))
    );
}

#[test]
fn branch_session_asks_only_moves_of_the_branch_from_the_start_of_the_game() {
    let (library, id) = library(PGN);
    let through = ["e2e4", "e7e5", "g1f3", "b8c6", "f1c4"].map(str::to_owned);
    let scope = StatsScope::for_repertoire(id.clone(), LearningSide::White, NOW);
    let mut learning = LearningData::default();
    // Through an own move: that move is asked, its sibling Bb5 is not.
    let mut session = LearningSessionSnapshot::new_branch_review(
        "branch",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
        &through,
    )
    .unwrap();
    assert_eq!(queue_moves(&session), ["f1c4"]);
    assert_eq!(
        session.stats_scope.branch_root.as_deref(),
        Some(&through[..4])
    );
    assert!(session.single_pass);
    let target = session.current_target().unwrap();
    assert_eq!(target.path, through);
    // The context is played from the start of the game, not from the branch.
    let presentation = session
        .presentation_with_learning(&library, &learning)
        .unwrap();
    assert_eq!(presentation.automatic_context.len(), 4);
    assert_eq!(presentation.automatic_context[0].uci, "e2e4");
    to_question(&mut session, &library, &mut learning);
    answer(&mut session, &library, &mut learning, "f1c4", "branch-bc4");
    assert_eq!(session.phase, LearningSessionPhase::Finished);
    session.validate(&library).unwrap();

    // Through an opponent's move: every own move below it.
    let below = LearningSessionSnapshot::new_branch_review(
        "branch-below",
        &library,
        &LearningData::default(),
        scope.clone(),
        SrsSettings::default(),
        &through[..4],
    )
    .unwrap();
    assert_eq!(queue_moves(&below), ["f1b5", "b5a4", "f1c4"]);
    assert!(
        below
            .queue
            .iter()
            .all(|target| target.path.starts_with(&through[..4]))
    );

    // The whole repertoire, and a daily session limited by a branch root.
    let whole = LearningSessionSnapshot::new_branch_review(
        "branch-whole",
        &library,
        &LearningData::default(),
        scope.clone(),
        SrsSettings::default(),
        &[],
    )
    .unwrap();
    assert_eq!(queue_moves(&whole).len(), 5);
    let daily = LearningSessionSnapshot::new_daily(
        "branch-daily",
        &library,
        &LearningData::default(),
        scope.with_branch_root(through[..4].to_vec()),
        SrsSettings::default(),
        10,
    )
    .unwrap();
    assert_eq!(queue_moves(&daily), ["f1b5", "b5a4", "f1c4"]);
}

#[test]
fn new_flags_are_optional_in_the_file_and_survive_a_roundtrip() {
    let (library, id) = library("1. e4 *");
    let scope = StatsScope::for_repertoire(id, LearningSide::White, NOW);
    let learning = LearningData::default();
    let plain = LearningSessionSnapshot::new(
        "plain",
        &library,
        &learning,
        scope.clone(),
        SrsSettings::default(),
    )
    .unwrap();
    let json = serde_json::to_value(&plain).unwrap();
    // 2.3.0 snapshots stay byte-compatible when no new behaviour is used.
    for field in ["return_mistakes", "single_pass", "showing_new"] {
        assert!(
            json.get(field).is_none(),
            "{field} written for an old-style session"
        );
    }
    let read: LearningSessionSnapshot = serde_json::from_value(json).unwrap();
    assert_eq!(read, plain);

    let mut daily = LearningSessionSnapshot::new_daily(
        "flags",
        &library,
        &learning,
        scope,
        SrsSettings::default(),
        10,
    )
    .unwrap();
    daily.preview_new("show", NOW).unwrap();
    daily.single_pass = true;
    let json = serde_json::to_string(&daily).unwrap();
    let read: LearningSessionSnapshot = serde_json::from_str(&json).unwrap();
    assert!(read.return_mistakes && read.single_pass && read.showing_new);
    read.validate(&library).unwrap();
}
