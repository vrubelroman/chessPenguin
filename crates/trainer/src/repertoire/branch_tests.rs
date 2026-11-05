use std::collections::BTreeSet;

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

fn scope(id: RepertoireId) -> StatsScope {
    StatsScope::for_repertoire(id, LearningSide::White, NOW)
}

fn strings(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|mv| (*mv).to_owned()).collect()
}

fn key_at_path(repertoire: &Repertoire, path: &[String], expected_uci: &str) -> LearningItemKey {
    let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
    let parent = tree.node_at_path(path).unwrap();
    LearningItemKey::local(tree.position_key(parent).unwrap(), expected_uci)
}

fn branch<'a>(
    rows: &'a [BranchLearningAnalytics],
    repertoire_id: &RepertoireId,
    path: &[&str],
) -> Option<&'a BranchLearningAnalytics> {
    rows.iter().find(|row| {
        &row.repertoire_id == repertoire_id
            && row
                .branch_root
                .iter()
                .map(String::as_str)
                .eq(path.iter().copied())
    })
}

fn learnt(item: &mut LearningItem, next_review_at: i64) {
    item.introduced = true;
    item.correct_streak = 3;
    item.correct_count = 3;
    item.last_review_at = Some(NOW - DAY);
    item.last_correct_at = Some(NOW - DAY);
    item.last_independent_at = Some(NOW - DAY);
    item.next_review_at = Some(next_review_at);
    item.previous_interval_seconds = Some(DAY);
}

fn transposition_pgn() -> &'static str {
    "1. e4 d5 (1... Nf6 2. d4 d5 3. c4) 2. d4 Nh6 3. Nf3 Ng8 4. Ng1 Nf6 5. c4 e6 6. g3 *"
}

fn branching_pgn() -> &'static str {
    "1. e4 e5 (1... c5 2. Nf3) 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O *"
}

fn attempt(
    key: &LearningItemKey,
    id: &str,
    event_type: TrainingEventType,
    happened_at: i64,
) -> TrainingAttempt {
    TrainingAttempt {
        user_id: key.user_id.clone(),
        position_fen4: key.position_fen4.clone(),
        client_attempt_id: id.into(),
        session_id: "branch-analytics".into(),
        event_type,
        expected_uci: key.expected_uci.clone(),
        actual_uci: Some(if event_type == TrainingEventType::IncorrectIndependent {
            "d2d4".into()
        } else {
            key.expected_uci.clone()
        }),
        response_time_ms: Some(250),
        happened_at,
    }
}

