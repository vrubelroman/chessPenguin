use std::collections::BTreeSet;

use chess_core::shakmaty::{CastlingMode, Position};
use chess_core::{Game, NodeId};

use super::*;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 24 * 60 * 60;

fn add_repertoire(library: &mut Library, name: &str, pgn: &str) -> RepertoireId {
    library
        .create(name, &chess_core::parse_str(pgn).game)
        .unwrap()
}

fn single_repertoire(pgn: &str) -> (Library, RepertoireId) {
    let mut library = Library::default();
    let id = add_repertoire(&mut library, "One", pgn);
    (library, id)
}

fn scope(id: RepertoireId, side: LearningSide) -> StatsScope {
    StatsScope::for_repertoire(id, side, NOW)
}

fn strings(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|mv| (*mv).to_owned()).collect()
}

fn key_at_path(repertoire: &Repertoire, path: &[String], expected_uci: &str) -> LearningItemKey {
    let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
    let parent = tree.node_at_path(path).unwrap();
    LearningItemKey::local(tree.position_key(parent).unwrap(), expected_uci)
}

fn learnt(item: &mut LearningItem, next_review_at: i64) {
    item.introduced = true;
    item.correct_streak = 3;
    item.correct_count = 3;
    item.last_review_at = Some(NOW - DAY);
    item.last_correct_at = Some(NOW - DAY);
    item.next_review_at = Some(next_review_at);
    item.previous_interval_seconds = Some(DAY);
}

fn pgn_with_white_moves(count: usize) -> String {
    let mut game = Game::new();
    let mut position = game.start().position().clone();
    let mut node = NodeId::ROOT;
    let mut visited = BTreeSet::from([fen4(&position)]);
    for _ in 0..count * 2 - 1 {
        let mut moves = position.legal_moves();
        moves.sort_by_key(|mv| mv.to_uci(CastlingMode::Standard).to_string());
        let mv = moves
            .into_iter()
            .find(|mv| {
                let mut next = position.clone();
                next.play_unchecked(*mv);
                !visited.contains(&fen4(&next))
            })
            .expect("fixture must have a non-repeating legal continuation");
        node = game.add_line(node, &[mv]).unwrap();
        position.play_unchecked(mv);
        visited.insert(fen4(&position));
    }
    game.to_pgn()
}

fn transposition_pgn() -> &'static str {
    "1. e4 d5 (1... Nf6 2. d4 d5 3. c4) 2. d4 Nh6 3. Nf3 Ng8 4. Ng1 Nf6 5. c4 e6 6. g3 *"
}

#[test]
fn at_01_one_hundred_real_pgn_moves_are_unique_unseen_and_not_due() {
    let (library, id) = single_repertoire(&pgn_with_white_moves(100));
    let learning = LearningData::default();
    let scoped = aggregate_scope(
        &library,
        &learning,
        &scope(id, LearningSide::White),
        &SrsSettings::default(),
    )
    .unwrap();

    assert_eq!(scoped.summary.total, 100);
    assert_eq!(scoped.summary.unseen, 100);
    assert_eq!(scoped.summary.learning, 0);
    assert_eq!(scoped.summary.learnt, 0);
    assert_eq!(scoped.summary.due, 0);
    assert_eq!(scoped.items.len(), 100);
    assert_eq!(scoped.scope.as_of, NOW);
    assert!(learning.items.is_empty());
}

#[test]
fn at_07_zero_due_with_unseen_is_not_confirmed_current() {
    let (library, id) = single_repertoire("1. e4 e5 2. Nf3 Nc6 *");
    let scope = scope(id, LearningSide::White);
    let mut learning = LearningData::default();
    let first = active_learning_keys(&library, &scope)
        .unwrap()
        .into_iter()
        .find(|key| key.expected_uci == "e2e4")
        .unwrap();
    let item = learning.ensure_item(first);
    item.introduced = true;
    item.correct_streak = 1;
    item.next_review_at = Some(NOW + DAY);

    let scoped = aggregate_scope(&library, &learning, &scope, &SrsSettings::default()).unwrap();
    assert_eq!(scoped.summary.due, 0);
    assert_eq!(scoped.summary.unseen, 1);
    assert_eq!(scoped.summary.learning, 1);
    assert_ne!(
        scoped.summary.repertoire_status,
        RepertoireStatus::ConfirmedCurrent
    );
    assert_eq!(
        scoped.summary.repertoire_status,
        RepertoireStatus::InProgress
    );
}

