use std::sync::atomic::AtomicU64;

use super::*;
use trainer::repertoire::{QueueMode, TrainingMode, active_learning_memberships};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);
const DAY: i64 = 24 * 60 * 60;

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "cp-repertoire-ui-{}-{}-{name}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn stores(&self) -> (LibraryStore, LearningStore) {
        (
            LibraryStore::new(self.0.join("repertoires.json")),
            LearningStore::new(self.0.join("repertoire-training.json")),
        )
    }

    fn ui(&self) -> RepertoireUi {
        let (library, learning) = self.stores();
        RepertoireUi::with_stores(library, learning)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn save_library(dir: &TestDir, repertoires: &[(&str, &str)]) -> Vec<RepertoireId> {
    let mut library = Library::default();
    let ids = repertoires
        .iter()
        .map(|(name, pgn)| {
            library
                .create(*name, &chess_core::parse_str(pgn).game)
                .unwrap()
        })
        .collect();
    dir.stores().0.save(&mut library).unwrap();
    ids
}

/// Frames until the background index and counts are in.
fn settle(ui: &mut RepertoireUi, ctx: &egui::Context, settings: &mut Settings, size: egui::Vec2) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        frame(ui, ctx, settings, size, Vec::new());
        let ready = ui.preparing.is_none()
            && ui.lines_job.is_none()
            && !ui.learning_loading
            && ui
                .library
                .repertoires
                .iter()
                .all(|r| ui.index.contains_key(&r.id) && ui.stats.contains_key(&r.id))
            && ui.stats_key.as_ref().is_some_and(|key| {
                key.library_revision == ui.library.revision
                    && key.learning_revision == ui.learning.revision
            })
            && ui.stats_shown == ui.stats_key;
        if ready || Instant::now() > deadline {
            frame(ui, ctx, settings, size, Vec::new());
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn frame(
    ui: &mut RepertoireUi,
    ctx: &egui::Context,
    settings: &mut Settings,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> Vec<RepertoireAction> {
    let mut actions = Vec::new();
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |root| actions = ui.show(root, settings),
    )
    .drop_without_applying_deltas();
    actions
}

fn panel_rect(ctx: &egui::Context) -> egui::Rect {
    egui::containers::panel::PanelState::load(ctx, egui::Id::new("opening_side"))
        .map(|state| state.outer_rect)
        .unwrap_or(egui::Rect::NOTHING)
}

fn key(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn the_card_says_what_to_do_in_every_state() {
    let stats = |due, unseen, total| RowStats {
        total,
        due,
        unseen,
        learning: 0,
        learnt: total - unseen,
        next: None,
    };
    assert_eq!(card_state(Some(true), None), CardState::Counting);
    assert_eq!(
        card_state(Some(true), Some(&Err("broken".into()))),
        CardState::Failed("broken".into())
    );
    assert_eq!(
        card_state(Some(false), Some(&Ok(stats(0, 0, 0)))),
        CardState::Empty
    );
    assert_eq!(
        card_state(Some(true), Some(&Ok(stats(0, 0, 0)))),
        CardState::OtherColor
    );
    assert_eq!(
        card_state(Some(true), Some(&Ok(stats(3, 40, 50)))),
        CardState::Review { due: 3, new: 10 }
    );
    assert_eq!(
        card_state(Some(true), Some(&Ok(stats(0, 4, 50)))),
        CardState::OnlyNew { new: 4 }
    );
    assert_eq!(
        card_state(Some(true), Some(&Ok(stats(0, 0, 50)))),
        CardState::Done { next: None }
    );
    // Every state has a main button that does something, but «Counting».
    for (state, side) in [
        (CardState::Review { due: 3, new: 10 }, LearningSide::White),
        (CardState::OnlyNew { new: 4 }, LearningSide::White),
        (
            CardState::Done {
                next: Some((1_800_000_000, 2)),
            },
            LearningSide::White,
        ),
        (CardState::Empty, LearningSide::White),
        (CardState::OtherColor, LearningSide::White),
        (CardState::Failed("x".into()), LearningSide::Black),
    ] {
        let (label, enabled, item) = main_button(&state, side);
        assert!(enabled && item.is_some() && !label.is_empty(), "{state:?}");
        let (today, _) = card_texts(&state, side);
        assert!(!today.is_empty(), "{state:?}");
    }
    assert!(!main_button(&CardState::Counting, LearningSide::White).1);
    assert_eq!(
        main_button(&CardState::OtherColor, LearningSide::White).0,
        tr("Switch colour to Black")
    );
    assert_eq!(
        card_texts(&CardState::Review { due: 3, new: 10 }, LearningSide::White),
        (
            trf("Today: {n} to review · {m} new", &[("n", &3), ("m", &10)]),
            trf("About {t} min", &[("t", &5)])
        )
    );
    // Under a minute the time is not shown; a part with zero is left out.
    assert_eq!(
        card_texts(&CardState::Review { due: 2, new: 0 }, LearningSide::White),
        (trf("Today: {n} to review", &[("n", &2)]), String::new())
    );
}

#[test]
fn today_counts_until_the_end_of_the_day_and_at_most_ten_new() {
    let mut library = Library::default();
    let pgn = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 \
               8. c3 O-O 9. h3 Na5 10. Bc2 c5 11. d4 Qc7 12. Nbd2 *";
    let id = library
        .create("Ruy", &chess_core::parse_str(pgn).game)
        .unwrap();
    let today = layout::end_of_today();
    let scope = StatsScope::for_repertoire(id.clone(), LearningSide::White, today);
    let keys: Vec<_> = active_learning_memberships(&library, &scope)
        .unwrap()
        .into_iter()
        .map(|membership| membership.key)
        .collect();
    assert_eq!(keys.len(), 12);
    let mut learning = LearningData::default();
    let now = layout::now();
    for (index, key) in keys.iter().take(3).enumerate() {
        let item = learning.ensure_item(key.clone());
        item.introduced = true;
        item.correct_streak = 3;
        // Due later today, due long ago, due in two days.
        item.next_review_at = Some(match index {
            0 => today - 1,
            1 => now - 10 * DAY,
            _ => now + 2 * DAY,
        });
    }
    let stats = row_stats(&library, &learning, &id, LearningSide::White, today).unwrap();
    assert_eq!(
        (stats.total, stats.due, stats.unseen, stats.learnt),
        (12, 2, 9, 3)
    );
    assert_eq!(stats.new_today(), 9);
    assert_eq!(stats.passed(), 3 + stats.learning);
    assert_eq!(
        card_state(Some(true), Some(&Ok(stats.clone()))),
        CardState::Review { due: 2, new: 9 }
    );
    let black = row_stats(&library, &learning, &id, LearningSide::Black, today).unwrap();
    assert_eq!(black.total, 11);
    assert_eq!(black.new_today(), NEW_PER_SESSION);
}

#[test]
fn loading_lifts_the_old_depth_limit_without_touching_the_pgn() {
    let dir = TestDir::new("depth");
    let ids = save_library(&dir, &[("Limited", "1. e4 e5 2. Nf3 *")]);
    let (store, _) = dir.stores();
    let stored = store.load().unwrap();
    assert_eq!(
        stored.repertoires[0].max_plies,
        trainer::repertoire::DEFAULT_MAX_PLIES
    );
    let ui = dir.ui();
    let repertoire = ui.library.repertoire(&ids[0]).unwrap();
    assert_eq!(repertoire.max_plies, u32::MAX);
    assert_eq!(repertoire.pgn, stored.repertoires[0].pgn);
    // Nothing is written until the user changes something.
    assert_eq!(
        store.load().unwrap().repertoires[0].max_plies,
        stored.repertoires[0].max_plies
    );
}

#[test]
fn an_import_reads_name_colour_counts_and_warnings() {
    let dir = TestDir::new("import");
    let path = dir.0.join("caro.pgn");
    std::fs::write(
        &path,
        "[Event \"Caro-Kann for Black\"]\n\n1. e4 c6 2. d4 (2. Nc3 d5) (2. c4 d5) d5 *\n\n\
         [Event \"Empty\"]\n\n*\n",
    )
    .unwrap();
    let preview = read_import(&path, None, Library::default(), &AtomicBool::new(false)).unwrap();
    assert_eq!(preview.default_name, "Caro-Kann for Black");
    assert_eq!(preview.suggested, LearningSide::Black);
    assert_eq!(preview.games, 2);
    assert_eq!(preview.lines, 3);
    assert_eq!(preview.own(LearningSide::Black), 4);
    assert_eq!(preview.own(LearningSide::White), 4);
    assert_eq!(
        preview.warnings,
        [trf("Game {n} has no moves", &[("n", &2)])]
    );
    // The source file is only read.
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("[Event \"Caro-Kann")
    );

    let mut ui = dir.ui();
    let mut settings = Settings::default();
    ui.dialog = Some(Dialog::Import(ImportDialog {
        job: 1,
        target: None,
        name: preview.default_name.clone(),
        side: preview.suggested,
        preview: Some(preview),
        error: None,
        cancel: Arc::new(AtomicBool::new(false)),
    }));
    assert!(ui.confirm_import(&mut settings));
    let id = ui.selected.clone().unwrap();
    assert_eq!(
        ui.library.repertoire(&id).unwrap().name,
        "Caro-Kann for Black"
    );
    assert_eq!(layout::side_of(&settings, &id), LearningSide::Black);
    assert_eq!(
        settings.repertoire_modes.get(id.as_str()),
        Some(&(TrainingMode::Black, QueueMode::SpacedRepetition))
    );
    assert_eq!(dir.stores().0.load().unwrap().repertoires.len(), 1);
}

#[test]
fn a_white_repertoire_is_suggested_for_white() {
    let game = chess_core::parse_str("1. e4 e5 (1... c5 2. Nf3) (1... e6 2. d4) 2. Nf3 *").game;
    assert_eq!(suggested_side(&game), LearningSide::White);
    let even = chess_core::parse_str("1. e4 e5 *").game;
    assert_eq!(suggested_side(&even), LearningSide::White);
}

#[test]
fn old_colours_open_as_before_and_both_opens_as_white() {
    let mut settings = Settings::default();
    let id = Library::default().create_empty("Any").unwrap();
    assert_eq!(layout::side_of(&settings, &id), LearningSide::White);
    settings.repertoire_modes.insert(
        id.to_string(),
        (TrainingMode::Black, QueueMode::ReviewInOrder),
    );
    assert_eq!(layout::side_of(&settings, &id), LearningSide::Black);
    settings
        .repertoire_modes
        .insert(id.to_string(), (TrainingMode::Both, QueueMode::LeastRecent));
    assert_eq!(layout::side_of(&settings, &id), LearningSide::White);
    layout::set_side(&mut settings, &id, LearningSide::Black);
    assert_eq!(
        settings.repertoire_modes[id.as_str()],
        (TrainingMode::Black, QueueMode::SpacedRepetition)
    );
}

#[test]
fn the_three_screens_keep_the_board_and_the_panel_in_one_place() {
    let dir = TestDir::new("geometry");
    save_library(
        &dir,
        &[
            ("Italian", "1. e4 e5 2. Nf3 Nc6 3. Bc4 *"),
            ("London", "1. d4 d5 2. Bf4 *"),
        ],
    );
    for (size, left) in [
        (vec2(1920.0, 1080.0), 1360.0),
        (vec2(1400.0, 900.0), 896.0),
        (vec2(960.0, 600.0), 580.0),
    ] {
        let mut ui = dir.ui();
        let mut settings = Settings::default();
        let ctx = egui::Context::default();
        crate::assets::install(&ctx);
        settle(&mut ui, &ctx, &mut settings, size);
        let repertoires = panel_rect(&ctx);
        assert!(
            (repertoires.left() - left).abs() <= 1.0,
            "{repertoires:?} in {size:?}"
        );
        // Selecting another row moves nothing.
        frame(
            &mut ui,
            &ctx,
            &mut settings,
            size,
            vec![key(Key::ArrowDown)],
        );
        frame(&mut ui, &ctx, &mut settings, size, Vec::new());
        assert_eq!(panel_rect(&ctx), repertoires);
        // Lines.
        ui.open_lines(&settings, None);
        settle(&mut ui, &ctx, &mut settings, size);
        assert!(matches!(ui.screen, Screen::Lines(_)));
        assert_eq!(panel_rect(&ctx), repertoires);
        // A session started from Lines, and back to Lines.
        ui.start(SessionKind::ReviewAll, &settings);
        settle(&mut ui, &ctx, &mut settings, size);
        assert!(matches!(ui.screen, Screen::Session(_)));
        assert_eq!(panel_rect(&ctx), repertoires);
        frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Escape)]);
        frame(&mut ui, &ctx, &mut settings, size, Vec::new());
        assert!(matches!(ui.screen, Screen::Lines(_)));
        frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Escape)]);
        frame(&mut ui, &ctx, &mut settings, size, Vec::new());
        assert!(matches!(ui.screen, Screen::Repertoires));
        assert_eq!(panel_rect(&ctx), repertoires);
    }
}

