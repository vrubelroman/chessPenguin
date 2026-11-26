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
        tr("Play as Black")
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
    assert_eq!(stats.today(), 11);
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
    for (size, left) in [(vec2(1400.0, 900.0), 940.0), (vec2(960.0, 600.0), 576.0)] {
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
    assert_eq!(library.repertoires.len(), 2);
    assert_eq!(
        sides.iter().map(|(_, side)| *side).collect::<Vec<_>>(),
        [LearningSide::White, LearningSide::Black]
    );
    for ((id, _), lines) in sides.iter().zip([3, 3]) {
        let repertoire = library.repertoire(id).unwrap();
        assert_eq!(layout::lines(&repertoire.game(), |_| false).len(), lines);
    }
    assert_eq!(library.repertoires[0].name, "Spanish Opening");
    assert_eq!(library.repertoires[1].name, "Sicilian Defence: Najdorf");

    let mut settings = Settings::default();
    let mut ui = dir.ui();
    ui.starter_sides = sides;
    egui::__run_test_ui(|egui_ui| {
        ui.show(egui_ui, &mut settings);
    });
    let black = &library.repertoires[1].id;
    assert_eq!(layout::side_of(&settings, black), LearningSide::Black);

    assert!(seed_starter(&store).unwrap().is_empty());
    assert_eq!(store.load().unwrap().repertoires.len(), 2);
}

#[test]
fn starter_repertoires_do_not_return_to_an_existing_library() {
    let dir = TestDir::new("starter-existing");
    let (store, _) = dir.stores();
    store.save(&mut Library::default()).unwrap();
    assert!(seed_starter(&store).unwrap().is_empty());
    assert!(store.load().unwrap().repertoires.is_empty());
}