#[test]
fn at_14_two_repertoires_use_one_global_learning_item() {
    let mut library = Library::default();
    let first = add_repertoire(&mut library, "Open", "1. e4 e5 2. Nf3 Nc6 *");
    let second = add_repertoire(&mut library, "Petrov", "1. e4 e5 2. Nf3 Nf6 *");
    let root = fen4(Game::new().start().position());
    let shared_key = LearningItemKey::local(root, "e2e4");
    let mut learning = LearningData::default();
    let shared_item = learning.ensure_item(shared_key.clone());
    shared_item.introduced = true;
    shared_item.correct_streak = 2;

    let first_scope = scope(first.clone(), LearningSide::White);
    let second_scope = scope(second.clone(), LearningSide::White);
    let both_scope = StatsScope::new([first, second], LearningSide::White, NOW);
    for selected in [&first_scope, &second_scope, &both_scope] {
        let scoped =
            aggregate_scope(&library, &learning, selected, &SrsSettings::default()).unwrap();
        assert_eq!(scoped.summary.total, 2);
        assert_eq!(scoped.summary.learning, 1);
        assert_eq!(scoped.summary.unseen, 1);
        assert_eq!(
            scoped
                .items
                .iter()
                .filter(|item| item.key == shared_key)
                .count(),
            1
        );
    }
    assert_eq!(learning.items.len(), 1);
    assert_eq!(learning.items[0].correct_streak, 2);
}

#[test]
fn at_15_global_aggregate_is_union_not_sum_of_repertoire_rows() {
    let mut library = Library::default();
    let first = add_repertoire(&mut library, "Two knights", "1. e4 e5 2. Nf3 Nc6 *");
    let second = add_repertoire(&mut library, "Petrov", "1. e4 e5 2. Nf3 Nf6 *");
    let learning = LearningData::default();
    let first_total = aggregate_scope(
        &library,
        &learning,
        &scope(first.clone(), LearningSide::Black),
        &SrsSettings::default(),
    )
    .unwrap()
    .summary
    .total;
    let second_total = aggregate_scope(
        &library,
        &learning,
        &scope(second.clone(), LearningSide::Black),
        &SrsSettings::default(),
    )
    .unwrap()
    .summary
    .total;
    let global = aggregate_scope(
        &library,
        &learning,
        &StatsScope::new([first, second], LearningSide::Black, NOW),
        &SrsSettings::default(),
    )
    .unwrap();

    assert_eq!((first_total, second_total), (2, 2));
    assert_eq!(global.summary.total, 3);
    assert!(global.summary.total < first_total + second_total);
}

#[test]
fn at_16_no_active_own_moves_is_empty_without_percentages() {
    let (library, id) = single_repertoire("1. e4 *");
    let scoped = aggregate_scope(
        &library,
        &LearningData::default(),
        &scope(id, LearningSide::Black),
        &SrsSettings::default(),
    )
    .unwrap();

    assert_eq!(scoped.summary.total, 0);
    assert_eq!(scoped.summary.repertoire_status, RepertoireStatus::Empty);
    for percentage in [
        scoped.summary.coverage_percent,
        scoped.summary.learnt_percent,
        scoped.summary.confident_percent,
        scoped.summary.learning_percent,
        scoped.summary.unseen_percent,
    ] {
        assert_eq!(percentage, None);
    }
}

#[test]
fn at_17_depth_limits_summary_but_not_full_scope() {
    let (library, id) =
        single_repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 *");
    let full_scope = scope(id.clone(), LearningSide::White);
    let depth_scope = full_scope.clone().with_max_depth(Some(10));
    let learning = LearningData::default();
    let depth =
        aggregate_scope(&library, &learning, &depth_scope, &SrsSettings::default()).unwrap();
    let full = aggregate_scope(&library, &learning, &full_scope, &SrsSettings::default()).unwrap();

    assert_eq!(depth.summary.total, 5);
    assert_eq!(full.summary.total, 6);
    assert!(
        full.items
            .iter()
            .any(|item| item.key.expected_uci == "f1e1")
    );
    assert!(
        !depth
            .items
            .iter()
            .any(|item| item.key.expected_uci == "f1e1")
    );
}