#[test]
fn keys_of_the_screen_do_nothing_under_a_dialog() {
    let dir = TestDir::new("dialog-keys");
    save_library(&dir, &[("A", "1. e4 *"), ("B", "1. d4 *")]);
    let mut ui = dir.ui();
    let mut settings = Settings::default();
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    let size = vec2(1400.0, 900.0);
    settle(&mut ui, &ctx, &mut settings, size);
    let selected = ui.selected.clone();
    ui.dialog = Some(Dialog::New {
        name: String::new(),
        side: LearningSide::White,
        error: None,
    });
    frame(&mut ui, &ctx, &mut settings, size, Vec::new());
    let actions = frame(
        &mut ui,
        &ctx,
        &mut settings,
        size,
        vec![key(Key::ArrowDown)],
    );
    assert!(actions.is_empty());
    assert_eq!(ui.selected, selected);
    let actions = frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Escape)]);
    assert!(
        actions
            .iter()
            .all(|action| !matches!(action, RepertoireAction::Close)),
        "Esc closes the dialog, not the screen"
    );
    frame(&mut ui, &ctx, &mut settings, size, Vec::new());
    assert!(ui.dialog.is_none());
    let actions = frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Escape)]);
    assert!(matches!(actions.as_slice(), [RepertoireAction::Close]));
}

