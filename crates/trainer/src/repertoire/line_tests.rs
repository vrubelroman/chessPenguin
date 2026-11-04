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

fn line<'a>(
    rows: &'a [LineLearningAnalytics],
    repertoire_id: &RepertoireId,
    path: &[&str],
) -> Option<&'a LineLearningAnalytics> {
    rows.iter().find(|row| {
        &row.repertoire_id == repertoire_id
            && row.line.iter().map(String::as_str).eq(path.iter().copied())
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
    "1. e4 e5 (1... c5 2. Nf3) 2. Nf3 Nc6 3. Bb5 *"
}

#[test]
fn at_30_overlapping_lines_share_items_but_global_scope_deduplicates() {
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
    let rows = line_learning_analytics(
        &library,
        &learning,
        &scope(first.clone()),
        &SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    let main_line = line(
        &rows,
        &first,
        &[
            "e2e4", "d7d5", "d2d4", "g8h6", "g1f3", "h6g8", "f3g1", "g8f6", "c2c4", "e7e6", "g2g3",
        ],
    )
    .unwrap();
    let transposing_line = line(&rows, &first, &["e2e4", "g8f6", "d2d4", "d7d5", "c2c4"]).unwrap();
    assert_eq!(main_line.summary.total, 6);
    assert_eq!(transposing_line.summary.total, 3);
    for row in [main_line, transposing_line] {
        assert_eq!(row.summary.learnt, 1);
        assert_eq!(row.summary.accuracy_correct, 3);
        assert_eq!(row.summary.accuracy_attempts, 3);
        assert!(!row.line_learnt);
        assert!(!row.line_fresh);
    }

    let global_scope = StatsScope::new([first, second], LearningSide::White, NOW);
    let global =
        aggregate_scope(&library, &learning, &global_scope, &SrsSettings::default()).unwrap();
    assert_eq!(global.summary.total, 7);
    assert_eq!(global.summary.learnt, 1);
    assert_eq!(global.summary.accuracy_correct, 3);
    assert!(global.summary.total < main_line.summary.total + transposing_line.summary.total);
    let mean = (main_line.summary.learnt_percent.unwrap()
        + transposing_line.summary.learnt_percent.unwrap())
        / 2.0;
    assert!((global.summary.learnt_percent.unwrap() - mean).abs() > f64::EPSILON);
}

#[test]
fn line_learnt_and_fresh_follow_success_due_uncertain_and_wrong() {
    let (library, id) = single_repertoire("1. e4 e5 2. Nf3 *");
    let selected = scope(id.clone());
    let keys = active_learning_keys(&library, &selected).unwrap();
    let mut learning = LearningData::default();
    let initial =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].summary.total, 2);
    assert_eq!(initial[0].summary.unseen, 2);
    assert!(!initial[0].line_learnt);
    assert!(!initial[0].line_fresh);

    for key in &keys {
        let item = learning.ensure_item(key.clone());
        for happened_at in [NOW - 3, NOW - 2, NOW - 1] {
            apply_independent_correct(item, happened_at, &SrsSettings::default());
        }
        item.next_review_at = Some(NOW + DAY);
        item.previous_interval_seconds = Some(DAY);
    }
    let fresh =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(fresh[0].summary.learnt, 2);
    assert_eq!(fresh[0].summary.due, 0);
    assert_eq!(fresh[0].summary.uncertain, 0);
    assert!(fresh[0].line_learnt);
    assert!(fresh[0].line_fresh);

    let first = keys.iter().next().unwrap();
    learning.ensure_item((*first).clone()).next_review_at = Some(NOW);
    let due =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(due[0].summary.due, 1);
    assert_eq!(due[0].summary.uncertain, 0);
    assert!(due[0].line_learnt);
    assert!(!due[0].line_fresh);

    learning.ensure_item((*first).clone()).next_review_at = Some(NOW - 8 * DAY);
    let uncertain =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(uncertain[0].summary.due, 1);
    assert_eq!(uncertain[0].summary.uncertain, 1);
    assert!(uncertain[0].line_learnt);
    assert!(!uncertain[0].line_fresh);

    apply_independent_wrong(
        learning.ensure_item((*first).clone()),
        "d2d4",
        NOW,
        &SrsSettings::default(),
    );
    let wrong =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(wrong[0].summary.learning, 1);
    assert_eq!(wrong[0].summary.accuracy_wrong, 1);
    assert!(!wrong[0].line_learnt);
    assert!(!wrong[0].line_fresh);

    for happened_at in [NOW + 1, NOW + 2, NOW + 3] {
        apply_independent_correct(
            learning.ensure_item((*first).clone()),
            happened_at,
            &SrsSettings::default(),
        );
    }
    let mut restored_scope = selected;
    restored_scope.as_of = NOW + 3;
    let restored = line_learning_analytics(
        &library,
        &learning,
        &restored_scope,
        &SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(restored[0].summary.learnt, 2);
    assert_eq!(restored[0].summary.due, 0);
    assert_eq!(restored[0].summary.uncertain, 0);
    assert!(restored[0].line_learnt);
    assert!(restored[0].line_fresh);
}

#[test]
fn empty_lines_are_omitted_and_never_reported_learnt() {
    let (library, id) = single_repertoire("1. e4 *");
    let empty_scope = StatsScope::new([], LearningSide::White, NOW);
    assert!(
        line_learning_analytics(
            &library,
            &LearningData::default(),
            &empty_scope,
            &SrsSettings::default(),
        )
        .unwrap()
        .is_empty()
    );

    let no_black_items = StatsScope::for_repertoire(id, LearningSide::Black, NOW);
    let rows = line_learning_analytics(
        &library,
        &LearningData::default(),
        &no_black_items,
        &SrsSettings::default(),
    )
    .unwrap();
    assert!(rows.is_empty());
    assert!(rows.iter().all(|row| !row.line_learnt && !row.line_fresh));
}

#[test]
fn line_scope_honours_max_depth_disable_and_branch_root() {
    let (mut library, id) = single_repertoire(branching_pgn());
    let learning = LearningData::default();
    let depth_scope = scope(id.clone()).with_max_depth(Some(2));
    let depth = line_learning_analytics(&library, &learning, &depth_scope, &SrsSettings::default())
        .unwrap();
    assert_eq!(depth.len(), 2);
    assert!(
        depth
            .iter()
            .all(|row| row.line.len() == 2 && row.summary.total == 1)
    );

    library
        .repertoire_mut(&id)
        .unwrap()
        .set_prefix_disabled(&strings(&["e2e4", "c7c5"]), true)
        .unwrap();
    let selected = scope(id.clone());
    let disabled =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(disabled.len(), 1);
    assert!(!disabled[0].line.starts_with(&strings(&["e2e4", "c7c5"])));

    let mut include_inactive = selected.clone();
    include_inactive.only_active = false;
    let inactive = line_learning_analytics(
        &library,
        &learning,
        &include_inactive,
        &SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(inactive.len(), 2);

    let root = strings(&["e2e4", "e7e5"]);
    let rooted = line_learning_analytics(
        &library,
        &learning,
        &selected.with_branch_root(root.clone()),
        &SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(rooted.len(), 1);
    assert!(rooted[0].line.starts_with(&root));
    assert_eq!(
        rooted[0].line,
        strings(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"])
    );
    assert_eq!(rooted[0].summary.total, 2);
}

#[test]
fn line_projection_is_deterministic_and_sorted() {
    let mut library = Library::default();
    let first = add_repertoire(&mut library, "First", branching_pgn());
    let second = add_repertoire(&mut library, "Second", branching_pgn());
    let selected = StatsScope::new([first, second], LearningSide::White, NOW);
    let mut learning = LearningData::default();
    for key in active_learning_keys(&library, &selected).unwrap() {
        learning.ensure_item(key);
    }
    let before =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    let mut sorted = before.clone();
    sorted.sort_by(|left, right| {
        (&left.repertoire_id, &left.line).cmp(&(&right.repertoire_id, &right.line))
    });
    assert_eq!(before, sorted);

    learning.items.reverse();
    let after =
        line_learning_analytics(&library, &learning, &selected, &SrsSettings::default()).unwrap();
    assert_eq!(before, after);
}