#[test]
fn at_30_transposition_is_unique_per_branch_and_in_global_scope() {
    let mut library = Library::default();
    let first = add_repertoire(&mut library, "First", transposition_pgn());
    let second = add_repertoire(&mut library, "Second", transposition_pgn());
    let shared = key_at_path(
        library.repertoire(&first).unwrap(),
        &strings(&["e2e4", "g8f6", "d2d4", "d7d5"]),
        "c2c4",
    );
    let mut learning = LearningData::default();
    learnt(learning.ensure_item(shared), NOW + DAY);

    let selected = scope(first.clone());
    let rows =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    let first_move = branch(&rows, &first, &["e2e4"]).unwrap();
    let main_branch = branch(&rows, &first, &["e2e4", "d7d5"]).unwrap();
    let transposing_branch = branch(&rows, &first, &["e2e4", "g8f6"]).unwrap();
    assert_eq!(first_move.summary.total, 7);
    assert_eq!(first_move.summary.learnt, 1);
    assert_eq!(first_move.summary.accuracy_correct, 3);
    assert_eq!(first_move.summary.accuracy_attempts, 3);
    assert_eq!(main_branch.summary.total, 5);
    assert_eq!(main_branch.summary.learnt, 1);
    assert_eq!(main_branch.summary.accuracy_correct, 3);
    assert_eq!(transposing_branch.summary.total, 2);
    assert_eq!(transposing_branch.summary.learnt, 1);
    assert_eq!(transposing_branch.summary.accuracy_correct, 3);
    assert_eq!(transposing_branch.summary.accuracy_attempts, 3);
    assert_eq!(
        first_move.summary.total + 1,
        1 + main_branch.summary.total + transposing_branch.summary.total
    );
    assert!(rows.iter().all(|row| !row.branch_root.is_empty()));

    let global_scope = StatsScope::new([first.clone(), second.clone()], LearningSide::White, NOW);
    let global_rows =
        branch_learning_analytics(&library, &learning, &global_scope, &SrsSettings::default())
            .unwrap();
    let global_first_moves: Vec<_> = global_rows
        .iter()
        .filter(|row| row.branch_root == strings(&["e2e4"]))
        .collect();
    assert_eq!(global_first_moves.len(), 2);
    assert_eq!(
        global_first_moves
            .iter()
            .map(|row| row.repertoire_id.clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([first, second])
    );
    assert!(
        global_first_moves
            .iter()
            .all(|row| row.summary.total == 7 && row.summary.accuracy_correct == 3)
    );
    let global =
        aggregate_scope(&library, &learning, &global_scope, &SrsSettings::default()).unwrap();
    assert_eq!(global.summary.total, 7);
    assert_eq!(global.summary.learnt, 1);
    assert_eq!(global.summary.accuracy_correct, 3);
}

#[test]
fn branch_summary_uses_core_statuses_due_and_independent_accuracy() {
    let (library, id) = single_repertoire(branching_pgn());
    let repertoire = library.repertoire(&id).unwrap();
    let nf3 = key_at_path(repertoire, &strings(&["e2e4", "e7e5"]), "g1f3");
    let bb5 = key_at_path(
        repertoire,
        &strings(&["e2e4", "e7e5", "g1f3", "b8c6"]),
        "f1b5",
    );
    let ba4 = key_at_path(
        repertoire,
        &strings(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6"]),
        "b5a4",
    );
    let castle = key_at_path(
        repertoire,
        &strings(&[
            "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6",
        ]),
        "e1g1",
    );
    let mut learning = LearningData::default();

    let item = learning.ensure_item(nf3);
    item.introduced = true;
    item.correct_streak = 1;
    item.correct_count = 1;
    item.next_review_at = Some(NOW + DAY);
    item.last_independent_at = Some(NOW - 40);

    let item = learning.ensure_item(bb5);
    learnt(item, NOW + DAY);
    item.last_independent_at = Some(NOW - 20);

    let item = learning.ensure_item(ba4);
    item.introduced = true;
    item.correct_streak = 3;
    item.correct_count = 2;
    item.wrong_count = 1;
    item.next_review_at = Some(NOW - 8 * DAY);
    item.previous_interval_seconds = Some(DAY);
    item.last_independent_at = Some(NOW - 10);

    let item = learning.ensure_item(castle);
    item.introduced = true;
    item.wrong_count = 2;
    item.next_review_at = Some(NOW - 1);
    item.last_independent_at = Some(NOW - 30);

    let rows = branch_learning_analytics(
        &library,
        &learning,
        &scope(id.clone()),
        &SrsSettings::default(),
    )
    .unwrap();
    let summary = &branch(&rows, &id, &["e2e4"]).unwrap().summary;
    assert_eq!(summary.total, 6);
    assert_eq!(summary.unseen, 2);
    assert_eq!(summary.learning, 2);
    assert_eq!(summary.learnt, 2);
    assert_eq!(summary.uncertain, 1);
    assert_eq!(summary.learnt_confident, 1);
    assert_eq!(summary.due, 2);
    assert_eq!(summary.accuracy_correct, 6);
    assert_eq!(summary.accuracy_wrong, 3);
    assert_eq!(summary.accuracy_attempts, 9);
    assert_eq!(summary.accuracy_percent, Some(100.0 * 6.0 / 9.0));
    assert_eq!(
        branch(&rows, &id, &["e2e4"]).unwrap().last_independent_at,
        Some(NOW - 10)
    );

    let unseen = branch(&rows, &id, &["e2e4", "c7c5"]).unwrap();
    assert_eq!(unseen.summary.total, 1);
    assert_eq!(unseen.summary.unseen, 1);
    assert_eq!(unseen.summary.accuracy_attempts, 0);
    assert_eq!(unseen.summary.accuracy_percent, None);
    assert_eq!(unseen.last_independent_at, None);
}

#[test]
fn branch_last_independent_ignores_preview_and_auto_context() {
    let (library, id) = single_repertoire(branching_pgn());
    let repertoire = library.repertoire(&id).unwrap();
    let main = key_at_path(repertoire, &strings(&["e2e4", "e7e5"]), "g1f3");
    let sideline = key_at_path(repertoire, &strings(&["e2e4", "c7c5"]), "g1f3");
    let mut learning = LearningData::default();
    for (key, event_id, event_type, happened_at) in [
        (
            &main,
            "main-independent",
            TrainingEventType::CorrectIndependent,
            NOW - 20,
        ),
        (
            &sideline,
            "sideline-independent",
            TrainingEventType::IncorrectIndependent,
            NOW - 10,
        ),
        (&main, "main-preview", TrainingEventType::Previewed, NOW - 1),
        (
            &sideline,
            "sideline-context",
            TrainingEventType::AutoContext,
            NOW,
        ),
    ] {
        learning
            .apply_attempt(
                attempt(key, event_id, event_type, happened_at),
                &SrsSettings::default(),
            )
            .unwrap();
    }
    assert_eq!(learning.item(&main).unwrap().last_review_at, Some(NOW - 1));
    assert_eq!(
        learning.item(&main).unwrap().last_independent_at,
        Some(NOW - 20)
    );

    let rows = branch_learning_analytics(
        &library,
        &learning,
        &scope(id.clone()),
        &SrsSettings::default(),
    )
    .unwrap();
    let all = branch(&rows, &id, &["e2e4"]).unwrap();
    assert_eq!(all.last_independent_at, Some(NOW - 10));
    assert_eq!(all.summary.accuracy_correct, 1);
    assert_eq!(all.summary.accuracy_wrong, 1);
    assert_eq!(all.summary.accuracy_attempts, 2);
    assert_eq!(all.summary.accuracy_percent, Some(50.0));
    assert_eq!(
        branch(&rows, &id, &["e2e4", "e7e5"])
            .unwrap()
            .last_independent_at,
        Some(NOW - 20)
    );
    assert_eq!(
        branch(&rows, &id, &["e2e4", "c7c5"])
            .unwrap()
            .last_independent_at,
        Some(NOW - 10)
    );
}

#[test]
fn disabled_branch_disappears_and_reenable_restores_history() {
    let (mut library, id) = single_repertoire(branching_pgn());
    let selected = scope(id.clone());
    let sideline = key_at_path(
        library.repertoire(&id).unwrap(),
        &strings(&["e2e4", "c7c5"]),
        "g1f3",
    );
    let mut learning = LearningData::default();
    learnt(learning.ensure_item(sideline.clone()), NOW + DAY);
    let before = learning.item(&sideline).unwrap().clone();

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "c7c5"]), true)
        .unwrap();
    let disabled =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert!(branch(&disabled, &id, &["e2e4", "c7c5"]).is_none());
    assert_eq!(branch(&disabled, &id, &["e2e4"]).unwrap().summary.total, 5);
    assert_eq!(learning.item(&sideline), Some(&before));

    let mut include_inactive = selected.clone();
    include_inactive.only_active = false;
    let inactive = branch_learning_analytics(
        &library,
        &learning,
        &include_inactive,
        &SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(
        branch(&inactive, &id, &["e2e4", "c7c5"])
            .unwrap()
            .summary
            .learnt,
        1
    );

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "c7c5"]), false)
        .unwrap();
    let enabled =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    let restored = branch(&enabled, &id, &["e2e4", "c7c5"]).unwrap();
    assert_eq!(restored.summary.total, 1);
    assert_eq!(restored.summary.learnt, 1);
    assert_eq!(restored.last_independent_at, Some(NOW - DAY));
    assert_eq!(learning.item(&sideline), Some(&before));
}