#[test]
fn names_are_checked_and_a_new_repertoire_keeps_its_colour() {
    let dir = TestDir::new("names");
    save_library(&dir, &[("Najdorf", "1. e4 c5 *")]);
    let mut ui = dir.ui();
    let mut settings = Settings::default();
    let mut actions = Vec::new();
    for (name, error) in [
        ("  ", tr("The repertoire name must not be empty")),
        ("najdorf", tr("A repertoire with this name already exists")),
    ] {
        ui.dialog = Some(Dialog::New {
            name: name.into(),
            side: LearningSide::Black,
            error: None,
        });
        ui.confirm_dialog(&mut settings, &mut actions);
        assert!(
            matches!(&ui.dialog, Some(Dialog::New { error: Some(e), .. }) if e == error),
            "{name:?}"
        );
    }
    ui.dialog = Some(Dialog::New {
        name: "Sveshnikov".into(),
        side: LearningSide::Black,
        error: None,
    });
    ui.confirm_dialog(&mut settings, &mut actions);
    assert!(ui.dialog.is_none());
    let id = ui.selected.clone().unwrap();
    assert_eq!(ui.library.repertoire(&id).unwrap().name, "Sveshnikov");
    assert_eq!(layout::side_of(&settings, &id), LearningSide::Black);
    assert!(matches!(
        actions.as_slice(),
        [RepertoireAction::SettingsChanged]
    ));
}