#[test]
fn at_18_19_disable_and_reenable_preserve_global_history() {
    let (mut library, id) = single_repertoire("1. e4 e5 (1... c5) 2. Nf3 *");
    let mut learning = LearningData::default();
    let nf3 = key_at_path(
        library.repertoire(&id).unwrap(),
        &strings(&["e2e4", "e7e5"]),
        "g1f3",
    );
    learnt(learning.ensure_item(nf3.clone()), NOW + DAY);
    let before = learning.item(&nf3).unwrap().clone();
    let selected = scope(id.clone(), LearningSide::White);

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "e7e5"]), true)
        .unwrap();
    let disabled =
        aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(disabled.summary.total, 1);
    assert!(!disabled.items.iter().any(|item| item.key == nf3));
    assert_eq!(learning.item(&nf3), Some(&before));

    let mut include_inactive = selected.clone();
    include_inactive.only_active = false;
    assert_eq!(
        aggregate_scope(
            &library,
            &learning,
            &include_inactive,
            &SrsSettings::default(),
        )
        .unwrap()
        .summary
        .learnt,
        1
    );

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "e7e5"]), false)
        .unwrap();
    let enabled = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(enabled.summary.total, 2);
    assert_eq!(enabled.summary.learnt, 1);
    assert_eq!(learning.item(&nf3), Some(&before));
}

#[test]
fn disabled_transposition_keeps_shared_key_through_another_active_branch() {
    let (mut library, id) = single_repertoire(transposition_pgn());
    let selected = scope(id.clone(), LearningSide::White);
    let shared = key_at_path(
        library.repertoire(&id).unwrap(),
        &strings(&["e2e4", "g8f6", "d2d4", "d7d5"]),
        "c2c4",
    );
    let mut learning = LearningData::default();
    learnt(learning.ensure_item(shared.clone()), NOW + DAY);
    let before = learning.item(&shared).unwrap().clone();

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "d7d5"]), true)
        .unwrap();
    let disabled_keys = active_learning_keys(&library, &selected).unwrap();
    assert!(disabled_keys.contains(&shared));
    assert_eq!(disabled_keys.len(), 3);

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "d7d5"]), false)
        .unwrap();
    let enabled_keys = active_learning_keys(&library, &selected).unwrap();
    assert!(enabled_keys.contains(&shared));
    assert_eq!(enabled_keys.len(), 7);
    assert_eq!(learning.item(&shared), Some(&before));
}

#[test]
fn at_24_transposition_paths_share_one_key_and_progress() {
    let (library, id) = single_repertoire(transposition_pgn());
    let repertoire = library.repertoire(&id).unwrap();
    let first_scope =
        scope(id.clone(), LearningSide::White).with_branch_root(strings(&["e2e4", "d7d5"]));
    let second_scope = scope(id, LearningSide::White).with_branch_root(strings(&["e2e4", "g8f6"]));
    let first_keys = active_learning_keys(&library, &first_scope).unwrap();
    let second_keys = active_learning_keys(&library, &second_scope).unwrap();
    let shared: Vec<_> = first_keys.intersection(&second_keys).cloned().collect();
    let expected = key_at_path(
        repertoire,
        &strings(&["e2e4", "g8f6", "d2d4", "d7d5"]),
        "c2c4",
    );
    assert_eq!(shared.as_slice(), std::slice::from_ref(&expected));

    let mut learning = LearningData::default();
    learnt(learning.ensure_item(expected), NOW + DAY);
    for selected in [first_scope, second_scope] {
        let scoped =
            aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
        assert_eq!(scoped.summary.learnt, 1);
    }
    assert_eq!(learning.items.len(), 1);
}

#[test]
fn at_25_adding_twenty_moves_recalculates_scope_and_preserves_successes() {
    let (mut library, id) = single_repertoire(&pgn_with_white_moves(80));
    let selected = scope(id.clone(), LearningSide::White);
    let old_keys = active_learning_keys(&library, &selected).unwrap();
    assert_eq!(old_keys.len(), 80);
    let mut learning = LearningData::default();
    for key in old_keys {
        learnt(learning.ensure_item(key), NOW + DAY);
    }
    let before = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(before.summary.learnt_percent, Some(100.0));

    library.repertoire_mut(&id).unwrap().pgn = pgn_with_white_moves(100);
    let after = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(after.summary.total, 100);
    assert_eq!(after.summary.learnt, 80);
    assert_eq!(after.summary.unseen, 20);
    assert_eq!(after.summary.learnt_percent, Some(80.0));
    assert!(learning.items.iter().all(|item| item.correct_streak == 3));
}

