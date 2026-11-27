use chess_core::{Game, NodeId};

use super::*;

fn repertoire(pgn: &str) -> Repertoire {
    let mut library = Library::default();
    let id = library
        .create("Scope", &chess_core::parse_str(pgn).game)
        .unwrap();
    library.repertoire(&id).unwrap().clone()
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