#[test]
fn delete_keeps_the_answers_and_selects_the_neighbour() {
    let dir = TestDir::new("delete");
    let ids = save_library(
        &dir,
        &[("A", "1. e4 *"), ("B", "1. d4 *"), ("C", "1. c4 *")],
    );
    let mut learning = LearningData::default();
    let library = dir.stores().0.load().unwrap();
    let scope = StatsScope::for_repertoire(ids[1].clone(), LearningSide::White, 0);
    let key = active_learning_keys(&library, &scope)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    learning.ensure_item(key.clone()).introduced = true;
    let data = LearningStoreData {
        learning,
        ..LearningStoreData::default()
    };
    std::fs::write(
        dir.0.join("repertoire-training.json"),
        serde_json::to_vec(&data).unwrap(),
    )
    .unwrap();
    let mut ui = dir.ui();
    ui.select(ids[1].clone());
    ui.dialog = Some(Dialog::Delete { id: ids[1].clone() });
    ui.delete_selected();
    assert!(ui.dialog.is_none());
    assert_eq!(ui.selected, Some(ids[2].clone()));
    assert!(ui.library.repertoire(&ids[1]).is_none());
    let kept = dir.stores().1.load().unwrap();
    assert!(kept.learning.item(&key).is_some_and(|item| item.introduced));
}