#[test]
fn at_28_one_hundred_learnt_with_three_due_is_all_learnt_not_confirmed() {
    let (library, id) = single_repertoire(&pgn_with_white_moves(100));
    let selected = scope(id, LearningSide::White);
    let mut learning = LearningData::default();
    for (index, key) in active_learning_keys(&library, &selected)
        .unwrap()
        .into_iter()
        .enumerate()
    {
        learnt(
            learning.ensure_item(key),
            if index < 3 { NOW } else { NOW + DAY },
        );
    }
    let scoped = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(scoped.summary.total, 100);
    assert_eq!(scoped.summary.learnt, 100);
    assert_eq!(scoped.summary.due, 3);
    assert_eq!(
        scoped.summary.repertoire_status,
        RepertoireStatus::AllLearnt
    );
}

#[test]
fn at_29_one_hundred_fresh_learnt_without_due_is_confirmed_current() {
    let (library, id) = single_repertoire(&pgn_with_white_moves(100));
    let selected = scope(id, LearningSide::White);
    let mut learning = LearningData::default();
    for key in active_learning_keys(&library, &selected).unwrap() {
        learnt(learning.ensure_item(key), NOW + DAY);
    }
    let scoped = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(scoped.summary.total, 100);
    assert_eq!(scoped.summary.learnt, 100);
    assert_eq!(scoped.summary.due, 0);
    assert_eq!(scoped.summary.uncertain, 0);
    assert_eq!(
        scoped.summary.repertoire_status,
        RepertoireStatus::ConfirmedCurrent
    );
}

#[test]
fn at_30_lines_overlap_but_repertoire_is_not_an_arithmetic_mean() {
    let (library, id) = single_repertoire(transposition_pgn());
    let repertoire = library.repertoire(&id).unwrap();
    let first_scope =
        scope(id.clone(), LearningSide::White).with_branch_root(strings(&["e2e4", "d7d5"]));
    let second_scope =
        scope(id.clone(), LearningSide::White).with_branch_root(strings(&["e2e4", "g8f6"]));
    let shared = key_at_path(
        repertoire,
        &strings(&["e2e4", "g8f6", "d2d4", "d7d5"]),
        "c2c4",
    );
    let mut learning = LearningData::default();
    learnt(learning.ensure_item(shared), NOW + DAY);
    let first =
        aggregate_scope(&library, &learning, &first_scope, &SrsSettings::default()).unwrap();
    let second =
        aggregate_scope(&library, &learning, &second_scope, &SrsSettings::default()).unwrap();
    let full = aggregate_scope(
        &library,
        &learning,
        &scope(id, LearningSide::White),
        &SrsSettings::default(),
    )
    .unwrap();

    assert_eq!((first.summary.total, second.summary.total), (5, 2));
    assert_eq!(first.summary.learnt, 1);
    assert_eq!(second.summary.learnt, 1);
    assert_eq!(full.summary.total, 7);
    assert_eq!(full.summary.learnt, 1);
    let mean =
        (first.summary.learnt_percent.unwrap() + second.summary.learnt_percent.unwrap()) / 2.0;
    assert!((full.summary.learnt_percent.unwrap() - mean).abs() > f64::EPSILON);
    let first_keys = active_learning_keys(&library, &first_scope).unwrap();
    let second_keys = active_learning_keys(&library, &second_scope).unwrap();
    assert_eq!(first_keys.union(&second_keys).count(), 6);
    assert!(first_keys.union(&second_keys).count() < first_keys.len() + second_keys.len());
}