#[test]
fn max_depth_and_branch_root_limit_candidates_and_items() {
    let (library, id) =
        single_repertoire("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 *");
    let learning = LearningData::default();
    let depth_scope = scope(id.clone()).with_max_depth(Some(4));
    let depth_rows =
        branch_learning_analytics(&library, &learning, &depth_scope, &SrsSettings::default())
            .unwrap();
    assert_eq!(
        branch(&depth_rows, &id, &["e2e4"]).unwrap().summary.total,
        2
    );
    assert!(depth_rows.iter().all(|row| row.branch_root.len() <= 4));
    assert!(
        depth_rows
            .iter()
            .all(|row| !row.branch_root.contains(&"f1b5".to_owned()))
    );

    let root = strings(&["e2e4", "e7e5"]);
    let rooted_scope = scope(id.clone()).with_branch_root(root.clone());
    let rooted_rows =
        branch_learning_analytics(&library, &learning, &rooted_scope, &SrsSettings::default())
            .unwrap();
    assert!(!rooted_rows.is_empty());
    assert!(
        rooted_rows
            .iter()
            .all(|row| row.branch_root.starts_with(&root))
    );
    assert!(branch(&rooted_rows, &id, &["e2e4"]).is_none());
    assert_eq!(
        branch(&rooted_rows, &id, &["e2e4", "e7e5"])
            .unwrap()
            .summary
            .total,
        5
    );

    let limited_scope = rooted_scope.with_max_depth(Some(4));
    let limited_rows =
        branch_learning_analytics(&library, &learning, &limited_scope, &SrsSettings::default())
            .unwrap();
    assert_eq!(limited_rows.len(), 2);
    assert_eq!(
        branch(&limited_rows, &id, &["e2e4", "e7e5"])
            .unwrap()
            .summary
            .total,
        1
    );
    assert!(limited_rows.iter().all(|row| row.branch_root.len() <= 4));
}

