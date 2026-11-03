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
        self.ui.mode = TrainingMode::White;
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
fn creating_an_empty_repertoire_selects_it_in_the_workspace() {
    let mut f = Fixture::new();
    f.ui.open_library();
    f.ui.edit_name = Some((None, "Empty opening".to_owned()));

    f.ui.save_name(None, "Empty opening", &f.settings);

    let created =
        f.ui.library
            .repertoires
            .iter()
            .find(|r| r.name == "Empty opening")
            .expect("the repertoire is saved");
    assert!(!f.ui.training_visible);
    assert_eq!(f.ui.selected.as_ref(), Some(&created.id));
    assert!(f.ui.edit_name.is_none());
    assert!(f.ui.error.is_none());
}

#[test]
fn clicking_a_row_selects_it_without_leaving_the_workspace() {
    let mut f = Fixture::new();
    f.ui.open_library();
    f.ui.selected = None;
    let ctx = egui::Context::default();
    let mut frame = |time: f64, events: Vec<egui::Event>| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    f.ui.show_library(ui, &f.settings);
                });
            },
        )
        .drop_without_applying_deltas();
        (f.ui.selected.clone(), f.ui.training_visible)
    };
    let click = || {
        let position = egui::pos2(200.0, 80.0);
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };

    frame(0.0, Vec::new());
    let (selected, training) = frame(1.0, click());
    assert_eq!(selected.as_ref(), Some(&f.id));
    assert!(!training);
    assert!(f.ui.tree.is_some());
}

#[test]
fn opening_index_has_the_deepest_eco_and_variation_name() {
    let game = chess_core::parse_str("1. e4 c5 2. Nf3 Nc6 3. Bb5 g6 4. O-O *").game;
    let mut library = Library::default();
    let id = library.create("Rossolimo", &game).unwrap();
    let tree = RepertoireTree::new(library.repertoire(&id).unwrap());

    let labels = opening_labels(&tree);

    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].eco, "B31");
    assert!(
        labels[0]
            .name
            .starts_with("Sicilian Defense: Nyezhmetdinov-Rossolimo Attack"),
        "{}",
        labels[0].name
    );
}

#[test]
fn rows_have_one_height_and_show_at_most_five_opening_names() {
    let f = Fixture::new();
    let row = |count: usize| LibraryRow {
        id: f.id.clone(),
        name: "Opening".to_owned(),
        openings: (0..count)
            .map(|index| OpeningLabel {
                eco: format!("B{index:02}"),
                name: format!("Opening {index}"),
            })
            .collect(),
    };

    assert_eq!(LibraryRow::height(), 28.0 + 22.0 * OPENING_LINES as f32);
    assert_eq!(row(0).visible_openings().len(), 0);
    assert_eq!(row(3).visible_openings().len(), 3);
    assert_eq!(row(8).visible_openings().len(), OPENING_LINES);
}

#[test]
fn load_persists_migration_of_legacy_training_limits() {
    let f = Fixture::new();
    let store = LibraryStore::new(f.dir.join("legacy-limits.json"));
    let mut library = Library::default();
    let id = library
        .create("Legacy limits", &chess_core::parse_str("1. e4 e5 *").game)
        .unwrap();
    let repertoire = library.repertoire_mut(&id).unwrap();
    repertoire.max_plies = 1;
    repertoire
        .set_prefix_disabled(&["e2e4".to_owned()], true)
        .unwrap();
    let pgn = repertoire.pgn.clone();
    store.save(&mut library).unwrap();

    let ui = RepertoireUi::with_stores(
        store.clone(),
        StatsStore::new(f.dir.join("legacy-stats.json")),
    );
    assert!(ui.library_error.is_none(), "{:?}", ui.library_error);
    let saved = store.load().unwrap();
    for library in [&ui.library, &saved] {
        let repertoire = library.repertoire(&id).unwrap();
        assert_eq!(repertoire.max_plies, u32::MAX);
        assert!(repertoire.disabled_prefixes.is_empty());
        assert_eq!(repertoire.pgn, pgn);
    }
}

#[test]
fn migration_write_failure_keeps_the_loaded_library_available() {
    let f = Fixture::new();
    let store = LibraryStore::new(f.dir.join("migration-failure.json"));
    let mut library = Library::default();
    let id = library
        .create(
            "Read only migration",
            &chess_core::parse_str("1. e4 e5 *").game,
        )
        .unwrap();
    let repertoire = library.repertoire_mut(&id).unwrap();
    repertoire.max_plies = 1;
    repertoire
        .set_prefix_disabled(&["e2e4".to_owned()], true)
        .unwrap();
    store.save(&mut library).unwrap();
    let lock = f.dir.join("migration-failure.json.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(lock).unwrap();

    let ui = RepertoireUi::with_stores(
        store.clone(),
        StatsStore::new(f.dir.join("migration-failure-stats.json")),
    );
    assert!(ui.library_error.is_some());
    let migrated = ui.library.repertoire(&id).unwrap();
    assert_eq!(migrated.max_plies, u32::MAX);
    assert!(migrated.disabled_prefixes.is_empty());
    let on_disk = store.load().unwrap();
    let unchanged = on_disk.repertoire(&id).unwrap();
    assert_eq!(unchanged.max_plies, 1);
    assert_eq!(unchanged.disabled_prefixes.len(), 1);
}

#[test]
fn practice_count_requires_a_move_for_the_selected_color() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    let id = library
        .create("White only", &chess_core::parse_str("1. e4 *").game)
        .unwrap();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());

    for (mode, expected) in [
        (TrainingMode::White, 1),
        (TrainingMode::Black, 0),
        (TrainingMode::Both, 1),
    ] {
        let scope = TrainingScope::whole();
        assert_eq!(
            f.ui.task_count(&id, mode, QueueMode::Practice, SrsPolicy::Due, &scope),
            None
        );
        f.deliver(f.completed_job());
        assert_eq!(
            f.ui.task_count(&id, mode, QueueMode::Practice, SrsPolicy::Due, &scope),
            Some(expected)
        );
    }
}