#[test]
fn all_answers_keep_real_pgn_context_and_shortest_graph_depth() {
    let (library, id) = single_repertoire(
        "1. e4 d5 (1... Nf6 2. d4 d5 3. g3) 2. d4 Nh6 3. Nf3 Ng8 4. Ng1 Nf6 5. c4 *",
    );
    let full_scope = scope(id.clone(), LearningSide::White);
    let full = active_learning_memberships(&library, &full_scope).unwrap();
    let c4 = full
        .iter()
        .find(|membership| membership.key.expected_uci == "c2c4")
        .unwrap();
    assert_eq!(c4.min_depth, 5);
    assert_eq!(c4.path.len(), 9);
    let tree = RepertoireTree::from_game(
        library.repertoire(&id).unwrap().game(),
        u32::MAX,
        Vec::new(),
    );
    assert!(
        tree.node_at_path(&c4.path).is_some(),
        "context must be an actual annotated PGN path"
    );
    assert_eq!(c4.path.last().map(String::as_str), Some("c2c4"));
    assert!(
        full.iter()
            .any(|membership| membership.key.expected_uci == "g2g3")
    );

    let depth =
        active_learning_memberships(&library, &full_scope.clone().with_max_depth(Some(5))).unwrap();
    assert!(depth.iter().any(|membership| membership.key == c4.key));
    assert!(
        !active_learning_keys(&library, &full_scope.with_max_depth(Some(4)))
            .unwrap()
            .contains(&c4.key)
    );

    let short_branch = scope(id, LearningSide::White).with_branch_root(strings(&["e2e4", "g8f6"]));
    let short_keys = active_learning_keys(&library, &short_branch).unwrap();
    assert_eq!(short_keys.len(), 2);
    assert!(short_keys.iter().any(|key| key.expected_uci == "g2g3"));
    assert!(!short_keys.iter().any(|key| key.expected_uci == "c2c4"));
}

#[test]
fn main_answer_override_validates_side_and_pgn_move_and_roundtrips() {
    let (library, id) = single_repertoire("1. e4 (1. d4) e5 *");
    let start = fen4(Game::new().start().position());
    let repertoire = library.repertoire(&id).unwrap();
    let original_pgn = repertoire.pgn.clone();
    let e4 = LearningItemKey::local(start.clone(), "e2e4");
    let d4 = LearningItemKey::local(start.clone(), "d2d4");
    let selected = scope(id.clone(), LearningSide::White);
    let default_keys = active_learning_keys(&library, &selected).unwrap();
    assert!(default_keys.contains(&e4));
    assert!(default_keys.contains(&d4));

    let mut changed = repertoire.clone();
    changed
        .set_main_answer_override(&start, "d2d4", LearningSide::White)
        .unwrap();
    assert_eq!(changed.main_answer_override(&start), Some("d2d4"));
    assert!(matches!(
        changed.set_main_answer_override(&start, "e7e5", LearningSide::Black),
        Err(RepertoireError::MainAnswerWrongSide { .. })
    ));
    assert!(matches!(
        changed.set_main_answer_override(&start, "c2c4", LearningSide::White),
        Err(RepertoireError::MainAnswerMoveNotFound { .. })
    ));
    assert!(matches!(
        changed.set_main_answer_override("missing", "d2d4", LearningSide::White),
        Err(RepertoireError::MainAnswerPositionNotFound(_))
    ));
    assert_eq!(changed.main_answer_override(&start), Some("d2d4"));
    assert_eq!(changed.pgn, original_pgn);

    let encoded = serde_json::to_string(&changed).unwrap();
    let restored: Repertoire = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.main_answer_override(&start), Some("d2d4"));
    let mut old_json = serde_json::to_value(&changed).unwrap();
    old_json
        .as_object_mut()
        .unwrap()
        .remove("main_answer_overrides");
    let old: Repertoire = serde_json::from_value(old_json).unwrap();
    assert!(old.main_answer_overrides.is_empty());

    let mut overridden_library = Library::default();
    overridden_library.repertoires.push(changed.clone());
    let overridden = active_learning_keys(&overridden_library, &selected).unwrap();
    assert!(overridden.contains(&d4));
    assert!(overridden.contains(&e4));
    assert_eq!(overridden, default_keys);
    let ordered = active_learning_memberships(&overridden_library, &selected).unwrap();
    let preferred = ordered.iter().find(|item| item.key == d4).unwrap();
    let other = ordered.iter().find(|item| item.key == e4).unwrap();
    assert!(preferred.pgn_order < other.pgn_order);
    assert_eq!(
        changed.clear_main_answer_override(&start).as_deref(),
        Some("d2d4")
    );
    assert_eq!(changed.clear_main_answer_override(&start), None);

    overridden_library.repertoires[0].clear_main_answer_override(&start);
    let second = add_repertoire(&mut overridden_library, "Queen pawn", "1. e4 (1. d4) e5 *");
    overridden_library
        .repertoire_mut(&second)
        .unwrap()
        .set_main_answer_override(&start, "d2d4", LearningSide::White)
        .unwrap();
    let global = active_learning_keys(
        &overridden_library,
        &StatsScope::new([id, second], LearningSide::White, NOW),
    )
    .unwrap();
    assert!(global.contains(&e4));
    assert!(global.contains(&d4));
    assert_eq!(global.len(), 2);
}