#[test]
fn a_session_needs_no_question_to_go_back_and_counts_refresh_after_it() {
    let dir = TestDir::new("session-back");
    save_library(&dir, &[("Scotch", "1. e4 e5 2. Nf3 Nc6 3. d4 *")]);
    let mut ui = dir.ui();
    let mut settings = Settings::default();
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    let size = vec2(1400.0, 900.0);
    settle(&mut ui, &ctx, &mut settings, size);
    let id = ui.selected.clone().unwrap();
    assert_eq!(ui.selected_card(), CardState::OnlyNew { new: 3 });
    frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Enter)]);
    settle(&mut ui, &ctx, &mut settings, size);
    assert!(matches!(ui.screen, Screen::Session(_)));
    ui.debug_moves("settle answer settle");
    frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Escape)]);
    frame(&mut ui, &ctx, &mut settings, size, vec![key(Key::Enter)]);
    settle(&mut ui, &ctx, &mut settings, size);
    assert!(matches!(ui.screen, Screen::Repertoires));
    // e4 was shown and played: it is due again today. Nf3 was on the board, unplayed,
    // when the session stopped: like d4 it is still new.
    let stats = ui.stats.get(&id).unwrap().as_ref().unwrap();
    assert_eq!((stats.unseen, stats.due), (2, 1));
}

#[test]
fn starter_repertoires_are_created_once_with_their_colours() {
    let dir = TestDir::new("starter");
    let (store, _) = dir.stores();
    let sides = seed_starter(&store).unwrap();
    let library = store.load().unwrap();
    assert_eq!(library.repertoires.len(), 4);
    assert_eq!(
        sides.iter().map(|(_, side)| *side).collect::<Vec<_>>(),
        [
            LearningSide::White,
            LearningSide::Black,
            LearningSide::White,
            LearningSide::Black
        ]
    );
    for (id, _) in &sides {
        let repertoire = library.repertoire(id).unwrap();
        assert!(!layout::lines(&repertoire.game(), |_| false).is_empty());
    }
    assert_eq!(
        library
            .repertoires
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        [
            "Spanish Opening",
            "Sicilian Defence: Najdorf",
            "Queen's Gambit",
            "Semi-Slav: Triangle"
        ]
    );

    let mut settings = Settings::default();
    let mut ui = dir.ui();
    ui.starter_sides = sides;
    egui::__run_test_ui(|egui_ui| {
        ui.show(egui_ui, &mut settings);
    });
    let black = &library.repertoires[1].id;
    assert_eq!(layout::side_of(&settings, black), LearningSide::Black);

    assert!(seed_starter(&store).unwrap().is_empty());
    assert_eq!(store.load().unwrap().repertoires.len(), 4);
}

