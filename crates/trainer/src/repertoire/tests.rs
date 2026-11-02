use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use chess_core::shakmaty::{Chess, Move, Position};
use chess_core::{Game, MoveKind, NodeId, StartPosition};
use serde_json::json;

use super::*;

static TEST_DIR: AtomicU64 = AtomicU64::new(1);

fn game(pgn: &str) -> Game {
    chess_core::parse_str(pgn).game
}

fn repertoire(pgn: &str) -> Repertoire {
    let mut library = Library::default();
    let id = library.create("Test", &game(pgn)).unwrap();
    library.repertoire(&id).unwrap().clone()
}

fn play(game: &Game, line: &[&str]) -> Vec<Move> {
    let mut position = game.start().position().clone();
    line.iter()
        .map(|text| {
            let mv = text
                .parse::<chess_core::shakmaty::uci::UciMove>()
                .unwrap()
                .to_move(&position)
                .unwrap();
            position.play_unchecked(mv);
            mv
        })
        .collect()
}

fn temp_dir(name: &str) -> PathBuf {
    let sequence = TEST_DIR.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cp-repertoire-{}-{sequence}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn prefix_branch_dedup_and_annotations_merge_without_touching_source() {
    let source = game("1. e4 {old} $1 c5 2. Nf3 d6 3. Bb5+ (3. d4 cxd4 4. Nxd4 (4. Qxd4)) *");
    let original = source.to_pgn();
    let canonical = source.canonical_tree();
    let mut library = Library::default();
    let id = library.create_empty("Sicilian").unwrap();

    let prefix_end = source.mainline()[3];
    let report = library.add_prefix(&id, &source, prefix_end).unwrap();
    assert_eq!(report.added_nodes, 4);
    let snapshot = library.repertoire(&id).unwrap().game();
    assert_eq!(snapshot.mainline().len(), 4);
    assert_eq!(
        snapshot.game_move(snapshot.mainline()[2]).unwrap().san,
        "Nf3"
    );
    assert!(!snapshot.to_pgn().contains("Bb5"));
    assert!(!snapshot.to_pgn().contains("cxd4"));

    let annotated = game("1. e4 {new} $2 c5 2. Nf3 Nc6 3. d4 *");
    library.add_game(&id, &annotated).unwrap();
    library.add_game(&id, &annotated).unwrap();
    library.add_game(&id, &source).unwrap();
    let merged = library.repertoire(&id).unwrap().game();
    let e4 = merged.main_child(NodeId::ROOT).unwrap();
    assert_eq!(merged.node(e4).unwrap().comments_after(), ["old", "new"]);
    assert_eq!(
        merged
            .node(e4)
            .unwrap()
            .nags()
            .iter()
            .map(|n| n.0)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    let moves = play(&merged, &["e2e4", "c7c5", "g1f3"]);
    let c5 = merged.find_child(e4, moves[1]).unwrap();
    let nf3 = merged.find_child(c5, moves[2]).unwrap();
    assert_eq!(merged.children(nf3).len(), 2);
    assert_eq!(
        library
            .repertoire(&id)
            .unwrap()
            .game()
            .attached_nodes()
            .len(),
        12
    );

    let before = library.repertoire(&id).unwrap().pgn.clone();
    let repeat = library.add_game(&id, &source).unwrap();
    assert_eq!(repeat.added_nodes, 0);
    assert_eq!(library.repertoire(&id).unwrap().pgn, before);
    assert_eq!(source.to_pgn(), original);
    assert_eq!(source.canonical_tree(), canonical);
}

#[test]
fn different_starts_are_rejected_and_fullmove_zero_is_preserved() {
    let fen_game =
        game("[SetUp \"1\"]\n[FEN \"4k3/8/8/8/8/8/4P3/4K2R b - - 0 0\"]\n\n1... Ke7 2. e4 *");
    let mut library = Library::default();
    let id = library.create("Black to move", &fen_game).unwrap();
    assert_eq!(
        library
            .repertoire(&id)
            .unwrap()
            .game()
            .start()
            .original_fen(),
        Some("4k3/8/8/8/8/8/4P3/4K2R b - - 0 0")
    );
    let before = library.repertoire(&id).unwrap().pgn.clone();
    assert!(matches!(
        library.add_game(&id, &game("1. e4 *")),
        Err(RepertoireError::DifferentStart { .. })
    ));
    assert_eq!(library.repertoire(&id).unwrap().pgn, before);
}

#[test]
fn null_and_unparsed_segments_survive_but_are_not_candidates() {
    let source = game("1. e4 -- (1... e5) 2. d4 *");
    let mut library = Library::default();
    let id = library.create("Teaching", &game("1. e4 e5 *")).unwrap();
    library.add_game(&id, &source).unwrap();
    let stored = library.repertoire(&id).unwrap();
    assert!(stored.pgn.contains("--"));
    let tree = RepertoireTree::new(stored);
    let stored_game = tree.game();
    let e4 = stored_game.main_child(NodeId::ROOT).unwrap();
    let null = stored_game
        .children(e4)
        .iter()
        .copied()
        .find(|node| {
            stored_game
                .game_move(*node)
                .is_some_and(|mv| mv.kind == MoveKind::Null)
        })
        .unwrap();
    assert!(tree.candidates(null).is_empty());
    assert!(tree.candidates(e4).iter().any(|c| c.uci == "e7e5"));
    assert!(tree.candidates(e4).iter().all(|c| c.uci != "0000"));

    let illegal = game("1. e4 e5 2. e5 *");
    assert!(illegal.canonical_tree().contains("U["));
    let mut illegal_library = Library::default();
    let illegal_id = illegal_library.create("Raw", &illegal).unwrap();
    let roundtrip = illegal_library.repertoire(&illegal_id).unwrap().game();
    assert_eq!(roundtrip.canonical_tree(), illegal.canonical_tree());
    assert!(
        illegal_library
            .add_game(&illegal_id, &game("1. e4 e5 2. Nf3 *"))
            .is_ok()
    );
    let mut target = Library::default();
    let target_id = target.create("Target", &game("1. e4 e5 *")).unwrap();
    let target_before = target.repertoire(&target_id).unwrap().pgn.clone();
    assert!(target.add_game(&target_id, &illegal).is_err());
    assert_eq!(target.repertoire(&target_id).unwrap().pgn, target_before);

    let mut empty = Library::default();
    let empty_id = empty.create_empty("Empty").unwrap();
    empty.add_game(&empty_id, &illegal).unwrap();
    assert_eq!(
        empty.repertoire(&empty_id).unwrap().game().canonical_tree(),
        illegal.canonical_tree()
    );

    let mut prefixes = Library::default();
    let prefix_id = prefixes.create("Prefix", &game("1. e4 *")).unwrap();
    let null_node = source.mainline()[1];
    prefixes.add_prefix(&prefix_id, &source, null_node).unwrap();
    let prefix_pgn = &prefixes.repertoire(&prefix_id).unwrap().pgn;
    assert!(prefix_pgn.contains("--"));
    assert!(!prefix_pgn.contains("d4"));
}

#[test]
fn transpositions_share_candidates_but_keep_both_pgn_paths() {
    let mut library = Library::default();
    let id = library.create_empty("Transpositions").unwrap();
    library
        .add_game(&id, &game("1. d4 d5 2. Nf3 Nf6 3. g3 *"))
        .unwrap();
    library
        .add_game(&id, &game("1. Nf3 d5 2. d4 Nf6 *"))
        .unwrap();
    let stored = library.repertoire(&id).unwrap();
    let tree = RepertoireTree::new(stored);
    let game = tree.game();
    let first_path = ["d2d4", "d7d5", "g1f3", "g8f6"];
    let second_path = ["g1f3", "d7d5", "d2d4", "g8f6"];
    let first = tree.node_at_path(&first_path.map(str::to_owned)).unwrap();
    let second = tree.node_at_path(&second_path.map(str::to_owned)).unwrap();
    assert_ne!(first, second);
    assert_eq!(tree.position_key(first), tree.position_key(second));
    assert!(tree.transpositions(first).contains(&second));
    assert!(tree.candidates(second).iter().any(|c| c.uci == "g2g3"));
    let pgn = stored.pgn.clone();
    assert!(pgn.contains("d4"));
    assert!(pgn.contains("Nf3"));
    assert_eq!(game.attached_nodes().len(), 10);
}

#[test]
fn fen4_ignores_counters_but_distinguishes_side_castling_and_legal_ep() {
    let position = |fen: &str| StartPosition::from_fen(fen).unwrap().position().clone();
    let a = position("4k3/8/8/8/8/8/4P3/4K2R w K - 0 1");
    let counters = position("4k3/8/8/8/8/8/4P3/4K2R w K - 37 91");
    let black = position("4k3/8/8/8/8/8/4P3/4K2R b K - 0 1");
    let no_rights = position("4k3/8/8/8/8/8/4P3/4K2R w - - 0 1");
    assert_eq!(fen4(&a), fen4(&counters));
    assert_ne!(fen4(&a), fen4(&black));
    assert_ne!(fen4(&a), fen4(&no_rights));

    let legal_ep = position("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2");
    let no_ep = position("4k3/8/8/3pP3/8/8/8/4K3 w - - 0 2");
    let illegal_ep = position("4k3/8/8/3p4/8/8/8/4K3 w - d6 0 2");
    let illegal_no_ep = position("4k3/8/8/3p4/8/8/8/4K3 w - - 0 2");
    assert_ne!(fen4(&legal_ep), fen4(&no_ep));
    assert_eq!(fen4(&illegal_ep), fen4(&illegal_no_ep));
}

#[test]
fn library_crud_depth_and_disable_are_persistent_data() {
    let mut library = Library::default();
    let id = library.create("  Name  ", &game("1. e4 e5 *")).unwrap();
    assert_eq!(library.repertoire(&id).unwrap().name, "Name");
    assert_eq!(library.repertoire(&id).unwrap().max_plies, 20);
    library.rename(&id, "Renamed").unwrap();
    assert!(library.rename(&id, " ").is_err());
    let repertoire = library.repertoire_mut(&id).unwrap();
    repertoire.set_max_plies(1);
    repertoire
        .set_prefix_disabled(&["e2e4".into()], true)
        .unwrap();
    repertoire
        .set_prefix_disabled(&["e2e4".into()], true)
        .unwrap();
    assert_eq!(repertoire.disabled_prefixes.len(), 1);
    assert!(
        RepertoireTree::new(repertoire)
            .candidates(NodeId::ROOT)
            .is_empty()
    );
    assert_eq!(library.delete(&id).unwrap().name, "Renamed");
    assert!(library.repertoire(&id).is_none());
}

#[test]
fn both_accepts_every_enabled_branch_and_rejects_outside_disabled_and_depth() {
    let rep = repertoire("1. e4 (1. d4 d5) e5 (1... c5) 2. Nf3 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    assert!(session.play_uci("not-a-move", 0).is_err());
    assert!(session.play_uci("e2e5", 0).is_err());
    assert!(session.attempts().is_empty());
    assert_eq!(session.play_uci("c2c4", 1).unwrap(), TrainingVerdict::Wrong);
    assert_eq!(session.current_node(), NodeId::ROOT);
    assert_eq!(session.total_mistakes(), 1);
    assert_eq!(session.play_uci("d2d4", 2).unwrap(), TrainingVerdict::Right);
    assert_eq!(
        session.play_uci("d7d5", 3).unwrap(),
        TrainingVerdict::Finished
    );
    let completions = session
        .stats()
        .runs(&rep.id, TrainingMode::Both)
        .unwrap()
        .completions;
    assert_eq!(completions, 1);
    assert_eq!(session.play_uci("d2d4", 4).unwrap(), TrainingVerdict::Done);
    assert_eq!(
        session
            .stats()
            .runs(&rep.id, TrainingMode::Both)
            .unwrap()
            .completions,
        1
    );

    session.restart(10).unwrap();
    assert_eq!(
        session.play_uci("e2e4", 10).unwrap(),
        TrainingVerdict::Right
    );
    assert_eq!(
        session.play_uci("c7c5", 11).unwrap(),
        TrainingVerdict::Finished
    );

    let mut disabled = rep.clone();
    disabled
        .set_prefix_disabled(&["e2e4".into(), "e7e5".into()], true)
        .unwrap();
    let mut session = TrainingSession::new(
        &disabled,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        20,
    )
    .unwrap();
    session.play_uci("e2e4", 20).unwrap();
    assert_eq!(
        session.play_uci("e7e5", 21).unwrap(),
        TrainingVerdict::Wrong
    );
    assert_eq!(
        session.play_uci("c7c5", 22).unwrap(),
        TrainingVerdict::Finished
    );

    let mut shallow = rep.clone();
    shallow.set_max_plies(1);
    let mut session = TrainingSession::new(
        &shallow,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        30,
    )
    .unwrap();
    assert_eq!(
        session.play_uci("e2e4", 30).unwrap(),
        TrainingVerdict::Finished
    );
}

#[test]
fn both_hint_after_three_errors_and_completion_are_recorded_once() {
    let rep = repertoire("1. e4 e5 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::SpacedRepetition,
        StatsData::default(),
        0,
    )
    .unwrap();
    for now in 1..=3 {
        assert_eq!(
            session.play_uci("d2d4", now).unwrap(),
            TrainingVerdict::Wrong
        );
        assert_eq!(session.current_node(), NodeId::ROOT);
    }
    assert!(session.may_hint());
    assert_eq!(session.hint(4).unwrap().uci, "e2e4");
    assert!(session.hinted());
    assert_eq!(session.play_uci("e2e4", 5).unwrap(), TrainingVerdict::Right);
    assert_eq!(
        session.play_uci("e7e5", 6).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(session.play_uci("e7e5", 7).unwrap(), TrainingVerdict::Done);
    let stats = &session.stats().move_stats;
    let e4 = stats
        .iter()
        .find(|entry| entry.key.position_fen4 == fen4(Game::new().start().position()))
        .unwrap();
    assert_eq!((e4.attempts, e4.correct, e4.wrong, e4.hints), (4, 1, 3, 1));
    assert_eq!(e4.streak, 0);
    assert_eq!(e4.recent.len(), 4);
    assert_eq!(e4.recent[0].actual_uci, "d2d4");
    let runs = session.stats().runs(&rep.id, TrainingMode::Both).unwrap();
    assert_eq!(runs.completions, 1);
    assert_eq!(runs.recent[0].mistakes, 3);
    assert!(runs.recent[0].hinted);
    assert_eq!(session.errors().count(), 3);
}

#[test]
fn both_continues_through_a_canonical_transposition_and_shares_stats() {
    let mut library = Library::default();
    let id = library.create_empty("Transpositions").unwrap();
    library
        .add_game(&id, &game("1. d4 d5 2. Nf3 Nf6 3. g3 *"))
        .unwrap();
    library
        .add_game(&id, &game("1. Nf3 d5 2. d4 Nf6 *"))
        .unwrap();
    let rep = library.repertoire(&id).unwrap().clone();
    let mut first = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    for (now, mv) in ["d2d4", "d7d5", "g1f3", "g8f6", "g2g3"]
        .into_iter()
        .enumerate()
    {
        first.play_uci(mv, now as i64).unwrap();
    }
    let mut second = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        first.into_stats(),
        10,
    )
    .unwrap();
    second.play_uci("g1f3", 10).unwrap();
    second.play_uci("d7d5", 11).unwrap();
    second.play_uci("d2d4", 12).unwrap();
    second.play_uci("g8f6", 13).unwrap();
    assert!(second.candidates().iter().any(|c| c.uci == "g2g3"));
    assert_eq!(
        second.play_uci("g2g3", 14).unwrap(),
        TrainingVerdict::Finished
    );
    let shared = second
        .stats()
        .move_stats
        .iter()
        .filter(|entry| {
            entry
                .recent
                .iter()
                .any(|attempt| attempt.actual_uci == "g2g3")
        })
        .collect::<Vec<_>>();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].attempts, 2);
    assert_eq!(shared[0].correct, 2);
}

struct SecondChooser;

impl MoveChooser for SecondChooser {
    fn choose(&mut self, candidates: &[Candidate]) -> Option<usize> {
        (candidates.len() > 1).then_some(1).or(Some(0))
    }
}

#[test]
fn one_side_alternative_and_injectable_chooser_do_not_count_auto_moves() {
    let rep = repertoire("1. e4 (1. d4 d5) e5 (1... c5) 2. Nf3 *");
    let mut session = TrainingSession::with_chooser(
        &rep,
        TrainingMode::White,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
        Box::new(SecondChooser),
    )
    .unwrap();
    assert_eq!(session.target().unwrap().expected_uci, "e2e4");
    assert_eq!(session.play_uci("e2e4", 2).unwrap(), TrainingVerdict::Right);
    assert_eq!(session.path()[0].uci, "e2e4");
    assert!(!session.path()[0].automatic);
    assert_eq!(session.path()[1].uci, "c7c5");
    assert!(session.path()[1].automatic);
    assert_eq!(session.attempts().len(), 1);
    assert_eq!(session.stats().move_stats.len(), 1);

    let black = repertoire("1. e4 e5 2. Nf3 *");
    let mut black_session = TrainingSession::new(
        &black,
        TrainingMode::Black,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        10,
    )
    .unwrap();
    assert_eq!(black_session.target().unwrap().expected_uci, "e7e5");
    assert_eq!(black_session.path()[0].uci, "e2e4");
    assert!(black_session.path()[0].automatic);
    assert_eq!(
        black_session.play_uci("e7e5", 11).unwrap(),
        TrainingVerdict::Finished
    );
    assert_eq!(black_session.attempts().len(), 1);
    assert!(
        black_session
            .path()
            .iter()
            .any(|mv| mv.uci == "g1f3" && mv.automatic)
    );
}

#[test]
fn srs_dates_unseen_review_order_and_least_recent_work() {
    let rep = repertoire("1. e4 *");
    let mut first = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    assert_eq!(
        first.play_uci("e2e4", 1_000).unwrap(),
        TrainingVerdict::Finished
    );
    let stats = &first.stats().move_stats[0];
    assert_eq!(stats.due, Some(1_000 + INITIAL_INTERVAL_SECONDS));
    assert_eq!(stats.streak, 1);

    let second_now = 1_000 + INITIAL_INTERVAL_SECONDS;
    let mut second = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::SpacedRepetition,
        first.into_stats(),
        second_now,
    )
    .unwrap();
    assert_eq!(
        second.play_uci("e2e4", second_now).unwrap(),
        TrainingVerdict::Finished
    );
    let stats = &second.stats().move_stats[0];
    assert_eq!(stats.due, Some(1_000 + 3 * INITIAL_INTERVAL_SECONDS));
    assert_eq!(stats.streak, 2);
    assert!(
        second
            .stats()
            .queue(
                &RepertoireTree::new(&rep),
                &rep.id,
                TrainingMode::White,
                QueueMode::SpacedRepetition,
                3_000,
            )
            .is_empty()
    );

    let wrong_rep = repertoire("1. e4 *");
    let mut wrong = TrainingSession::new_srs(
        &wrong_rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    wrong.play_uci("d2d4", 100).unwrap();
    let key = wrong
        .target()
        .map(|target| MoveKey {
            repertoire_id: wrong_rep.id.clone(),
            mode: TrainingMode::White,
            position_fen4: target.position_fen4.clone(),
            expected_uci: target.expected_uci.clone(),
        })
        .unwrap();
    assert_eq!(wrong.stats().get(&key).unwrap().due, Some(700));
    wrong.play_uci("e2e4", 101).unwrap();
    assert_eq!(wrong.stats().get(&key).unwrap().due, Some(701));
    assert_eq!(wrong.stats().get(&key).unwrap().streak, 0);

    let branches = repertoire("1. e4 (1. d4 d5) e5 2. Nf3 *");
    let mut training = TrainingSession::new(
        &branches,
        TrainingMode::White,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    training.play_uci("e2e4", 10).unwrap();
    let queue = training.stats().queue(
        &RepertoireTree::new(&branches),
        &branches.id,
        TrainingMode::White,
        QueueMode::LeastRecent,
        20,
    );
    assert_eq!(queue[0].expected_uci, "d2d4");
    assert_eq!(queue.last().unwrap().expected_uci, "e2e4");
    let review = training.stats().queue(
        &RepertoireTree::new(&branches),
        &branches.id,
        TrainingMode::White,
        QueueMode::ReviewInOrder,
        20,
    );
    assert_eq!(review[0].expected_uci, "d2d4");
}

#[test]
fn prefix_ignores_unparsed_tail_after_the_selected_node() {
    let source = game("1. e4 e4 2. d4 *");
    assert_eq!(source.mainline().len(), 1);
    assert!(!source.mainline().is_empty());
    let mut library = Library::default();
    let id = library.create("Target", &game("1. e4 c5 *")).unwrap();
    let before = library.repertoire(&id).unwrap().pgn.clone();

    let report = library
        .add_prefix(&id, &source, source.mainline()[0])
        .unwrap();

    assert_eq!(report.added_nodes, 0);
    assert_eq!(library.repertoire(&id).unwrap().pgn, before);
}

#[test]
fn review_queues_record_attempts_without_changing_srs_due() {
    let rep = repertoire("1. e4 *");
    let mut srs = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    srs.play_uci("e2e4", 100).unwrap();
    let original_due = srs.stats().move_stats[0].due;
    assert_eq!(original_due, Some(100 + INITIAL_INTERVAL_SECONDS));

    for queue_mode in [QueueMode::ReviewInOrder, QueueMode::LeastRecent] {
        let mut review = TrainingSession::new(
            &rep,
            TrainingMode::White,
            queue_mode,
            srs.stats().clone(),
            0,
        )
        .unwrap();
        review.play_uci("e2e4", 200).unwrap();
        let stats = &review.stats().move_stats[0];
        assert_eq!(stats.attempts, 2);
        assert_eq!(stats.last_seen, Some(200));
        assert_eq!(stats.due, original_due);

        review.restart(201).unwrap();
        review.play_uci("d2d4", 201).unwrap();
        assert_eq!(review.stats().move_stats[0].due, original_due);
    }
}

#[test]
fn srs_retry_is_kept_until_the_next_clean_success() {
    let rep = repertoire("1. e4 *");
    let mut session = TrainingSession::new_srs(
        &rep,
        TrainingMode::White,
        StatsData::default(),
        0,
        SrsPolicy::Introduce,
    )
    .unwrap();
    session.play_uci("d2d4", 100).unwrap();
    session.play_uci("e2e4", 101).unwrap();
    let stats = &session.stats().move_stats[0];
    assert_eq!(stats.due, Some(101 + MISTAKE_INTERVAL_SECONDS));
    assert_eq!(stats.streak, 0);

    session.restart_with_policy(701, SrsPolicy::Due).unwrap();
    session.play_uci("e2e4", 701).unwrap();
    let stats = &session.stats().move_stats[0];
    assert_eq!(stats.due, Some(701 + INITIAL_INTERVAL_SECONDS));
    assert_eq!(stats.streak, 1);

    session.restart(701 + INITIAL_INTERVAL_SECONDS).unwrap();
    session
        .play_uci("e2e4", 701 + INITIAL_INTERVAL_SECONDS)
        .unwrap();
    let stats = &session.stats().move_stats[0];
    assert_eq!(stats.due, Some(701 + 3 * INITIAL_INTERVAL_SECONDS));
    assert_eq!(stats.streak, 2);
}

#[test]
fn both_attempts_never_create_srs_due() {
    let rep = repertoire("1. e4 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    session.play_uci("d2d4", 100).unwrap();
    session.play_uci("e2e4", 101).unwrap();
    assert!(
        session
            .stats()
            .move_stats
            .iter()
            .all(|stats| stats.due.is_none())
    );
}

#[test]
fn stats_are_isolated_by_mode_and_repertoire() {
    let first_rep = repertoire("1. e4 *");
    let second_rep = repertoire("1. e4 *");
    let mut white = TrainingSession::new(
        &first_rep,
        TrainingMode::White,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    white.play_uci("e2e4", 1).unwrap();
    let mut both = TrainingSession::new(
        &first_rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        white.into_stats(),
        2,
    )
    .unwrap();
    both.play_uci("e2e4", 2).unwrap();
    let mut other = TrainingSession::new(
        &second_rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        both.into_stats(),
        3,
    )
    .unwrap();
    other.play_uci("e2e4", 3).unwrap();
    assert_eq!(other.stats().move_stats.len(), 3);
    assert!(
        other
            .stats()
            .move_stats
            .iter()
            .any(|entry| entry.key.mode == TrainingMode::White)
    );
    assert_eq!(
        other
            .stats()
            .move_stats
            .iter()
            .filter(|entry| entry.key.mode == TrainingMode::Both)
            .count(),
        2
    );
}

#[test]
fn library_store_roundtrip_errors_and_conflicts_are_visible() {
    let dir = temp_dir("library-store");
    let path = dir.join("repertoires.json");
    let store = LibraryStore::new(&path);
    let mut first = store.load().unwrap();
    let id = first.create("One", &game("1. e4 *")).unwrap();
    store.save(&mut first).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(store.load().unwrap().repertoire(&id).unwrap().name, "One");

    let mut stale = Library::default();
    stale.create("Stale", &game("1. d4 *")).unwrap();
    assert!(matches!(
        store.save(&mut stale),
        Err(StoreError::Conflict { .. })
    ));
    let disk = store.load().unwrap();
    assert_eq!(disk.repertoires.len(), 1);
    assert_eq!(disk.repertoires[0].name, "One");

    std::fs::write(&path, b"{broken").unwrap();
    assert!(store.load().is_err());
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"version": 99, "repertoires": []})).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        store.load(),
        Err(StoreError::UnsupportedVersion { .. })
    ));

    let blocker = dir.join("not-a-directory");
    std::fs::write(&blocker, b"file").unwrap();
    let blocked = LibraryStore::new(blocker.join("library.json"));
    let mut library = Library::default();
    assert!(blocked.save(&mut library).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn legacy_roundtrip_backup_migration_and_stats_conflict_preserve_data() {
    let dir = temp_dir("legacy-store");
    let legacy_path = dir.join("training.json");
    let new_path = dir.join("repertoire-training.json");
    let start = crate::line_key(&Chess::default(), &play(&Game::new(), &["e2e4", "c7c5"]));
    let other = crate::line_key(&Chess::default(), &play(&Game::new(), &["e2e4", "e7e5"]));
    let legacy_bytes = serde_json::to_vec_pretty(&json!({
        (start.clone()): {"runs": 4, "last": 2, "best": 0, "clean_streak": 0, "recent": [[2, false]], "last_date": "2026-10-01", "unknown": 7},
        (other): {"runs": 2, "last": 0, "best": 0, "clean_streak": 2, "recent": [], "last_date": "2026-10-02"},
        "future-key": {"arbitrary": [1, 2, 3]}
    }))
    .unwrap();
    std::fs::write(&legacy_path, &legacy_bytes).unwrap();
    let store = StatsStore::with_legacy(&new_path, &legacy_path);
    let mut stats = store.load().unwrap();
    assert_eq!(stats.legacy_lines.len(), 3);
    assert!(stats.move_stats.is_empty());
    assert_eq!(stats.legacy_lines[&start]["runs"], 4);
    assert_eq!(stats.legacy_lines[&start]["unknown"], 7);

    let mut library = Library::default();
    let report = library.migrate_legacy_lines(&stats.legacy_lines);
    assert_eq!(report.repertoires, 1);
    assert_eq!(report.lines, 2);
    assert_eq!(report.skipped, ["future-key"]);
    let after_first = library.repertoires[0].pgn.clone();
    let id = library.repertoires[0].id.clone();
    library.migrate_legacy_lines(&stats.legacy_lines);
    assert_eq!(library.repertoires.len(), 1);
    assert_eq!(library.repertoires[0].id, id);
    assert_eq!(library.repertoires[0].pgn, after_first);

    let mut stale = stats.clone();
    store.save(&mut stats).unwrap();
    assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy_bytes);
    let backup = dir.join("training.json.before-repertoire-v2");
    assert_eq!(std::fs::read(backup).unwrap(), legacy_bytes);
    assert!(matches!(
        store.save(&mut stale),
        Err(StoreError::Conflict { .. })
    ));
    let reopened = store.load().unwrap();
    assert_eq!(reopened.legacy_lines, stats.legacy_lines);
    assert!(reopened.move_stats.is_empty());

    std::fs::write(&legacy_path, b"{bad").unwrap();
    let missing_new = StatsStore::with_legacy(dir.join("other.json"), &legacy_path);
    assert!(missing_new.load().is_err());
    assert_eq!(std::fs::read(&legacy_path).unwrap(), b"{bad");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stats_store_write_failure_does_not_replace_legacy_or_report_success() {
    let dir = temp_dir("write-error");
    let legacy = dir.join("training.json");
    std::fs::write(&legacy, b"{}").unwrap();
    let blocker = dir.join("blocked");
    std::fs::write(&blocker, b"file").unwrap();
    let store = StatsStore::with_legacy(blocker.join("stats.json"), &legacy);
    let mut stats = StatsData::default();
    assert!(store.save(&mut stats).is_err());
    assert_eq!(std::fs::read(&legacy).unwrap(), b"{}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_stats_can_be_saved_after_every_run() {
    let dir = temp_dir("session-save");
    let store = StatsStore::new(dir.join("stats.json"));
    let rep = repertoire("1. e4 *");
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::White,
        QueueMode::ReviewInOrder,
        store.load().unwrap(),
        0,
    )
    .unwrap();
    session.play_uci("e2e4", 10).unwrap();
    store.save(session.stats_mut()).unwrap();
    assert_eq!(session.stats().revision, 1);
    session.restart(20).unwrap();
    session.play_uci("e2e4", 20).unwrap();
    store.save(session.stats_mut()).unwrap();
    assert_eq!(session.stats().revision, 2);
    assert_eq!(
        store
            .load()
            .unwrap()
            .runs(&rep.id, TrainingMode::White)
            .unwrap()
            .completions,
        2
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn adding_training_and_deleting_never_touches_the_source_file() {
    let dir = temp_dir("source-file");
    let source_path = dir.join("legacy.pgn");
    let bytes = b"[Event \"Legacy\"]\n\n1. e4 c5 2. Nf3 d6 *\n\xff";
    std::fs::write(&source_path, bytes).unwrap();
    let source = game("1. e4 c5 2. Nf3 d6 3. Bb5+ *");
    let mut library = Library::default();
    let id = library.create_empty("Snapshot").unwrap();
    let at = source.mainline()[3];
    library.add_prefix(&id, &source, at).unwrap();
    let rep = library.repertoire(&id).unwrap().clone();
    let mut session = TrainingSession::new(
        &rep,
        TrainingMode::Both,
        QueueMode::ReviewInOrder,
        StatsData::default(),
        0,
    )
    .unwrap();
    for (now, mv) in ["e2e4", "c7c5", "g1f3", "d7d6"].into_iter().enumerate() {
        session.play_uci(mv, now as i64).unwrap();
    }
    library.delete(&id).unwrap();
    assert_eq!(std::fs::read(&source_path).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(dir);
}