#[test]
fn empty_scope_has_no_rows_or_whole_repertoire_root() {
    let (library, id) = single_repertoire("1. e4 *");
    let empty = StatsScope::new([], LearningSide::White, NOW);
    assert!(
        branch_learning_analytics(
            &library,
            &LearningData::default(),
            &empty,
            &SrsSettings::default(),
        )
        .unwrap()
        .is_empty()
    );

    let no_black_moves = StatsScope::for_repertoire(id, LearningSide::Black, NOW);
    assert!(
        branch_learning_analytics(
            &library,
            &LearningData::default(),
            &no_black_moves,
            &SrsSettings::default(),
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn branch_projection_is_deterministic_and_sorted() {
    let (library, id) = single_repertoire(branching_pgn());
    let selected = scope(id.clone());
    let mut learning = LearningData::default();
    for key in active_learning_keys(&library, &selected).unwrap() {
        learning.ensure_item(key);
    }
    let first =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    let mut sorted = first.clone();
    sorted.sort_by(|left, right| {
        (&left.repertoire_id, &left.branch_root).cmp(&(&right.repertoire_id, &right.branch_root))
    });
    assert_eq!(first, sorted);

    learning.items.reverse();
    let second =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn all_path_projection_honours_main_answer_override_and_disabled_main() {
    let (mut library, id) = single_repertoire("1. e4 (1. d4 d5) e5 *");
    let selected = scope(id.clone());
    let learning = LearningData::default();
    let default_rows =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(
        branch(&default_rows, &id, &["e2e4"]).unwrap().summary.total,
        1
    );
    assert!(branch(&default_rows, &id, &["d2d4"]).is_none());

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4"]), true)
        .unwrap();
    assert!(
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default(),)
            .unwrap()
            .is_empty()
    );

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4"]), false)
        .unwrap();
    let root = {
        let repertoire = library.repertoire(&id).unwrap();
        let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
        tree.position_key(tree.root()).unwrap().to_owned()
    };
    library
        .repertoire_mut(&id)
        .unwrap()
        .set_main_answer_override(&root, "d2d4", LearningSide::White)
        .unwrap();
    let overridden =
        branch_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert!(branch(&overridden, &id, &["e2e4"]).is_none());
    assert_eq!(
        branch(&overridden, &id, &["d2d4"]).unwrap().summary.total,
        1
    );
}
