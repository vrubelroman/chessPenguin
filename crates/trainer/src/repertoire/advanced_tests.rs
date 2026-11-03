use chess_core::shakmaty::{Color, Position};
use chess_core::{Game, NodeId};

use super::*;

fn repertoire(pgn: &str) -> Repertoire {
    let mut library = Library::default();
    let id = library
        .create("Scope", &chess_core::parse_str(pgn).game)
        .unwrap();
    library.repertoire(&id).unwrap().clone()
}

fn repertoire_from_lines(lines: &[&str]) -> Repertoire {
    let mut game = Game::new();
    for line in lines {
        game.add_uci_line(NodeId::ROOT, &[*line]).unwrap();
    }
    let mut library = Library::default();
    let id = library.create("Many", &game).unwrap();
    library.repertoire(&id).unwrap().clone()
}

fn strings(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|mv| (*mv).to_owned()).collect()
}

fn random_session(rep: &Repertoire, mode: TrainingMode, seed: u64) -> TrainingSession {
    TrainingSession::scoped_random_with_seed(
        rep,
        TrainingScope::whole(),
        mode,
        StatsData::default(),
        0,
        seed,
    )
    .unwrap()
}

fn random_route(session: &TrainingSession) -> Vec<String> {
    let mut route = session.scope().parent_prefix().to_vec();
    route.extend(session.suggested_path().unwrap().iter().cloned());
    route
}

fn path_key(rep: &Repertoire, scope: &TrainingScope, path: &[String]) -> PathKey {
    PathKey {
        repertoire_id: rep.id.clone(),
        scope: scope.key(),
        path: path.to_vec(),
    }
}

#[test]
fn scope_starts_at_black_move_with_context_and_stops_at_early_boundary() {
    let rep = repertoire("1. e4 e5 2. Nf3 Nc6 (2... Nf6) 3. Bb5 a6 4. Ba4 Nf6 *");
    let prefix = strings(&["e2e4", "e7e5", "g1f3", "b8c6"]);
    let end = strings(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6"]);
    let scope = TrainingScope::between(prefix, end);
    let mut session = TrainingSession::scoped(
        &rep,
        scope.clone(),
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();

    assert_eq!(session.position().turn(), Color::Black);
    assert_eq!(
        session
            .path()
            .iter()
            .map(|mv| mv.uci.as_str())
            .collect::<Vec<_>>(),
        ["e2e4", "e7e5", "g1f3"]
    );
    assert!(session.path().iter().all(|mv| mv.automatic));
    assert!(session.attempts().is_empty());
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.uci.as_str())
            .collect::<Vec<_>>(),
        ["b8c6"]
    );

    assert_eq!(session.play_uci("g8f6", 1).unwrap(), TrainingVerdict::Wrong);
    assert_eq!(session.play_uci("b8c6", 2).unwrap(), TrainingVerdict::Right);
    assert_eq!(session.play_uci("f1b5", 3).unwrap(), TrainingVerdict::Right);
    assert_eq!(
        session.play_uci("a7a6", 4).unwrap(),
        TrainingVerdict::Finished
    );
    assert!(session.is_done());
    let actual = strings(&["b8c6", "f1b5", "a7a6"]);
    let stats = session
        .stats()
        .path_stats(&path_key(&rep, &scope, &actual))
        .unwrap();
    assert_eq!(stats.completions, 1);
    assert_eq!(stats.mistakes, 1);
    assert_eq!(stats.recent[0].path, actual);
    assert_eq!(stats.recent[0].scope, scope.key());
    assert_eq!(stats.recent[0].automatic_moves, 3);

    let mut without_context = TrainingSession::scoped(
        &rep,
        scope.with_context(false),
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        10,
    )
    .unwrap();
    assert!(without_context.path().is_empty());
    assert_eq!(without_context.position().turn(), Color::Black);
    assert_eq!(
        without_context.play_uci("b8c6", 10).unwrap(),
        TrainingVerdict::Right
    );
}