#[test]
fn starter_repertoires_come_to_an_empty_library_of_2_3() {
    let dir = TestDir::new("starter-existing");
    let (store, _) = dir.stores();
    store.save(&mut Library::default()).unwrap();
    assert_eq!(seed_starter(&store).unwrap().len(), 4);
    assert_eq!(store.load().unwrap().repertoires.len(), 4);
}

#[test]
fn starter_examples_of_2_4_0_are_not_doubled() {
    let dir = TestDir::new("starter-240");
    // 2.4.0 created the examples in the interface language and kept no marks.
    save_library(
        &dir,
        &[
            ("Испанская партия", "1. e4 e5 *"),
            ("Sicilian Defence: Najdorf", "1. e4 c5 *"),
        ],
    );
    let (store, _) = dir.stores();
    let sides = seed_starter(&store).unwrap();
    assert_eq!(
        sides.iter().map(|(_, side)| *side).collect::<Vec<_>>(),
        [LearningSide::White, LearningSide::Black]
    );
    let names: Vec<_> = store
        .load()
        .unwrap()
        .repertoires
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(
        names,
        [
            "Испанская партия",
            "Sicilian Defence: Najdorf",
            "Queen's Gambit",
            "Semi-Slav: Triangle"
        ]
    );
    assert!(seed_starter(&store).unwrap().is_empty());
}

#[test]
fn a_deleted_starter_does_not_come_back() {
    let dir = TestDir::new("starter-deleted");
    let (store, _) = dir.stores();
    seed_starter(&store).unwrap();
    let mut library = store.load().unwrap();
    let id = library.repertoires[2].id.clone();
    library.delete(&id).unwrap();
    store.save(&mut library).unwrap();
    assert!(seed_starter(&store).unwrap().is_empty());
    let library = store.load().unwrap();
    assert_eq!(library.repertoires.len(), 3);
    assert!(
        library
            .repertoires
            .iter()
            .all(|r| r.name != "Queen's Gambit")
    );
}

#[test]
fn starters_leave_own_repertoires_and_progress_alone() {
    let dir = TestDir::new("starter-own");
    let ids = save_library(&dir, &[("Mine", "1. d4 d5 2. c4 e6 *")]);
    let (store, learning) = dir.stores();
    std::fs::write(learning.path(), b"progress bytes").unwrap();
    let before = serde_json::to_value(store.load().unwrap().repertoire(&ids[0]).unwrap()).unwrap();
    assert_eq!(seed_starter(&store).unwrap().len(), 4);
    let library = store.load().unwrap();
    assert_eq!(library.repertoires.len(), 5);
    assert_eq!(
        serde_json::to_value(library.repertoire(&ids[0]).unwrap()).unwrap(),
        before
    );
    assert_eq!(std::fs::read(learning.path()).unwrap(), b"progress bytes");
}