#[test]
fn persist_normalizes_limits_before_saving_and_indexing() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    let repertoire = library.repertoire_mut(&f.id).unwrap();
    repertoire.max_plies = 1;
    repertoire
        .set_prefix_disabled(&["e2e4".to_owned()], true)
        .unwrap();
    let pgn = repertoire.pgn.clone();

    assert!(f.ui.persist(library));
    let saved = f.ui.library_store.load().unwrap();
    for library in [&f.ui.library, &saved] {
        let repertoire = library.repertoire(&f.id).unwrap();
        assert_eq!(repertoire.max_plies, u32::MAX);
        assert!(repertoire.disabled_prefixes.is_empty());
        assert_eq!(repertoire.pgn, pgn);
    }
    f.deliver(f.completed_job());
    assert_eq!(
        f.ui.trees[&f.id]
            .training_path_count(&TrainingScope::whole())
            .unwrap(),
        1
    );
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
fn completed_training_start_is_ignored_after_leaving_the_workspace() {
    let mut f = Fixture::new();
    f.start();
    let ready = f.completed_job();
    f.ui.open_library();
    f.deliver(ready);
    assert!(!f.ui.preparing);
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
        saved.runs(&f.id, TrainingMode::White).unwrap().completions,
        1
    );
    assert_eq!(saved.move_stats.iter().map(|s| s.correct).sum::<u32>(), 1);
    assert!(f.ui.error.is_none());
}

#[test]
fn reset_clears_the_saved_statistics_of_the_selected_mode() {
    let mut f = Fixture::new();
    f.start();
    let ready = f.completed_job();
    f.deliver(ready);
    f.ui.debug_moves("e2e4 e7e5");
    assert!(
        f.ui.stats_store
            .load()
            .unwrap()
            .runs(&f.id, TrainingMode::White)
            .is_some()
    );

    f.ui.open_library();
    f.ui.reset_stats = Some(f.id.clone());
    f.ui.reset_statistics(&f.id);

    let saved = f.ui.stats_store.load().unwrap();
    assert!(saved.runs(&f.id, TrainingMode::White).is_none());
    assert!(saved.move_stats.is_empty());
    assert!(f.ui.reset_stats.is_none());
    assert!(f.ui.error.is_none());
}

#[test]
fn start_ignores_legacy_queue() {
    let mut f = Fixture::new();
    f.settings.repertoire_modes.insert(
        f.id.to_string(),
        (TrainingMode::White, QueueMode::SpacedRepetition),
    );
    f.ui.select(f.id.clone(), &f.settings);
    assert_eq!(f.ui.mode, TrainingMode::White);

    f.ui.start(&mut f.settings);
    assert!(f.ui.preparing);
    assert_eq!(
        f.settings.repertoire_modes.get(f.id.as_str()),
        Some(&(TrainingMode::White, QueueMode::Practice))
    );

    f.deliver(f.completed_job());
    assert!(f.ui.training_visible);
    f.ui.debug_moves("e2e4 e7e5");
    let training = f.ui.training.as_ref().unwrap();
    assert!(!training.target_available());
    assert!(!training.has_unsaved());
    assert_eq!(
        f.ui.stats_store
            .load()
            .unwrap()
            .runs(&f.id, TrainingMode::White)
            .unwrap()
            .completions,
        1
    );
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
fn variant_count_does_not_depend_on_completed_training_runs() {
    let mut f = Fixture::new();
    let game = chess_core::parse_str("1. e4 e5 (1... c5) 2. Nf3 *").game;
    let mut library = f.ui.library.clone();
    let id = library.create("Two paths", &game).unwrap();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());
    assert!(f.ui.stats.runs(&id, TrainingMode::Both).is_none());

    let scope = TrainingScope::whole();
    assert_eq!(
        f.ui.task_count(
            &id,
            TrainingMode::Both,
            QueueMode::Practice,
            SrsPolicy::Due,
            &scope
        ),
        None
    );
    f.deliver(f.completed_job());
    assert_eq!(
        f.ui.task_count(
            &id,
            TrainingMode::Both,
            QueueMode::Practice,
            SrsPolicy::Due,
            &scope
        ),
        Some(2)
    );

    f.ui.stats.runs.push(trainer::repertoire::RunStats {
        repertoire_id: id.clone(),
        mode: TrainingMode::Both,
        completions: 7,
        last: Some(now()),
        recent: Vec::new(),
    });
    f.ui.stats.revision += 1;
    assert_eq!(
        f.ui.task_count(
            &id,
            TrainingMode::Both,
            QueueMode::Practice,
            SrsPolicy::Due,
            &scope
        ),
        None
    );
    f.deliver(f.completed_job());
    assert_eq!(
        f.ui.task_count(
            &id,
            TrainingMode::Both,
            QueueMode::Practice,
            SrsPolicy::Due,
            &scope
        ),
        Some(2)
    );
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
