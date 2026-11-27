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
fn adding_and_deleting_never_touches_the_source_file() {
    let dir = temp_dir("source-file");
    let source_path = dir.join("legacy.pgn");
    let bytes = b"[Event \"Legacy\"]\n\n1. e4 c5 2. Nf3 d6 *\n\xff";
    std::fs::write(&source_path, bytes).unwrap();
    let source = game("1. e4 c5 2. Nf3 d6 3. Bb5+ *");
    let mut library = Library::default();
    let id = library.create_empty("Snapshot").unwrap();
    let at = source.mainline()[3];
    library.add_prefix(&id, &source, at).unwrap();
    library.delete(&id).unwrap();
    assert_eq!(std::fs::read(&source_path).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn v2_stats_without_the_random_cursor_field_still_read_and_save() {
    let mut old = serde_json::to_value(StatsData::default()).unwrap();
    assert!(
        old.as_object_mut()
            .unwrap()
            .remove("random_practice_cursors")
            .is_some()
    );
    let stats: StatsData = serde_json::from_value(old).unwrap();
    let saved = serde_json::to_value(&stats).unwrap();
    assert!(saved["random_practice_cursors"].is_array());
}