#[test]
fn starters_are_not_doubled_when_their_marks_were_lost() {
    let dir = TestDir::new("starter-lost-marks");
    let (store, _) = dir.stores();
    seed_starter(&store).unwrap();
    // A failed write of the marks, or marks from another copy that did not see them.
    std::fs::remove_file(starter_marks_path(&store)).unwrap();
    assert!(seed_starter(&store).unwrap().is_empty());
    std::fs::write(starter_marks_path(&store), br#"{"version":1,"given":[]}"#).unwrap();
    assert!(seed_starter(&store).unwrap().is_empty());
    assert_eq!(store.load().unwrap().repertoires.len(), 4);
}

#[test]
fn two_copies_starting_together_give_the_starters_once() {
    for round in 0..20 {
        let dir = TestDir::new(&format!("starter-race-{round}"));
        let (store, _) = dir.stores();
        let results: Vec<_> = std::thread::scope(|s| {
            let a = s.spawn(|| seed_starter(&store.clone()));
            let b = s.spawn(|| seed_starter(&store.clone()));
            vec![a.join().unwrap(), b.join().unwrap()]
        });
        let added: usize = results.iter().map(|r| r.as_ref().unwrap().len()).sum();
        assert_eq!(added, 4);
        assert_eq!(store.load().unwrap().repertoires.len(), 4);
    }
}

#[test]
fn damaged_starter_marks_give_a_short_message_and_change_nothing() {
    let dir = TestDir::new("starter-damaged");
    let (store, _) = dir.stores();
    store.save(&mut Library::default()).unwrap();
    let path = starter_marks_path(&store);
    std::fs::write(&path, br#"{"version":1,"given":["#).unwrap();
    let error = seed_starter(&store).unwrap_err();
    assert_eq!(
        error,
        "The record of example repertoires is damaged; examples were not added"
    );
    assert!(!error.contains(&*dir.0.to_string_lossy()));
    assert_eq!(std::fs::read(&path).unwrap(), br#"{"version":1,"given":["#);
    assert!(store.load().unwrap().repertoires.is_empty());
}

#[test]
fn starter_marks_of_an_unknown_version_are_left_alone() {
    let dir = TestDir::new("starter-newer");
    let (store, _) = dir.stores();
    store.save(&mut Library::default()).unwrap();
    let path = starter_marks_path(&store);
    std::fs::write(&path, br#"{"version":99,"given":5}"#).unwrap();
    assert!(seed_starter(&store).unwrap().is_empty());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        br#"{"version":99,"given":5}"#
    );
    assert!(store.load().unwrap().repertoires.is_empty());
}

#[test]
fn the_panel_opens_at_the_width_the_user_dragged_it_to() {
    let dir = TestDir::new("side-width");
    save_library(&dir, &[("Italian", "1. e4 e5 2. Nf3 Nc6 3. Bc4 *")]);
    let size = vec2(1920.0, 1080.0);
    for (kept, width) in [
        (Some(400.0_f32), 400.0),
        (Some(100.0), 360.0),
        (None, 560.0),
    ] {
        let mut ui = dir.ui();
        let mut settings = Settings::default();
        let ctx = egui::Context::default();
        crate::assets::install(&ctx);
        // As the app does: the saved width is given once the context runs.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        if let Some(kept) = kept {
            ctx.data_mut(|d| {
                d.insert_temp(egui::Id::new(crate::opening_layout::SIDE_WIDTH_ID), kept)
            });
        }
        settle(&mut ui, &ctx, &mut settings, size);
        let rect = panel_rect(&ctx);
        assert!((rect.width() - width).abs() <= 1.0, "{rect:?} for {kept:?}");
    }
}

#[test]
fn a_window_that_grows_gives_the_panel_the_width_of_its_final_size() {
    let dir = TestDir::new("side-grow");
    save_library(&dir, &[("Italian", "1. e4 e5 2. Nf3 Nc6 3. Bc4 *")]);
    let mut ui = dir.ui();
    let mut settings = Settings::default();
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    settle(&mut ui, &ctx, &mut settings, vec2(960.0, 600.0));
    settle(&mut ui, &ctx, &mut settings, vec2(1920.0, 1080.0));
    let rect = panel_rect(&ctx);
    assert!((rect.width() - 560.0).abs() <= 1.0, "{rect:?}");
}

#[test]
fn the_table_fits_the_least_panel_in_every_language() {
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    for lang in crate::i18n::Lang::ALL {
        crate::i18n::set_lang(lang);
        ctx.run_ui(egui::RawInput::default(), |root| {
            egui::CentralPanel::default().show(root, |ui| {
                // The table's width in a panel at its least: its frame takes 8 on each side.
                let width = crate::opening_layout::SIDE_LEAST - 16.0;
                let columns = table_widths(ui, width);
                let gap = ui.spacing().item_spacing.x;
                assert!(
                    columns.least(gap) <= width,
                    "{lang:?}: {columns:?} + gaps > {width}"
                );
                assert!(columns.name_least >= NAME_WANTED, "{lang:?}: {columns:?}");
                // The bar, 6 px and the number from the left fit for any count, the
                // header from the bar's left edge too; the bar keeps off «Today».
                let body = |text: &str| style_width(ui, text, egui::TextStyle::Body);
                for number in ["0/22", "12/22", "00/000"] {
                    assert!(
                        columns.start + columns.bar + MINI_BAR_GAP + body(number)
                            <= columns.passed - 4.0 + 0.5,
                        "{lang:?}: {number} in {columns:?}"
                    );
                }
                assert!(columns.start + body(tr("Passed")) <= columns.passed - 4.0 + 0.5);
                assert!(
                    // Unless the header itself takes the whole column.
                    columns.start + gap >= PASSED_GAP - 0.5 || columns.start < 1.0,
                    "{lang:?}: {columns:?}"
                );
            });
        })
        .drop_without_applying_deltas();
    }
    crate::i18n::set_lang(crate::i18n::Lang::En);
}

#[test]
fn a_review_date_is_short_in_the_table() {
    use chrono::{Local, TimeZone};
    let now = Local
        .with_ymd_and_hms(2026, 11, 27, 9, 0, 0)
        .single()
        .unwrap()
        .timestamp();
    assert_eq!(when_short(now + 3 * 60 * 60 + 30 * 60, now), "12:30");
    assert_eq!(when_short(now + 40 * 24 * 60 * 60, now), "06.01");
}

/// The comments of a PGN text: what stands in braces.
fn pgn_comments(pgn: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in pgn.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            c if depth > 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn cyrillic(text: &str) -> bool {
    text.chars()
        .any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap_or(c)))
}

#[test]
fn starter_comments_follow_the_interface_language() {
    for (lang, russian) in [
        (crate::i18n::Lang::Ru, true),
        (crate::i18n::Lang::En, false),
        (crate::i18n::Lang::De, false),
    ] {
        crate::i18n::set_lang(lang);
        let dir = TestDir::new(&format!("starter-lang-{lang:?}"));
        let (store, _) = dir.stores();
        assert_eq!(seed_starter(&store).unwrap().len(), 4);
        for repertoire in store.load().unwrap().repertoires {
            let comments = pgn_comments(&repertoire.pgn);
            assert!(!comments.trim().is_empty(), "{lang:?} {}", repertoire.name);
            assert_eq!(
                cyrillic(&comments),
                russian,
                "{lang:?} {}: {comments}",
                repertoire.name
            );
        }
    }
    crate::i18n::set_lang(crate::i18n::Lang::En);
}

#[test]
fn a_given_starter_keeps_its_comments_when_the_language_changes() {
    let dir = TestDir::new("starter-lang-kept");
    let (store, _) = dir.stores();
    crate::i18n::set_lang(crate::i18n::Lang::En);
    seed_starter(&store).unwrap();
    let before = std::fs::read(store.path()).unwrap();
    crate::i18n::set_lang(crate::i18n::Lang::Ru);
    assert!(seed_starter(&store).unwrap().is_empty());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    crate::i18n::set_lang(crate::i18n::Lang::En);
}

/// Every line of moves of a PGN, as UCI paths from the start, comments aside.
fn move_tree(pgn: &str) -> Vec<Vec<String>> {
    let cancel = AtomicBool::new(false);
    let db = PgnDatabase::from_bytes(pgn.as_bytes().to_vec(), None, &mut |_| {}, &cancel).unwrap();
    let mut lines = Vec::new();
    for index in 0..db.len() {
        let game = db.load_game(index).unwrap().game;
        let mut stack = vec![(NodeId::ROOT, Vec::new())];
        while let Some((node, path)) = stack.pop() {
            let children = game.children(node);
            if children.is_empty() {
                lines.push(path);
                continue;
            }
            for child in children {
                let mut next = path.clone();
                next.push(game.move_label(*child).unwrap_or_default());
                stack.push((*child, next));
            }
        }
    }
    lines.sort();
    lines
}

#[test]
fn russian_and_english_starters_have_the_same_moves() {
    for starter in &STARTER_REPERTOIRES {
        assert_eq!(
            move_tree(starter.pgn),
            move_tree(starter.pgn_ru),
            "{}",
            starter.key
        );
        assert!(!move_tree(starter.pgn).is_empty(), "{}", starter.key);
    }
}
