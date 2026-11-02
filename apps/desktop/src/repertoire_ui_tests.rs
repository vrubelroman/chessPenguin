use super::*;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    ui: RepertoireUi,
    settings: Settings,
    dir: PathBuf,
    id: RepertoireId,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "cp-repertoire-ui-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let library_store = LibraryStore::new(dir.join("library.json"));
        let mut library = Library::default();
        let id = library
            .create("Async opening", &chess_core::parse_str("1. e4 e5 *").game)
            .unwrap();
        library_store.save(&mut library).unwrap();
        let mut ui =
            RepertoireUi::with_stores(library_store, StatsStore::new(dir.join("stats.json")));
        let event = ui.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        ui.tx.send(event).unwrap();
        let mut settings = Settings::default();
        ui.poll(&mut settings);
        assert!(!ui.indexing);
        ui.select(id.clone(), &settings);
        Self {
            ui,
            settings,
            dir,
            id,
        }
    }

    fn completed_job(&self) -> Event {
        self.ui.rx.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn deliver(&mut self, event: Event) {
        self.ui.tx.send(event).unwrap();
        self.ui.poll(&mut self.settings);
    }

    fn start(&mut self) {
        self.ui.mode = TrainingMode::Both;
        self.ui.queue = QueueMode::ReviewInOrder;
        self.ui.start(&mut self.settings);
        assert!(self.ui.preparing);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn stale_index_cannot_replace_a_newer_rebuild() {
    let mut f = Fixture::new();
    f.ui.rebuild_tree();
    let old = f.completed_job();
    f.ui.rebuild_tree();
    let current = f.completed_job();
    f.deliver(old);
    assert!(f.ui.indexing);
    assert!(f.ui.tree.is_none());
    assert!(f.ui.trees.is_empty());
    f.deliver(current);
    assert!(!f.ui.indexing);
    assert!(f.ui.tree.is_some());
    assert!(f.ui.trees.contains_key(&f.id));
}

#[test]
fn completed_training_start_is_ignored_after_leaving_the_detail_screen() {
    let mut f = Fixture::new();
    f.start();
    let ready = f.completed_job();
    f.ui.open_library();
    f.deliver(ready);
    assert!(!f.ui.preparing);
    assert!(!f.ui.detail);
    assert!(!f.ui.training_visible);
    assert!(f.ui.training.is_none());
    assert!(f.ui.error.is_none());
}

#[test]
fn stale_training_start_cannot_cancel_or_replace_a_new_generation() {
    let mut f = Fixture::new();
    f.start();
    let old = f.completed_job();
    let old_generation = f.ui.training_generation;
    f.ui.open_library();
    f.ui.select(f.id.clone(), &f.settings);
    f.start();
    let current = f.completed_job();
    assert!(f.ui.training_generation > old_generation);
    f.deliver(old);
    f.deliver(Event::TrainingReady(
        old_generation,
        Err("stale failure".into()),
    ));
    assert!(f.ui.preparing);
    assert!(f.ui.training.is_none());
    assert!(f.ui.error.is_none());
    f.deliver(current);
    assert!(!f.ui.preparing);
    assert!(f.ui.training_visible);
    assert!(f.ui.training.is_some());
}

#[test]
fn async_start_can_finish_a_real_opening_and_persist_its_statistics() {
    let mut f = Fixture::new();
    f.start();
    let ready = f.completed_job();
    f.deliver(ready);
    assert!(f.ui.training_visible);
    f.ui.debug_moves("e2e4 e7e5");
    let training = f.ui.training.as_ref().unwrap();
    assert!(!training.target_available());
    assert!(!training.has_unsaved());
    let saved = f.ui.stats_store.load().unwrap();
    assert_eq!(
        saved.runs(&f.id, TrainingMode::Both).unwrap().completions,
        1
    );
    assert_eq!(saved.move_stats.iter().map(|s| s.correct).sum::<u32>(), 2);
    assert!(f.ui.error.is_none());
}

#[test]
fn due_count_reuses_pending_work_across_minutes_and_discards_evicted_results() {
    let mut f = Fixture::new();
    let key = CountKey {
        id: f.id.clone(),
        mode: TrainingMode::White,
        queue: QueueMode::SpacedRepetition,
        policy: SrsPolicy::Due,
        scope: TrainingScope::whole(),
        library_revision: f.ui.library.revision,
        stats_revision: f.ui.stats.revision,
        minute: now() / 60 - 2,
    };
    f.ui.counts.insert(key.clone(), None);
    assert_eq!(
        f.ui.task_count(&f.id, key.mode, key.queue, key.policy, &key.scope),
        None
    );
    assert_eq!(f.ui.counts.len(), 1);
    assert_eq!(f.ui.counts.get(&key), Some(&None));
    assert!(matches!(f.ui.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    f.deliver(Event::Counted(key.clone(), 0));
    assert_eq!(f.ui.counts.get(&key), Some(&Some(0)));
    f.ui.counts.clear();
    f.deliver(Event::Counted(key, 99));
    assert!(f.ui.counts.is_empty());
}

#[test]
fn both_and_review_counts_have_no_minute_expiry_and_complete_asynchronously() {
    let mut f = Fixture::new();
    for (mode, queue, expected) in [
        (TrainingMode::Both, QueueMode::LeastRecent, 1),
        (TrainingMode::White, QueueMode::ReviewInOrder, 1),
    ] {
        let scope = TrainingScope::whole();
        assert_eq!(
            f.ui.task_count(&f.id, mode, queue, SrsPolicy::Due, &scope),
            None
        );
        assert!(
            f.ui.counts
                .keys()
                .filter(|k| k.mode == mode)
                .all(|k| k.minute == 0)
        );
        let event = f.completed_job();
        f.deliver(event);
        assert_eq!(
            f.ui.task_count(&f.id, mode, queue, SrsPolicy::Due, &scope),
            Some(expected)
        );
        assert!(matches!(f.ui.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
#[ignore = "manual corpus import and indexing measurement"]
fn corpus_import_preserves_source_and_reports_real_outcome() {
    for filename in ["The Ruy Lopez - Move by Move.pgn", "Alekhin.pgn"] {
        println!("File: {filename}");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../pgnFilesforExample")
            .join(filename);
        let before = std::fs::read(&path).unwrap();
        let cancelled = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let imported = read_import(&path, None, Library::default(), &cancelled);
        let import_elapsed = started.elapsed();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "import changed source bytes"
        );

        // Count the source independently, including records after any rejected one.
        let db = PgnDatabase::from_bytes(before.clone(), None, &mut |_| {}, &cancelled).unwrap();
        let source_nodes: usize = (0..db.len())
            .map(|index| {
                db.load_game(index)
                    .unwrap()
                    .game
                    .attached_nodes()
                    .len()
                    .saturating_sub(1)
            })
            .sum();
        println!(
            "Corpus: {} bytes; {} records; {} parsed source move nodes",
            before.len(),
            db.len(),
            source_nodes
        );
        match imported {
            Ok(preview) => {
                let repertoire = preview.library.repertoire(&preview.id).unwrap();
                let started = std::time::Instant::now();
                let tree = RepertoireTree::new(repertoire);
                let index_elapsed = started.elapsed();
                let nodes = tree.game().attached_nodes().len().saturating_sub(1);
                assert_eq!(preview.records, db.len());
                assert!(nodes > 0);
                println!(
                    "IMPORTED: {} records; {} merged move nodes; import {:?}; index {:?}; combined {:?}",
                    preview.records,
                    nodes,
                    import_elapsed,
                    index_elapsed,
                    import_elapsed + index_elapsed
                );
            }
            Err(reason) => {
                assert!(!reason.is_empty());
                println!(
                    "REJECTED (no repertoire imported, indexing not run): {reason}; read_import elapsed {import_elapsed:?}"
                );
            }
        }
        if filename == "Alekhin.pgn" {
            let fixture = Fixture::new();
            let subset_path = fixture.dir.join("Alekhin-first-200.pgn");
            let subset = (0..200)
                .map(|index| db.raw_text(index).expect("source record exists"))
                .collect::<Vec<_>>()
                .join("\n\n");
            std::fs::write(&subset_path, subset.as_bytes()).unwrap();
            let started = std::time::Instant::now();
            let mut preview = read_import(&subset_path, None, Library::default(), &cancelled)
                .expect("the first 200 complete source records must import successfully");
            let subset_import_elapsed = started.elapsed();
            assert_eq!(preview.records, 200);
            let repertoire = preview.library.repertoire(&preview.id).unwrap();
            let started = std::time::Instant::now();
            let tree = RepertoireTree::new(repertoire);
            let index_elapsed = started.elapsed();
            let nodes = tree.game().attached_nodes().len().saturating_sub(1);
            let canonical = tree.game().canonical_tree();
            let started = std::time::Instant::now();
            let store = LibraryStore::new(fixture.dir.join("corpus-library.json"));
            store.save(&mut preview.library).unwrap();
            let loaded = store.load().unwrap();
            assert_eq!(
                loaded
                    .repertoire(&preview.id)
                    .unwrap()
                    .game()
                    .canonical_tree(),
                canonical
            );
            let roundtrip_elapsed = started.elapsed();
            assert_eq!(std::fs::read(&subset_path).unwrap(), subset.as_bytes());
            assert!(nodes > 0);
            println!(
                "IMPORTED TEMP FIRST 200: {} bytes; {} merged move nodes; import {:?}; index {:?}; import+index {:?}; save/reload+canonical verification {:?}",
                subset.len(),
                nodes,
                subset_import_elapsed,
                index_elapsed,
                subset_import_elapsed + index_elapsed,
                roundtrip_elapsed
            );
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "corpus check changed source bytes"
        );
    }
}

#[test]
fn expanding_and_collapsing_variations_only_changes_visible_rows() {
    let game = chess_core::parse_str("1. e4 e5 (1... c5 2. Nf3) 2. Bc4 *").game;
    let before = game.canonical_tree();
    let e4 = game.children(NodeId::ROOT)[0];
    let e5 = game.children(e4)[0];
    let c5 = game.children(e4)[1];
    let bc4 = game.children(e5)[0];
    let nf3 = game.children(c5)[0];
    let mut expanded = HashSet::new();
    let collapsed = tree_rows(&game, &expanded);
    assert_eq!(collapsed, vec![(e4, 0), (e5, 0), (bc4, 0)]);
    expanded.insert(e4);
    let visible = tree_rows(&game, &expanded);
    assert_eq!(visible, vec![(e4, 0), (e5, 0), (bc4, 0), (c5, 1), (nf3, 1)]);
    assert_eq!(game.canonical_tree(), before);
    expanded.remove(&e4);
    assert_eq!(tree_rows(&game, &expanded), collapsed);
    assert_eq!(game.canonical_tree(), before);
}