#[test]
fn scope_honours_disabled_depth_and_selected_move_as_its_only_early_end() {
    let mut rep = repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 (3. Bc4) a6 4. Ba4 *");
    rep.set_prefix_disabled(&strings(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"]), true)
        .unwrap();
    let scope = TrainingScope::from_prefix(strings(&["e2e4", "e7e5", "g1f3", "b8c6"]));
    let mut session = TrainingSession::scoped(
        &rep,
        scope.clone(),
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    session.play_uci("b8c6", 1).unwrap();
    assert_eq!(session.play_uci("f1b5", 2).unwrap(), TrainingVerdict::Wrong);
    assert_eq!(
        session.play_uci("f1c4", 3).unwrap(),
        TrainingVerdict::Finished
    );

    rep.set_max_plies(4);
    let mut depth = TrainingSession::scoped(
        &rep,
        scope.clone(),
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        10,
    )
    .unwrap();
    assert_eq!(
        depth.play_uci("b8c6", 10).unwrap(),
        TrainingVerdict::Finished
    );

    let only = TrainingScope::between(scope.prefix.clone(), scope.prefix.clone());
    let mut one_move = TrainingSession::scoped(
        &rep,
        only,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        20,
    )
    .unwrap();
    assert_eq!(
        one_move.play_uci("b8c6", 20).unwrap(),
        TrainingVerdict::Finished
    );
}

#[test]
fn scoped_one_side_black_targets_use_context_but_never_count_it() {
    let rep = repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *");
    let scope = TrainingScope::from_prefix(strings(&["e2e4", "e7e5", "g1f3", "b8c6"]));
    let mut session = TrainingSession::scoped(
        &rep,
        scope,
        TrainingMode::Black,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(session.target().unwrap().expected_uci, "b8c6");
    assert_eq!(session.path().len(), 3);
    assert!(session.path().iter().all(|mv| mv.automatic));
    assert!(session.attempts().is_empty());
    assert_eq!(session.play_uci("b8c6", 1).unwrap(), TrainingVerdict::Right);
    assert!(
        session
            .path()
            .iter()
            .any(|mv| mv.uci == "f1b5" && mv.automatic)
    );
    assert_eq!(session.attempts().len(), 1);
    assert_eq!(
        session.play_uci("a7a6", 2).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.attempts().len(), 2);
    assert!(session.path().iter().filter(|mv| mv.automatic).count() >= 2);

    let mut no_context = TrainingSession::scoped(
        &rep,
        TrainingScope::from_prefix(strings(&["e2e4", "e7e5", "g1f3", "b8c6"])).with_context(false),
        TrainingMode::Black,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        10,
    )
    .unwrap();
    assert!(no_context.path().is_empty());
    assert_eq!(no_context.target().unwrap().expected_uci, "b8c6");
    assert_eq!(
        no_context.play_uci("b8c6", 10).unwrap(),
        TrainingVerdict::Right
    );
    assert_eq!(no_context.attempts().len(), 1);
}

#[test]
fn scoped_transpositions_do_not_import_continuations_from_outside_scope() {
    let mut library = Library::default();
    let id = library.create_empty("Transpositions").unwrap();
    library
        .add_game(
            &id,
            &chess_core::parse_str("1. d4 d5 2. Nf3 Nf6 3. g3 *").game,
        )
        .unwrap();
    library
        .add_game(&id, &chess_core::parse_str("1. Nf3 d5 2. d4 Nf6 *").game)
        .unwrap();
    let rep = library.repertoire(&id).unwrap().clone();
    let scope = TrainingScope::from_prefix(strings(&["g1f3"]));
    let mut session = TrainingSession::scoped(
        &rep,
        scope,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(session.play_uci("g1f3", 1).unwrap(), TrainingVerdict::Right);
    assert_eq!(session.play_uci("d7d5", 2).unwrap(), TrainingVerdict::Right);
    assert_eq!(session.play_uci("d2d4", 3).unwrap(), TrainingVerdict::Right);
    assert_eq!(
        session.play_uci("g8f6", 4).unwrap(),
        TrainingVerdict::Finished
    );
    assert!(session.candidates().is_empty());

    let mut whole = TrainingSession::scoped(
        &rep,
        TrainingScope::whole(),
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        10,
    )
    .unwrap();
    for (now, mv) in ["g1f3", "d7d5", "d2d4", "g8f6", "g2g3"]
        .into_iter()
        .enumerate()
    {
        whole.play_uci(mv, 10 + now as i64).unwrap();
    }
    assert!(whole.is_done());
}

#[test]
fn both_review_queue_advances_proposals_but_repeat_keeps_the_same_route() {
    let rep = repertoire("1. e4 e5 (1... c5) 2. Nf3 (2. c4) *");
    let main = strings(&["e2e4", "e7e5", "g1f3"]);
    let second = strings(&["e2e4", "e7e5", "c2c4"]);
    let deviation = strings(&["e2e4", "c7c5"]);
    let scope = TrainingScope::whole();
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(session.suggested_path().unwrap(), main);
    session.play_uci("e2e4", 1).unwrap();
    assert_eq!(
        session.play_uci("c7c5", 2).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(
        session
            .stats()
            .path_stats(&path_key(&rep, &scope, &deviation))
            .unwrap()
            .completions,
        1
    );
    assert!(
        session
            .stats()
            .path_stats(&path_key(&rep, &scope, &main))
            .is_none()
    );

    session.next_path(3).unwrap();
    assert_eq!(session.suggested_path().unwrap(), second);
    session.restart(4).unwrap();
    assert_eq!(session.suggested_path().unwrap(), second);
}

#[test]
fn path_history_records_hints_mistakes_and_last_seen() {
    let rep = repertoire("1. e4 e5 *");
    let scope = TrainingScope::whole();
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    for now in 1..=3 {
        session.play_uci("d2d4", now).unwrap();
    }
    session.hint(4).unwrap();
    session.play_uci("e2e4", 5).unwrap();
    session.play_uci("e7e5", 6).unwrap();
    let path = strings(&["e2e4", "e7e5"]);
    let stats = session
        .stats()
        .path_stats(&path_key(&rep, &scope, &path))
        .unwrap();
    assert_eq!(stats.completions, 1);
    assert_eq!(stats.mistakes, 3);
    assert_eq!(stats.hints, 1);
    assert_eq!(stats.last_seen, Some(6));
    assert!(stats.recent[0].hinted);
}

#[test]
fn both_least_recent_uses_completions_then_oldest_actual_path() {
    let rep = repertoire("1. e4 e5 (1... c5) 2. Nf3 (2. c4) *");
    let paths = [
        strings(&["e2e4", "e7e5", "g1f3"]),
        strings(&["e2e4", "e7e5", "c2c4"]),
        strings(&["e2e4", "c7c5"]),
    ];
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::LeastRecent,
        StatsData::default(),
        0,
    )
    .unwrap();
    for (index, expected) in paths.iter().enumerate() {
        assert_eq!(session.suggested_path().unwrap(), expected);
        for (ply, mv) in expected.iter().enumerate() {
            session.play_uci(mv, (index * 10 + ply) as i64).unwrap();
        }
        assert!(session.is_done());
        session.next_path((index * 10 + 8) as i64).unwrap();
    }
    assert_eq!(session.suggested_path().unwrap(), paths[0]);
}

#[test]
fn repeated_position_is_a_terminal_path_not_a_hang() {
    let rep = repertoire("1. Nf3 Nf6 2. Ng1 Ng8 3. Nf3 Nf6 4. Ng1 Ng8 *");
    let scope = TrainingScope::whole();
    let paths = RepertoireTree::new(&rep).training_paths(&scope).unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].moves, strings(&["g1f3", "g8f6", "f3g1", "f6g8"]));

    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    for (now, mv) in ["g1f3", "g8f6", "f3g1", "f6g8"].into_iter().enumerate() {
        session.play_uci(mv, now as i64).unwrap();
    }
    assert!(session.is_done());
    let stats = session
        .stats()
        .path_stats(&path_key(&rep, &scope, &paths[0].moves))
        .unwrap();
    assert_eq!(stats.completions, 1);
}

#[test]
fn introduction_is_explicit_batches_of_ten_then_unscheduled() {
    let rep = repertoire_from_lines(&[
        "e2e4", "d2d4", "c2c4", "g1f3", "g2g3", "b2b3", "b1c3", "e2e3", "d2d3", "c2c3", "f2f4",
        "a2a3",
    ]);
    let mut first = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    let mut played = 0;
    while !first.is_done() {
        let target = first.target().unwrap().expected_uci.clone();
        first.play_uci(&target, played as i64).unwrap();
        played += 1;
    }
    assert_eq!(played, INTRODUCTION_BATCH_SIZE);
    assert_eq!(first.stats().move_stats.len(), INTRODUCTION_BATCH_SIZE);

    first.restart_with_policy(20, SrsPolicy::Introduce).unwrap();
    let mut remaining = 0;
    while !first.is_done() {
        let target = first.target().unwrap().expected_uci.clone();
        first.play_uci(&target, 20 + remaining).unwrap();
        remaining += 1;
    }
    assert_eq!(remaining, 2);
    assert_eq!(first.stats().move_stats.len(), 12);

    first.restart_with_policy(30, SrsPolicy::Introduce).unwrap();
    assert!(first.is_done());
}

#[test]
fn introduction_takes_unseen_before_previously_reviewed_unscheduled() {
    let rep = repertoire_from_lines(&["e2e4", "d2d4"]);
    let mut review = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    review.play_uci("e2e4", 10).unwrap();
    assert!(review.stats().move_stats[0].due.is_none());

    let mut introduction = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        review.into_stats(),
        20,
        SrsPolicy::Introduce,
    )
    .unwrap();
    assert_eq!(introduction.target().unwrap().expected_uci, "d2d4");
    introduction.play_uci("d2d4", 21).unwrap();
    assert_eq!(introduction.target().unwrap().expected_uci, "e2e4");
    introduction.play_uci("e2e4", 22).unwrap();
    assert!(introduction.is_done());
    assert!(
        introduction
            .stats()
            .move_stats
            .iter()
            .all(|stats| stats.due.is_some())
    );
}

#[test]
fn due_policy_does_not_fall_through_to_unseen_and_early_review_preserves_due() {
    let rep = repertoire_from_lines(&["e2e4", "d2d4"]);
    let mut initial = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    initial.play_uci("e2e4", 100).unwrap();
    initial.restart_with_policy(200, SrsPolicy::Due).unwrap();
    assert!(initial.is_done());
    assert_eq!(initial.stats().move_stats.len(), 1);

    initial
        .restart_with_policy(200, SrsPolicy::Introduce)
        .unwrap();
    assert_eq!(initial.target().unwrap().expected_uci, "d2d4");
    initial.play_uci("d2d4", 201).unwrap();
    let original_due: Vec<Option<i64>> = initial
        .stats()
        .move_stats
        .iter()
        .map(|stats| stats.due)
        .collect();

    initial
        .restart_with_policy(300, SrsPolicy::EarlyReview)
        .unwrap();
    assert_eq!(initial.srs_policy(), SrsPolicy::EarlyReview);
    let target = initial.target().unwrap().expected_uci.clone();
    let key = MoveKey {
        repertoire_id: rep.id.clone(),
        mode: TrainingMode::White,
        position_fen4: initial.target().unwrap().position_fen4.clone(),
        expected_uci: target.clone(),
    };
    let before_attempts = initial.stats().get(&key).unwrap().attempts;
    assert_eq!(
        initial.play_uci("h2h3", 301).unwrap(),
        TrainingVerdict::Wrong
    );
    initial.play_uci(&target, 302).unwrap();
    let stats = initial.stats().get(&key).unwrap();
    assert_eq!(stats.attempts, before_attempts + 2);
    assert_eq!(stats.last_seen, Some(302));
    let after_due: Vec<Option<i64>> = initial
        .stats()
        .move_stats
        .iter()
        .map(|stats| stats.due)
        .collect();
    assert_eq!(after_due, original_due);
}

#[test]
fn path_history_and_scope_cursor_survive_stats_store_roundtrip() {
    let dir = std::env::temp_dir().join(format!("cp-path-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = StatsStore::new(dir.join("stats.json"));
    let rep = repertoire("1. e4 e5 (1... c5) 2. Nf3 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        store.load().unwrap(),
        0,
    )
    .unwrap();
    let completed = session.suggested_path().unwrap().to_vec();
    for (now, mv) in completed.clone().into_iter().enumerate() {
        session.play_uci(&mv, now as i64).unwrap();
    }
    store.save(session.stats_mut()).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(
        loaded
            .path_stats(&path_key(&rep, &TrainingScope::whole(), &completed))
            .unwrap()
            .completions,
        1
    );
    let next = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        loaded,
        10,
    )
    .unwrap();
    assert_ne!(next.suggested_path().unwrap(), completed);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn default_srs_never_introduces_new_targets_or_records_empty_completion() {
    let rep = repertoire("1. e4 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::SpacedRepetition,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert!(session.is_done());
    assert_eq!(session.srs_policy(), SrsPolicy::Due);
    assert!(session.stats().runs.is_empty());
    session.restart(1).unwrap();
    assert!(session.is_done());
    assert!(session.stats().move_stats.is_empty());
}

#[test]
fn repeated_hint_requests_do_not_inflate_hint_statistics() {
    let rep = repertoire("1. e4 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    for now in 1..=3 {
        session.play_uci("d2d4", now).unwrap();
    }
    assert!(session.hint(4).is_some());
    assert!(session.hint(5).is_some());
    session.play_uci("e2e4", 6).unwrap();
    let stats = &session.stats().move_stats[0];
    assert!(stats.key.expected_uci.is_empty());
    assert_eq!((stats.attempts, stats.hints, stats.wrong), (4, 1, 3));
    assert!(session.attempts().iter().all(|a| a.expected_uci.is_empty()));
}

#[test]
fn transposition_continuation_preserves_actual_depth_and_position_counters() {
    let mut library = Library::default();
    let id = library.create_empty("Depth").unwrap();
    for line in [
        "1. d4 d5 2. Nf3 Nf6 3. g3 g6 4. Bg2 *",
        "1. Nh3 Nh6 2. Ng5 Ng4 3. Nf3 Nf6 4. d4 d5 *",
    ] {
        library
            .add_game(&id, &chess_core::parse_str(line).game)
            .unwrap();
    }
    let rep = library.repertoire_mut(&id).unwrap();
    rep.set_max_plies(9);
    let paths = RepertoireTree::new(rep)
        .training_paths(&TrainingScope::whole())
        .unwrap();
    assert!(paths.iter().all(|path| path.moves.len() <= 9));
    let mut session = TrainingSession::new(
        rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    for (i, mv) in [
        "g1h3", "g8h6", "h3g5", "h6g4", "g5f3", "g4f6", "d2d4", "d7d5",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            session.play_uci(mv, i as i64).unwrap(),
            TrainingVerdict::Right
        );
    }
    assert_eq!(
        session.play_uci("g2g3", 9).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.position().fullmoves().get(), 5);
    assert_eq!(session.path().len(), 9);
    assert!(session.candidates().is_empty());
}

#[test]
fn scoped_target_context_cannot_be_borrowed_from_an_outside_transposition() {
    let mut library = Library::default();
    let id = library.create_empty("Scope").unwrap();
    for pgn in ["1. d4 d5 2. Nf3 Nf6 *", "1. Nf3 d5 2. d4 Nf6 3. g3 *"] {
        library
            .add_game(&id, &chess_core::parse_str(pgn).game)
            .unwrap();
    }
    let rep = library.repertoire(&id).unwrap();
    let scope = TrainingScope::from_prefix(strings(&["g1f3"]));
    let targets = RepertoireTree::new(rep).scoped_targets(Color::White, &scope);
    assert!(
        targets
            .iter()
            .all(|t| t.path.is_empty() || t.path[0] == "g1f3")
    );
}

#[test]
fn early_review_preserves_the_next_scheduled_interval() {
    let rep = repertoire("1. e4 *");
    let mut stats = StatsData::default();
    let mut due = 0;
    for interval in [INITIAL_INTERVAL_SECONDS, 2 * INITIAL_INTERVAL_SECONDS] {
        let policy = if due == 0 {
            SrsPolicy::Introduce
        } else {
            SrsPolicy::Due
        };
        let mut session =
            TrainingSession::new_srs(&rep, TrainingMode::White, stats, due, policy).unwrap();
        session.play_uci("e2e4", due).unwrap();
        due += interval;
        stats = session.into_stats();
    }
    let mut review = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        stats,
        due - 100,
        SrsPolicy::EarlyReview,
    )
    .unwrap();
    review.play_uci("e2e4", due - 100).unwrap();
    assert_eq!(
        review.stats().move_stats[0].interval_seconds,
        Some(2 * INITIAL_INTERVAL_SECONDS)
    );
    let mut next = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        review.into_stats(),
        due,
        SrsPolicy::Due,
    )
    .unwrap();
    next.play_uci("e2e4", due).unwrap();
    assert_eq!(
        next.stats().move_stats[0].due,
        Some(due + 4 * INITIAL_INTERVAL_SECONDS)
    );
}

#[test]
fn simultaneous_writers_cannot_overwrite_the_same_revision() {
    let path = std::env::temp_dir().join(format!("cp-trainer-race-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let writers: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut library = Library::default();
                library.create_empty(format!("Writer {i}")).unwrap();
                barrier.wait();
                LibraryStore::new(path).save(&mut library)
            })
        })
        .collect();
    let results: Vec<_> = writers
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Conflict { .. })))
            .count(),
        7
    );
    assert_eq!(
        LibraryStore::new(&path).load().unwrap().repertoires.len(),
        1
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn previous_both_move_keys_migrate_to_position_statistics_without_data_loss() {
    let path =
        std::env::temp_dir().join(format!("cp-trainer-migration-{}.json", std::process::id()));
    let rep = repertoire("1. e4 (1. d4) *");
    let position_fen4 = fen4(Game::new().start().position());
    let entries = ["e2e4", "d2d4"].map(|uci| {
        let mut entry = MoveStats::new(MoveKey {
            repertoire_id: rep.id.clone(),
            mode: TrainingMode::Both,
            position_fen4: position_fen4.clone(),
            expected_uci: uci.into(),
        });
        entry.attempts = 2;
        entry.correct = 1;
        entry.wrong = 1;
        entry.hints = 1;
        entry
    });
    let mut data = StatsData::default();
    data.move_stats = entries.into();
    std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    let store = StatsStore::new(&path);
    let mut migrated = store.load().unwrap();
    assert_eq!(migrated.move_stats.len(), 1);
    let entry = &migrated.move_stats[0];
    assert!(entry.key.expected_uci.is_empty());
    assert_eq!(
        (entry.attempts, entry.correct, entry.wrong, entry.hints),
        (4, 2, 2, 2)
    );
    store.save(&mut migrated).unwrap();
    assert_eq!(store.load().unwrap().move_stats, migrated.move_stats);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn removing_repertoire_cleans_all_modern_statistics_and_cursors_but_keeps_others() {
    let first = repertoire("1. e4 (1. d4) *");
    let second = repertoire("1. Nf3 (1. c4) *");
    let mut data = StatsData::default();
    data.legacy_lines
        .insert("original".into(), serde_json::json!({"runs": 3}));
    for rep in [&first, &second] {
        for mode in [TrainingMode::White, TrainingMode::Both] {
            let mut session =
                TrainingSession::new(rep, mode, QueueMode::ReviewInOrder, data, 0).unwrap();
            while !session.is_done() {
                let mv = session
                    .target()
                    .map(|t| t.expected_uci.clone())
                    .unwrap_or_else(|| session.candidates()[0].uci.clone());
                session.play_uci(&mv, 1).unwrap();
            }
            data = session.into_stats();
        }
    }
    let before = serde_json::to_value(&data).unwrap();
    for field in [
        "move_stats",
        "runs",
        "path_stats",
        "review_cursors",
        "path_review_cursors",
    ] {
        assert!(
            before[field].as_array().unwrap().iter().any(|entry| {
                entry
                    .get("repertoire_id")
                    .or_else(|| entry.get("key").and_then(|key| key.get("repertoire_id")))
                    == Some(&serde_json::json!(first.id))
            }),
            "missing fixture for {field}"
        );
    }
    data.remove_repertoire(&first.id);
    let after = serde_json::to_value(&data).unwrap();
    for field in [
        "move_stats",
        "runs",
        "path_stats",
        "review_cursors",
        "path_review_cursors",
    ] {
        let expected: Vec<_> = before[field]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| {
                entry
                    .get("repertoire_id")
                    .or_else(|| entry.get("key").and_then(|key| key.get("repertoire_id")))
                    == Some(&serde_json::json!(second.id))
            })
            .cloned()
            .collect();
        assert_eq!(
            after[field],
            serde_json::json!(expected),
            "incorrect cleanup of {field}"
        );
    }
    assert_eq!(after["legacy_lines"], before["legacy_lines"]);
    data.remove_repertoire(&first.id);
    assert_eq!(serde_json::to_value(&data).unwrap(), after);
}

#[test]
fn streaming_paths_stop_early_and_match_full_queue_across_wraps_and_history() {
    use std::ops::ControlFlow;
    let rep = repertoire(
        "1. d4 d5 2. Nf3 Nf6 (2... Nc6) (1... Nf6 2. Nf3 d5 3. c4) (1. Nf3 d5 2. d4 Nf6 3. c4) *",
    );
    let tree = RepertoireTree::new(&rep);
    let scopes = [
        TrainingScope::whole(),
        TrainingScope::from_prefix(strings(&["d2d4"])),
    ];
    for scope in scopes {
        let paths = tree.training_paths(&scope).unwrap();
        assert!(!paths.is_empty());
        assert_eq!(tree.training_path_count(&scope).unwrap(), paths.len());
        let mut visited = 0;
        tree.visit_training_paths(&scope, |moves| {
            visited += 1;
            assert_eq!(moves, paths[0].moves);
            ControlFlow::Break(())
        })
        .unwrap();
        assert_eq!(visited, 1);
        let mut stats = StatsData::default();
        for step in 0..paths.len() * 3 {
            for mode in [
                QueueMode::ReviewInOrder,
                QueueMode::LeastRecent,
                QueueMode::SpacedRepetition,
            ] {
                let full = stats.path_queue(&tree, &rep.id, &scope, mode).unwrap();
                assert_eq!(
                    stats
                        .next_training_path(&tree, &rep.id, &scope, mode)
                        .unwrap(),
                    full.first().cloned()
                );
            }
            let path = &paths[step % paths.len()];
            stats.record_path_run(
                path_key(&rep, &scope, &path.moves),
                RunRecord {
                    at: step as i64,
                    mistakes: 0,
                    hints: 0,
                    hinted: false,
                    path: path.moves.clone(),
                    user_attempts: path.moves.len() as u32,
                    automatic_moves: 0,
                    scope: scope.key(),
                },
            );
            stats.path_presented(&rep.id, &scope, QueueMode::ReviewInOrder);
        }
    }
    let empty = RepertoireTree::new(&repertoire("*"));
    assert_eq!(
        empty.training_path_count(&TrainingScope::whole()).unwrap(),
        0
    );
    assert_eq!(
        StatsData::default()
            .next_training_path(
                &empty,
                &rep.id,
                &TrainingScope::whole(),
                QueueMode::ReviewInOrder
            )
            .unwrap(),
        None
    );
}

#[test]
fn null_subtree_grafts_under_the_exact_path_instead_of_an_earlier_transposition() {
    let base = chess_core::parse_str("1. d4 (1. Nf3 d5 2. d4) d5 2. Nf3 *").game;
    let source = chess_core::parse_str(
        "1. Nf3 d5 2. d4 -- {teaching pass} 3. c4 $1 (3. e4 {side branch} $2) *",
    )
    .game;
    let original = source.canonical_tree();
    let mut library = Library::default();
    let id = library.create("Exact graft", &base).unwrap();
    let report = library.add_game(&id, &source).unwrap();
    assert_eq!(report.added_nodes, 3);
    let merged = library.repertoire(&id).unwrap().game();
    let main_end = *merged.mainline().last().unwrap();
    assert!(merged.children(main_end).is_empty());
    let alternative = merged.children(NodeId::ROOT)[1];
    let d5 = merged.children(alternative)[0];
    let d4 = merged.children(d5)[0];
    let null = merged.children(d4)[0];
    assert!(merged.game_move(null).unwrap().is_null());
    assert_eq!(
        merged.node(null).unwrap().comments_after(),
        &["teaching pass"]
    );
    let c4 = merged.children(null)[0];
    assert_eq!(merged.game_move(c4).unwrap().san, "c4");
    assert_eq!(merged.node(c4).unwrap().nags()[0].0, 1);
    assert_eq!(merged.start().fen(), source.start().fen());
    // Match each complete source path, including every RAV under the null move.
    // Compare position counters and annotations as well as moves: remapping the
    // graft must not retain any IDs from the temporary planning document.
    for source_id in source.attached_nodes() {
        let mut target_id = NodeId::ROOT;
        for step in source.path(source_id).into_iter().skip(1) {
            let kind = source.game_move(step).unwrap().kind;
            target_id = merged
                .children(target_id)
                .iter()
                .copied()
                .find(|id| merged.game_move(*id).is_some_and(|mv| mv.kind == kind))
                .expect("the complete source path survives the graft");
        }
        let expected = source.node(source_id).unwrap();
        let actual = merged.node(target_id).unwrap();
        assert_eq!(actual.comments_before(), expected.comments_before());
        assert_eq!(actual.comments_after(), expected.comments_after());
        assert_eq!(actual.nags(), expected.nags());
        assert_eq!(actual.unparsed(), expected.unparsed());
        assert_eq!(merged.fen(target_id), source.fen(source_id));
    }
    assert_eq!(source.canonical_tree(), original);
    let canonical = merged.canonical_tree();
    assert_eq!(library.add_game(&id, &source).unwrap().added_nodes, 0);
    assert_eq!(
        library.repertoire(&id).unwrap().game().canonical_tree(),
        canonical
    );
}

#[test]
fn practice_follows_the_users_branch_without_hidden_targets_or_automatic_attempts() {
    let rep = repertoire("1. e4 (1. d4 d5 2. c4) e5 2. Nf3 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::Practice,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert!(session.target().is_none());
    assert!(session.may_hint());
    assert_eq!(session.play_uci("d2d4", 1).unwrap(), TrainingVerdict::Right);
    assert_eq!(
        session
            .path()
            .iter()
            .map(|m| m.uci.as_str())
            .collect::<Vec<_>>(),
        vec!["d2d4", "d7d5"]
    );
    assert_eq!(session.attempts().len(), 1);
    assert_eq!(
        session.play_uci("c2c4", 2).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.play_uci("a2a3", 3).unwrap(), TrainingVerdict::Done);
    let stats = session.stats();
    assert_eq!(stats.move_stats.len(), 2);
    assert!(
        stats
            .move_stats
            .iter()
            .all(|m| m.correct == 1 && m.due.is_none())
    );
    assert_eq!(
        stats
            .runs(&rep.id, TrainingMode::White)
            .unwrap()
            .completions,
        1
    );
}

#[test]
fn practice_black_starts_with_an_automatic_white_move_and_rotates_opponent_branches() {
    let rep = repertoire("1. e4 e5 (1... c5) 2. Nf3 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::Practice,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(session.play_uci("e2e4", 1).unwrap(), TrainingVerdict::Right);
    assert_eq!(session.path()[1].uci, "e7e5");
    session.next_path(2).unwrap();
    assert_eq!(
        session.play_uci("e2e4", 3).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.path()[1].uci, "c7c5");
    session.restart(4).unwrap();
    session.play_uci("e2e4", 5).unwrap();
    assert_eq!(session.path()[1].uci, "c7c5");
    let saved = serde_json::to_vec(session.stats()).unwrap();
    let stats: StatsData = serde_json::from_slice(&saved).unwrap();
    let mut black =
        TrainingSession::new(&rep, TrainingMode::Black, QueueMode::Practice, stats, 6).unwrap();
    assert_eq!(black.path()[0].uci, "e2e4");
    assert!(black.path()[0].automatic);
    assert!(black.attempts().is_empty());
    assert_eq!(
        black.play_uci("c7c5", 7).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(black.attempts().len(), 1);
}

#[test]
fn practice_wrong_move_stays_put_and_hint_is_available_before_any_error() {
    let rep = repertoire("1. e4 e5 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::Practice,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert!(session.hint(1).is_some());
    let position = session.position_key();
    assert_eq!(session.play_uci("d2d4", 2).unwrap(), TrainingVerdict::Wrong);
    assert_eq!(session.position_key(), position);
    assert_eq!(session.total_mistakes(), 1);
    assert_eq!(
        session.play_uci("e2e4", 3).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.stats().move_stats[0].hints, 1);
    assert_eq!(session.stats().move_stats[0].wrong, 1);
    assert_eq!(session.stats().move_stats[0].correct, 1);
    assert!(session.stats().move_stats[0].due.is_none());
}

#[test]
fn practice_obeys_depth_disabled_repetition_and_both_never_answers_automatically() {
    let mut rep = repertoire("1. Nf3 (1. d4 d5) Nf6 2. Ng1 Ng8 3. e4 *");
    rep.set_prefix_disabled(&strings(&["d2d4"]), true).unwrap();
    let mut both = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::Practice,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(both.play_uci("d2d4", 1).unwrap(), TrainingVerdict::Wrong);
    for uci in ["g1f3", "g8f6", "f3g1"] {
        assert_eq!(both.play_uci(uci, 2).unwrap(), TrainingVerdict::Right);
    }
    assert_eq!(both.play_uci("f6g8", 3).unwrap(), TrainingVerdict::Finished);
    assert!(both.path().iter().all(|m| !m.automatic));
    assert_eq!(both.path().len(), 4);
    rep.max_plies = 1;
    let mut white = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::Practice,
        StatsData::default(),
        4,
    )
    .unwrap();
    assert_eq!(
        white.play_uci("g1f3", 5).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(white.path().len(), 1);
}

#[test]
fn scheduled_alternative_is_credited_only_to_actual_move_and_hidden_target_stays_unseen() {
    let rep = repertoire("1. e4 (1. d4) *");
    let mut session = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    assert_eq!(session.target().unwrap().expected_uci, "e2e4");
    assert_eq!(
        session.play_uci("d2d4", 1).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.total_mistakes(), 0);
    assert_eq!(session.stats().move_stats.len(), 1);
    assert_eq!(session.stats().move_stats[0].key.expected_uci, "d2d4");
    assert_eq!(session.stats().move_stats[0].correct, 1);
    let next = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        session.into_stats(),
        2,
        SrsPolicy::Introduce,
    )
    .unwrap();
    assert_eq!(next.target().unwrap().expected_uci, "e2e4");
}

#[test]
fn practice_preserves_existing_srs_due_and_interval() {
    let rep = repertoire("1. e4 *");
    let mut srs = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    srs.play_uci("e2e4", 1).unwrap();
    let due = srs.stats().move_stats[0].due;
    let interval = srs.stats().move_stats[0].interval_seconds;
    let mut practice = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::Practice,
        srs.into_stats(),
        2,
    )
    .unwrap();
    practice.play_uci("e2e4", 3).unwrap();
    assert_eq!(practice.stats().move_stats[0].due, due);
    assert_eq!(practice.stats().move_stats[0].interval_seconds, interval);
}

#[test]
fn practice_scope_replay_origin_precedes_automatic_moves_and_preserves_counters() {
    let rep = repertoire(
        "[SetUp \"1\"]\n[FEN \"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 7 42\"]\n\n42. e4 e5 *",
    );
    let session = TrainingSession::scoped(
        &rep,
        TrainingScope::whole().with_context(false),
        TrainingMode::Black,
        QueueMode::Practice,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(session.scope_start_position().turn(), Color::White);
    assert_eq!(session.position().turn(), Color::Black);
    assert_eq!(session.path().len(), 1);
    assert_eq!(session.scope_start_position().fullmoves().get(), 42);
    assert_eq!(session.scope_start_position().halfmoves(), 7);
}

#[test]
fn practice_next_branch_hint_follows_proposal_and_both_path_completion_is_once() {
    let rep = repertoire("1. e4 (1. d4 d5) e5 (1... c5) *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::Practice,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert_eq!(session.hint(1).unwrap().uci, "e2e4");
    session.next_path(2).unwrap();
    assert_eq!(session.hint(3).unwrap().uci, "e2e4");
    session.next_path(4).unwrap();
    assert_eq!(session.hint(5).unwrap().uci, "d2d4");
    assert_eq!(session.play_uci("d2d4", 6).unwrap(), TrainingVerdict::Right);
    assert_eq!(session.hint(7).unwrap().uci, "d7d5");
    assert_eq!(
        session.play_uci("d7d5", 8).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.play_uci("c2c4", 9).unwrap(), TrainingVerdict::Done);
    let paths = &session.stats().path_stats;
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].key.path, strings(&["d2d4", "d7d5"]));
    assert_eq!(paths[0].completions, 1);
    assert_eq!(
        session
            .stats()
            .runs(&rep.id, TrainingMode::Both)
            .unwrap()
            .completions,
        1
    );
    session.next_path(10).unwrap();
    assert_eq!(session.hint(11).unwrap().uci, "e2e4");
    // Deviating to another valid branch makes the hint follow that actual branch.
    session.play_uci("d2d4", 12).unwrap();
    assert_eq!(session.hint(13).unwrap().uci, "d7d5");
}

#[test]
fn random_practice_is_deterministic_and_covers_every_route_before_a_new_cycle() {
    let rep = repertoire("1. e4 e5 (1... c5) 2. Nf3 (2. c4) *");
    let expected: std::collections::BTreeSet<Vec<String>> = RepertoireTree::new(&rep)
        .training_paths(&TrainingScope::whole())
        .unwrap()
        .into_iter()
        .map(|path| path.moves)
        .collect();
    assert_eq!(expected.len(), 3);
    let mut left = random_session(&rep, TrainingMode::Both, 42);
    let mut right = random_session(&rep, TrainingMode::Both, 42);
    let mut routes = Vec::new();
    for step in 0..6 {
        assert_eq!(left.scope(), right.scope());
        assert_eq!(random_route(&left), random_route(&right));
        routes.push(random_route(&left));
        if step < 5 {
            left.next_path(step).unwrap();
            right.next_path(step).unwrap();
        }
    }
    for cycle in routes.chunks(3) {
        let actual: std::collections::BTreeSet<Vec<String>> = cycle.iter().cloned().collect();
        assert_eq!(actual.len(), 3);
        assert_eq!(actual, expected);
    }
}

#[test]
fn random_practice_mid_path_context_has_the_correct_side_for_every_mode() {
    let rep = repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *");
    for mode in [TrainingMode::White, TrainingMode::Black, TrainingMode::Both] {
        let mut session = None;
        for seed in 0..64 {
            let candidate = random_session(&rep, mode, seed);
            if !candidate.path().is_empty() {
                session = Some(candidate);
                break;
            }
        }
        let mut session = session.expect("a deterministic mid-path start");
        let start = session.path().len();
        assert!(start > 0);
        assert!(session.path().iter().all(|played| played.automatic));
        assert!(session.attempts().is_empty());
        assert_eq!(session.scope().prefix.len(), start + 1);
        assert_eq!(
            session.scope().prefix.last().unwrap(),
            &session.suggested_path().unwrap()[0]
        );
        assert_eq!(random_route(&session).len(), 6);
        assert_eq!(
            session.position_key(),
            session.path().last().unwrap().position_after
        );
        if let Some(color) = mode.user_color() {
            assert_eq!(session.position().turn(), color);
        }
        let target = session.suggested_path().unwrap()[0].clone();
        assert!(matches!(
            session.play_uci(&target, 1).unwrap(),
            TrainingVerdict::Right | TrainingVerdict::Finished
        ));
        assert!(!session.path()[start].automatic);
    }
}

#[test]
fn random_repeat_preserves_actual_route_start_and_the_next_cursor() {
    let rep = repertoire("1. e4 e5 (1... c5) 2. Nf3 (2. c4) *");
    let mut session = (0..64)
        .map(|seed| random_session(&rep, TrainingMode::Both, seed))
        .find(|session| !session.path().is_empty())
        .expect("a deterministic mid-path start");
    let initial_scope = session.scope().clone();
    let initial_context = session.path().to_vec();
    let selected = session.suggested_path().unwrap().to_vec();
    for (index, mv) in selected.iter().enumerate() {
        session.play_uci(mv, index as i64).unwrap();
    }
    assert!(session.is_done());
    let completed_path = session.path().to_vec();
    let completed_route = random_route(&session);
    let before_repeat = serde_json::to_vec(session.stats()).unwrap();

    session.restart(10).unwrap();
    assert_eq!(session.scope(), &initial_scope);
    assert_eq!(session.path(), initial_context);
    assert_eq!(serde_json::to_vec(session.stats()).unwrap(), before_repeat);
    for (index, mv) in selected.iter().enumerate() {
        session.play_uci(mv, 10 + index as i64).unwrap();
    }
    assert_eq!(session.path(), completed_path);

    session.next_path(20).unwrap();
    let next = random_route(&session);
    assert_ne!(next, completed_route);
    assert!(
        RepertoireTree::new(&rep)
            .training_paths(&TrainingScope::whole())
            .unwrap()
            .iter()
            .any(|path| path.moves == next)
    );
}

#[test]
fn random_practice_path_count_change_starts_a_fresh_complete_cycle() {
    let rep = repertoire("1. e4 (1. d4 d5) e5 *");
    let mut initial = random_session(&rep, TrainingMode::Both, 7);
    initial.next_path(1).unwrap();
    let before = serde_json::to_value(initial.stats()).unwrap();
    let old_seed = before["random_practice_cursors"][0]["seed"]
        .as_u64()
        .unwrap();
    let stats = initial.into_stats();
    let mut expanded = rep.clone();
    expanded.pgn = chess_core::parse_str("1. e4 (1. d4 d5) (1. c4 e5) (1. Nf3 Nf6) e5 (1... c5) *")
        .game
        .to_pgn();
    let mut session = TrainingSession::scoped_random_with_seed(
        &expanded,
        TrainingScope::whole(),
        TrainingMode::Both,
        stats,
        2,
        7,
    )
    .unwrap();
    let after = serde_json::to_value(session.stats()).unwrap();
    assert_ne!(
        after["random_practice_cursors"][0]["seed"]
            .as_u64()
            .unwrap(),
        old_seed
    );
    assert_eq!(after["random_practice_cursors"][0]["position"], 1);
    assert_eq!(after["random_practice_cursors"][0]["count"], 5);

    let mut routes = vec![random_route(&session)];
    for now in 3..7 {
        session.next_path(now).unwrap();
        routes.push(random_route(&session));
    }
    let unique: std::collections::BTreeSet<Vec<String>> = routes.into_iter().collect();
    assert_eq!(unique.len(), 5);
}

#[test]
fn random_practice_reads_old_stats_without_the_random_cursor_field() {
    let rep = repertoire("1. e4 e5 (1... c5) *");
    let mut old = serde_json::to_value(StatsData::default()).unwrap();
    assert!(
        old.as_object_mut()
            .unwrap()
            .remove("random_practice_cursors")
            .is_some()
    );
    let stats: StatsData = serde_json::from_value(old).unwrap();
    let session = TrainingSession::scoped_random_with_seed(
        &rep,
        TrainingScope::whole(),
        TrainingMode::Both,
        stats,
        0,
        11,
    )
    .unwrap();
    assert!(session.suggested_path().is_some());
    let saved = serde_json::to_value(session.stats()).unwrap();
    assert_eq!(saved["random_practice_cursors"][0]["position"], 1);
}

#[test]
fn random_both_result_finishes_on_the_real_last_ply_beyond_legacy_depth() {
    let mut rep = repertoire("1. e4 e5 2. Nf3 Nc6 *");
    rep.max_plies = 1;
    let mut session = (0..64)
        .map(|seed| random_session(&rep, TrainingMode::Both, seed))
        .find(|session| {
            random_route(session).len() == 4 && session.suggested_path().unwrap().len() == 1
        })
        .expect("a deterministic start at the final ply");
    assert_eq!(session.path().len(), 3);
    assert!(session.path().iter().all(|played| played.automatic));
    assert_eq!(
        session.play_uci("b8c6", 1).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.path().len(), 4);
    assert_eq!(session.path().last().unwrap().uci, "b8c6");
    assert!(!session.path().last().unwrap().automatic);
    let record = session
        .stats()
        .runs(&rep.id, TrainingMode::Both)
        .unwrap()
        .recent
        .last()
        .unwrap();
    assert_eq!(record.path, strings(&["b8c6"]));
    assert_eq!(record.automatic_moves, 3);
    assert_eq!(record.user_attempts, 1);
}

#[test]
fn random_scoped_start_never_escapes_the_base_end_or_creates_an_invalid_scope() {
    let rep = repertoire("1. e4 e5 2. Nf3 Nc6 (2... Nf6) 3. Bb5 a6 *");
    let prefix = strings(&["e2e4", "e7e5", "g1f3"]);
    let end = strings(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"]);
    let base = TrainingScope::between(prefix.clone(), end.clone());
    let mut saw_short = false;
    let mut saw_end = false;
    for seed in 0..64 {
        let session = TrainingSession::scoped_random_with_seed(
            &rep,
            base.clone(),
            TrainingMode::Both,
            StatsData::default(),
            0,
            seed,
        )
        .unwrap();
        let route = random_route(&session);
        assert!(route.starts_with(&prefix));
        assert!(route.len() <= end.len());
        assert_eq!(session.scope().end_prefix.as_ref(), Some(&route));
        saw_short |= route.len() < end.len();
        saw_end |= route == end;
    }
    assert!(saw_short && saw_end);
}

#[test]
fn random_practice_pgn_fingerprint_change_restarts_the_cycle_at_the_same_count() {
    let rep = repertoire("1. e4 (1. d4 d5) e5 *");
    let initial = random_session(&rep, TrainingMode::Both, 19);
    let before = serde_json::to_value(initial.stats()).unwrap();
    let old_seed = before["random_practice_cursors"][0]["seed"]
        .as_u64()
        .unwrap();
    let old_fingerprint = before["random_practice_cursors"][0]["fingerprint"].clone();
    assert_eq!(before["random_practice_cursors"][0]["position"], 1);
    assert_eq!(before["random_practice_cursors"][0]["count"], 2);

    let mut changed = rep.clone();
    changed.pgn = chess_core::parse_str("1. c4 (1. Nf3 Nf6) e5 *")
        .game
        .to_pgn();
    let mut session = TrainingSession::scoped_random_with_seed(
        &changed,
        TrainingScope::whole(),
        TrainingMode::Both,
        initial.into_stats(),
        1,
        19,
    )
    .unwrap();
    let after = serde_json::to_value(session.stats()).unwrap();
    assert_eq!(after["random_practice_cursors"][0]["count"], 2);
    assert_eq!(after["random_practice_cursors"][0]["position"], 1);
    assert_ne!(
        after["random_practice_cursors"][0]["seed"]
            .as_u64()
            .unwrap(),
        old_seed
    );
    assert_ne!(
        after["random_practice_cursors"][0]["fingerprint"],
        old_fingerprint
    );
    let first = random_route(&session);
    session.next_path(2).unwrap();
    assert_ne!(random_route(&session), first);
}

#[test]
fn random_practice_cursor_without_fingerprint_safely_starts_a_new_cycle() {
    let rep = repertoire("1. e4 (1. d4 d5) e5 *");
    let initial = random_session(&rep, TrainingMode::Both, 23);
    let mut old = serde_json::to_value(initial.stats()).unwrap();
    let cursor = old["random_practice_cursors"][0].as_object_mut().unwrap();
    assert!(cursor.remove("fingerprint").is_some());
    let old_seed = cursor["seed"].as_u64().unwrap();
    assert_eq!(cursor["position"], 1);
    let stats: StatsData = serde_json::from_value(old).unwrap();

    let session = TrainingSession::scoped_random_with_seed(
        &rep,
        TrainingScope::whole(),
        TrainingMode::Both,
        stats,
        1,
        23,
    )
    .unwrap();
    let after = serde_json::to_value(session.stats()).unwrap();
    assert!(after["random_practice_cursors"][0]["fingerprint"].is_number());
    assert_eq!(after["random_practice_cursors"][0]["position"], 1);
    assert_ne!(
        after["random_practice_cursors"][0]["seed"]
            .as_u64()
            .unwrap(),
        old_seed
    );
}