#[test]
fn both_training_mode_is_rejected_for_learning_statistics() {
    assert_eq!(
        LearningSide::try_from(TrainingMode::White),
        Ok(LearningSide::White)
    );
    assert_eq!(
        LearningSide::try_from(TrainingMode::Black),
        Ok(LearningSide::Black)
    );
    assert_eq!(
        LearningSide::try_from(TrainingMode::Both),
        Err(LearningGraphError::BothSidesNotSupported)
    );
}

#[test]
fn repeated_fen4_cycle_terminates_and_deduplicates_items() {
    let (library, id) = single_repertoire("1. Nf3 Nf6 2. Ng1 Ng8 3. Nf3 Nf6 4. Ng1 Ng8 *");
    let memberships =
        active_learning_memberships(&library, &scope(id, LearningSide::White)).unwrap();
    assert_eq!(memberships.len(), 2);
    let mut depths: Vec<_> = memberships
        .iter()
        .map(|membership| membership.min_depth)
        .collect();
    depths.sort_unstable();
    assert_eq!(depths, [1, 3]);
}

#[test]
fn aggregate_scope_uses_one_explicit_as_of() {
    let (library, id) = single_repertoire("1. e4 *");
    let mut selected = scope(id, LearningSide::White);
    let key = active_learning_keys(&library, &selected)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let mut learning = LearningData::default();
    let item = learning.ensure_item(key);
    item.introduced = true;
    item.correct_streak = 1;
    item.next_review_at = Some(NOW);

    selected.as_of = NOW - 1;
    let before = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(before.summary.due, 0);
    assert_eq!(before.summary.next_due_at, Some(NOW));
    assert_eq!(before.scope.as_of, NOW - 1);

    selected.as_of = NOW;
    let due = aggregate_scope(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(due.summary.due, 1);
    assert_eq!(due.summary.next_due_at, None);
    assert_eq!(due.scope.as_of, NOW);
}

#[test]
fn reset_impact_projects_n_m_and_names_without_resetting() {
    let mut library = Library::default();
    let first = add_repertoire(&mut library, "First", "1. e4 e5 2. Nf3 *");
    let second = add_repertoire(&mut library, "Second", "1. e4 c5 2. c4 *");
    let selected = scope(first, LearningSide::White);
    let mut learning = LearningData::default();
    for key in active_learning_keys(&library, &selected).unwrap() {
        learnt(learning.ensure_item(key), NOW + DAY);
    }
    let before = learning.clone();

    let impact = reset_impact(&library, &selected).unwrap();
    assert_eq!(impact.reset_count, 2);
    assert_eq!(impact.shared_count, 1);
    assert_eq!(impact.affected_repertoires.len(), 2);
    let first_impact = impact
        .affected_repertoires
        .iter()
        .find(|repertoire| repertoire.name == "First")
        .unwrap();
    let second_impact = impact
        .affected_repertoires
        .iter()
        .find(|repertoire| repertoire.name == "Second")
        .unwrap();
    assert_eq!(first_impact.item_count, 2);
    assert_eq!(second_impact.item_count, 1);
    assert!(
        impact
            .affected_repertoires
            .iter()
            .any(|repertoire| repertoire.id == second)
    );
    assert_eq!(learning, before);
}

#[test]
fn disabling_one_answer_keeps_other_active_variations() {
    let (mut library, id) = single_repertoire("1. e4 (1. d4) e5 *");
    let selected = scope(id.clone(), LearningSide::White);
    let root = fen4(Game::new().start().position());
    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4"]), true)
        .unwrap();
    assert_eq!(
        active_learning_keys(&library, &selected).unwrap(),
        BTreeSet::from([LearningItemKey::local(root.clone(), "d2d4")])
    );

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_main_answer_override(&root, "d2d4", LearningSide::White)
        .unwrap();
    assert_eq!(
        active_learning_keys(&library, &selected).unwrap(),
        BTreeSet::from([LearningItemKey::local(root, "d2d4")])
    );
}
