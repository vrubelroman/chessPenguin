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
            RepertoireUi::with_stores(library_store, LearningStore::new(dir.join("learning.json")));
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

    fn aggregate(&mut self) -> ScopedLearningAggregate {
        for _ in 0..4 {
            if self.ui.selected_aggregate().is_some() {
                break;
            }
            let event = self.completed_job();
            self.deliver(event);
        }
        self.ui.selected_aggregate().cloned().expect("aggregate")
    }

    fn start(&mut self) {
        self.ui.side = LearningSide::White;
        self.aggregate();
        self.ui.start(&mut self.settings);
        assert!(self.ui.preparing);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn render_library(f: &mut Fixture, ctx: &egui::Context, width: f32, time: f64) {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            time: Some(time),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                f.ui.show_library(ui, &f.settings);
            });
        },
    )
    .drop_without_applying_deltas();
}

fn render_repertoire(
    f: &mut Fixture,
    ctx: &egui::Context,
    width: f32,
    time: f64,
    events: Vec<egui::Event>,
) -> Vec<egui::epaint::ClippedShape> {
    render_repertoire_frame(f, ctx, width, 700.0, time, events).1
}

fn render_repertoire_frame(
    f: &mut Fixture,
    ctx: &egui::Context,
    width: f32,
    height: f32,
    time: f64,
    events: Vec<egui::Event>,
) -> (Vec<RepertoireAction>, Vec<egui::epaint::ClippedShape>) {
    let mut actions = Vec::new();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, height),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                actions = f.ui.show(ui, &mut f.settings);
            });
        },
    );
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (actions, shapes)
}

fn click_events(position: egui::Pos2) -> Vec<egui::Event> {
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
}

fn text_click_position(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Pos2 {
    shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.pos + egui::vec2(8.0, 6.0))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing text {text:?}"))
}

fn assert_rect_inside(outer: egui::Rect, inner: egui::Rect) {
    assert!(inner.min.x >= outer.min.x, "{inner:?} outside {outer:?}");
    assert!(inner.min.y >= outer.min.y, "{inner:?} outside {outer:?}");
    assert!(inner.max.x <= outer.max.x, "{inner:?} outside {outer:?}");
    assert!(inner.max.y <= outer.max.y, "{inner:?} outside {outer:?}");
    assert!(inner.width() > 0.0, "empty rect: {inner:?}");
    assert!(inner.height() > 0.0, "empty rect: {inner:?}");
}

#[test]
fn dashboard_close_reloads_learning_and_preserves_repertoire_selection() {
    let mut f = Fixture::new();
    f.ui.side = LearningSide::Black;
    let ctx = egui::Context::default();
    let (_, shapes) = render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 0.0, Vec::new());
    let details = text_click_position(&shapes, "Details");

    let (actions, _) =
        render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 1.0, click_events(details));

    assert!(actions.is_empty());
    assert_eq!(
        f.ui.dashboard
            .as_ref()
            .and_then(LearningDashboardUi::selected_repertoire),
        Some(&f.id)
    );
    assert_eq!(
        f.ui.dashboard
            .as_ref()
            .map(LearningDashboardUi::selected_side),
        Some(LearningSide::Black)
    );
    f.ui.dashboard.as_mut().unwrap().wait_until_idle_for_test();
    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::Black, now());
    let key = active_learning_keys(&f.ui.library, &scope)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let mut data = f.ui.learning_store.load().unwrap();
    assert_eq!(data.snapshots.len(), 1);
    data.revision = 23;
    data.learning.ensure_item(key.clone()).introduced = true;
    std::fs::write(
        f.ui.learning_store.path(),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
    let (actions, shapes) = render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 2.0, Vec::new());
    assert!(actions.is_empty());
    let close = text_click_position(&shapes, "Close");
    f.ui.scroll_to_selected = false;

    let (actions, _) =
        render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 3.0, click_events(close));

    assert!(actions.is_empty());
    assert!(f.ui.dashboard.is_none());
    assert_eq!(f.ui.selected.as_ref(), Some(&f.id));
    assert_eq!(f.ui.learning.revision, 23);
    assert!(f.ui.learning.learning.item(&key).unwrap().introduced);
    assert!(!f.ui.scroll_to_selected);

    data.revision = 24;
    std::fs::write(
        f.ui.learning_store.path(),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
    let before_reopen = std::fs::read(f.ui.learning_store.path()).unwrap();
    f.ui.open_dashboard();
    assert!(f.ui.dashboard.is_some());
    f.ui.dashboard.as_mut().unwrap().wait_until_idle_for_test();
    assert_eq!(
        std::fs::read(f.ui.learning_store.path()).unwrap(),
        before_reopen
    );
    f.ui.scroll_to_selected = false;
    let (actions, _) = render_repertoire_frame(
        &mut f,
        &ctx,
        1400.0,
        900.0,
        4.0,
        vec![egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );

    assert!(actions.is_empty());
    assert!(f.ui.dashboard.is_none());
    assert_eq!(f.ui.selected.as_ref(), Some(&f.id));
    assert_eq!(f.ui.learning.revision, 24);
    assert!(!f.ui.scroll_to_selected);
}

#[test]
fn compact_statistics_names_total_and_explains_percentages() {
    let mut f = Fixture::new();
    f.aggregate();
    let ctx = egui::Context::default();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        },
        |ui| f.ui.statistics_panel(ui),
    );
    let has_text = |expected: &str| {
        output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(shape) if shape.galley.text().replace('\n', " ") == expected)
        })
    };
    assert!(has_text("Total"));
    assert!(has_text(
        "Learnt = Confident + Uncertain · percentages use Total."
    ));
    output.drop_without_applying_deltas();
}

#[test]
fn dashboard_does_not_open_over_editor_or_preparing_training() {
    let mut f = Fixture::new();
    f.ui.open_editor();

    f.ui.open_dashboard();
    assert!(f.ui.dashboard.is_none());

    f.ui.close_editor();
    f.ui.preparing = true;
    f.ui.open_dashboard();
    assert!(f.ui.dashboard.is_none());
}

#[test]
fn dashboard_uses_the_full_workspace_at_both_window_sizes() {
    for (width, height) in [(1400.0, 900.0), (640.0, 480.0)] {
        let mut f = Fixture::new();
        f.ui.open_dashboard();
        let ctx = egui::Context::default();
        let mut bounds = None;
        let mut actions = Vec::new();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let available = ui.available_rect_before_wrap();
                    actions = f.ui.show(ui, &mut f.settings);
                    bounds = Some((available, ui.min_rect()));
                });
            },
        );
        let dashboard_visible = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(shape) if shape.galley.text() == "Learning dashboard")
        });
        let repertoire_header_visible = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(shape) if shape.galley.text() == "Opening training")
        });
        output.drop_without_applying_deltas();

        assert!(actions.is_empty());
        assert!(dashboard_visible);
        assert!(!repertoire_header_visible);
        let (available, content) = bounds.unwrap();
        assert_rect_inside(available, content);
        assert!(f.ui.dashboard.is_some());
        assert_eq!(f.ui.selected.as_ref(), Some(&f.id));
        f.ui.dashboard.as_mut().unwrap().wait_until_idle_for_test();
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
fn editor_opens_in_the_full_workspace() {
    let mut f = Fixture::new();
    f.ui.open_editor();
    let ctx = egui::Context::default();

    render_repertoire(&mut f, &ctx, 1000.0, 0.0, Vec::new());

    let editor_rect = f.ui.debug_layout.editor.unwrap();
    assert!(editor_rect.width() > 900.0, "{editor_rect:?}");
    assert!(f.ui.editor.is_some());
    assert_eq!(f.ui.editor_id.as_ref(), Some(&f.id));
}

#[test]
fn applied_editor_library_is_adopted_without_a_second_save() {
    let mut f = Fixture::new();
    f.ui.open_editor();
    let mut applied = f.ui.library.clone();
    applied.repertoire_mut(&f.id).unwrap().set_max_plies(7);
    f.ui.library_store.save(&mut applied).unwrap();
    let before = std::fs::read(f.ui.library_store.path()).unwrap();
    let expected = applied.clone();
    let generation = f.ui.index_generation;

    f.ui.handle_editor_action(RepertoireEditorAction::Applied {
        repertoire_id: f.id.clone(),
        library: applied,
    });

    assert_eq!(
        serde_json::to_value(&f.ui.library).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(std::fs::read(f.ui.library_store.path()).unwrap(), before);
    assert!(f.ui.index_generation > generation);
    assert!(f.ui.editor.is_none());
    assert!(f.ui.editor_id.is_none());
}

#[test]
fn cancelling_editor_does_not_change_library_bytes() {
    let mut f = Fixture::new();
    f.ui.open_editor();
    let before = std::fs::read(f.ui.library_store.path()).unwrap();
    let library = f.ui.library.clone();

    f.ui.handle_editor_action(RepertoireEditorAction::Cancel);

    assert!(f.ui.editor.is_none());
    assert_eq!(
        serde_json::to_value(&f.ui.library).unwrap(),
        serde_json::to_value(&library).unwrap()
    );
    assert_eq!(std::fs::read(f.ui.library_store.path()).unwrap(), before);
}

#[test]
fn editor_draft_blocks_selection_start_and_unresolved_exit() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    let second = library
        .create("Other opening", &chess_core::parse_str("1. d4 d5 *").game)
        .unwrap();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());
    f.ui.select(f.id.clone(), &f.settings);
    f.ui.open_editor();
    let ctx = egui::Context::default();
    let shapes = render_repertoire(&mut f, &ctx, 1000.0, 0.0, Vec::new());
    let start = shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "20" => {
                Some(text.pos + egui::vec2(8.0, 6.0))
            }
            _ => None,
        })
        .expect("depth DragValue text");
    render_repertoire(
        &mut f,
        &ctx,
        1000.0,
        1.0,
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    render_repertoire(
        &mut f,
        &ctx,
        1000.0,
        2.0,
        vec![
            egui::Event::Text("7".to_owned()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert!(f.ui.has_unsaved(), "edit did not change the editor depth");
    let before = std::fs::read(f.ui.library_store.path()).unwrap();

    f.ui.select(second, &f.settings);
    assert_eq!(f.ui.selected.as_ref(), Some(&f.id));
    assert!(f.ui.editor.is_some());
    f.ui.start(&mut f.settings);
    assert!(!f.ui.preparing);
    assert!(f.ui.editor.is_some());
    f.ui.resolve_unsaved();
    assert!(f.ui.editor.is_some());
    assert!(!f.ui.training_visible);
    assert_eq!(std::fs::read(f.ui.library_store.path()).unwrap(), before);
}

#[test]
fn stale_applied_is_ignored_after_changing_repertoire() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    let second = library
        .create("Other opening", &chess_core::parse_str("1. d4 d5 *").game)
        .unwrap();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());
    f.ui.select(f.id.clone(), &f.settings);
    f.ui.open_editor();
    let current = f.ui.library.clone();
    let before = std::fs::read(f.ui.library_store.path()).unwrap();
    let mut stale = current.clone();
    stale.revision = current.revision.saturating_add(1);
    stale.repertoire_mut(&f.id).unwrap().set_max_plies(3);
    f.ui.select(second.clone(), &f.settings);

    f.ui.handle_editor_action(RepertoireEditorAction::Applied {
        repertoire_id: f.id.clone(),
        library: stale,
    });

    assert_eq!(f.ui.selected.as_ref(), Some(&second));
    assert_eq!(
        serde_json::to_value(&f.ui.library).unwrap(),
        serde_json::to_value(&current).unwrap()
    );
    assert_eq!(std::fs::read(f.ui.library_store.path()).unwrap(), before);
    assert!(f.ui.editor.is_none());
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
        let position = egui::pos2(200.0, 120.0);
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
fn long_save_error_and_import_cancel_stay_inside_the_fixed_status_row() {
    for width in [640.0_f32, 1400.0] {
        let mut f = Fixture::new();
        f.ui.side = LearningSide::White;
        let ctx = egui::Context::default();
        render_library(&mut f, &ctx, width, 0.0);
        let baseline = f.ui.debug_layout;
        let baseline_status = baseline.status.unwrap();
        let baseline_table = baseline.table.unwrap();
        assert_eq!(baseline_status.height(), 24.0);
        assert!(baseline_table.height() > 0.0);

        let path = f
            .dir
            .join("a-very-long-save-error-parent-directory-for-layout-regression")
            .join("another-long-directory-component-that-makes-the-error-wrap")
            .join("learning.json");
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let lock = path.with_file_name(format!("{file_name}.lock"));
        std::fs::create_dir_all(&lock).unwrap();
        let attempt = trainer::repertoire::TrainingAttempt {
            user_id: trainer::repertoire::LOCAL_PROFILE_ID.to_owned(),
            position_fen4: fen4(&chess_core::shakmaty::Chess::default()),
            client_attempt_id: "status-layout-error".to_owned(),
            session_id: "status-layout-session".to_owned(),
            event_type: trainer::repertoire::TrainingEventType::CorrectIndependent,
            expected_uci: "e2e4".to_owned(),
            actual_uci: Some("e2e4".to_owned()),
            response_time_ms: Some(1),
            happened_at: now(),
        };
        let error = LearningStore::new(&path)
            .apply_attempt(attempt, &SrsSettings::default())
            .unwrap_err()
            .to_string();
        assert!(error.len() > 160, "short fixture error: {error}");
        assert!(error.contains("learning.json.lock"), "{error}");
        f.ui.error = Some(error);
        f.ui.cancel = Some(Arc::new(AtomicBool::new(false)));

        render_library(&mut f, &ctx, width, 1.0);

        let with_error = f.ui.debug_layout;
        let status = with_error.status.unwrap();
        let message = with_error.message.unwrap();
        let retry = with_error.retry.unwrap();
        let cancel = with_error.cancel.unwrap();
        assert_eq!(status, baseline_status);
        assert_eq!(with_error.table, baseline.table);
        assert_rect_inside(status, message);
        assert_rect_inside(status, retry);
        assert_rect_inside(status, cancel);
        assert!(message.max.x <= cancel.min.x, "{message:?} {cancel:?}");
        assert!(cancel.max.x <= retry.min.x, "{cancel:?} {retry:?}");
        assert!(status.bottom() <= with_error.table.unwrap().top());
        assert_eq!(f.ui.side, LearningSide::White);
        assert!(f.ui.error.is_some());
    }
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

    assert_eq!(LibraryRow::height(), 156.0);
    assert_eq!(row(0).visible_openings().len(), 0);
    assert_eq!(row(3).visible_openings().len(), 3);
    assert_eq!(row(8).visible_openings().len(), OPENING_LINES);
}

#[test]
fn load_preserves_scope_and_main_answer_overrides() {
    let f = Fixture::new();
    let store = LibraryStore::new(f.dir.join("scoped-library.json"));
    let mut library = Library::default();
    let id = library
        .create("Scoped opening", &chess_core::parse_str("1. e4 e5 *").game)
        .unwrap();
    let repertoire = library.repertoire_mut(&id).unwrap();
    repertoire.max_plies = 1;
    repertoire
        .set_prefix_disabled(&["e2e4".to_owned()], true)
        .unwrap();
    let start = fen4(repertoire.game().start().position());
    repertoire
        .set_main_answer_override(&start, "e2e4", LearningSide::White)
        .unwrap();
    let pgn = repertoire.pgn.clone();
    let overrides = repertoire.main_answer_overrides.clone();
    store.save(&mut library).unwrap();

    let ui = RepertoireUi::with_stores(
        store.clone(),
        LearningStore::new(f.dir.join("scoped-learning.json")),
    );

    assert!(ui.library_error.is_none(), "{:?}", ui.library_error);
    let saved = store.load().unwrap();
    for library in [&ui.library, &saved] {
        let repertoire = library.repertoire(&id).unwrap();
        assert_eq!(repertoire.max_plies, 1);
        assert_eq!(repertoire.disabled_prefixes, [vec!["e2e4".to_owned()]]);
        assert_eq!(repertoire.main_answer_overrides, overrides);
        assert_eq!(repertoire.pgn, pgn);
    }
}

#[test]
fn persist_does_not_wipe_scope_or_main_answer_overrides() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    let repertoire = library.repertoire_mut(&f.id).unwrap();
    repertoire.max_plies = 1;
    repertoire
        .set_prefix_disabled(&["e2e4".to_owned()], true)
        .unwrap();
    let start = fen4(repertoire.game().start().position());
    repertoire
        .set_main_answer_override(&start, "e2e4", LearningSide::White)
        .unwrap();
    let pgn = repertoire.pgn.clone();
    let overrides = repertoire.main_answer_overrides.clone();

    assert!(f.ui.persist(library));

    let saved = f.ui.library_store.load().unwrap();
    for library in [&f.ui.library, &saved] {
        let repertoire = library.repertoire(&f.id).unwrap();
        assert_eq!(repertoire.max_plies, 1);
        assert_eq!(repertoire.disabled_prefixes, [vec!["e2e4".to_owned()]]);
        assert_eq!(repertoire.main_answer_overrides, overrides);
        assert_eq!(repertoire.pgn, pgn);
    }
    f.deliver(f.completed_job());
    assert_eq!(f.ui.variant_counts[&f.id], 1);
}

#[test]
fn aggregate_reports_mixed_learning_and_unseen_items() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    library.repertoire_mut(&f.id).unwrap().pgn =
        chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 3. Bb5 *")
            .game
            .to_pgn();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());

    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now());
    let keys = trainer::repertoire::active_learning_keys(&f.ui.library, &scope).unwrap();
    assert_eq!(keys.len(), 3);
    let at = now();
    let mut learning = trainer::repertoire::LearningData::default();
    for key in keys {
        match key.expected_uci.as_str() {
            "e2e4" => {
                let item = learning.ensure_item(key);
                item.introduced = true;
                item.correct_streak = 1;
                item.next_review_at = Some(at - 1);
            }
            "g1f3" => {
                let item = learning.ensure_item(key);
                item.introduced = true;
                item.correct_streak = 3;
                item.next_review_at = Some(at + 86_400);
                item.previous_interval_seconds = Some(86_400);
            }
            _ => {}
        }
    }
    f.ui.learning.learning = learning;
    f.ui.learning.revision += 1;

    let scoped = f.aggregate();
    let summary = &scoped.summary;
    assert_eq!(scoped.scope.side, LearningSide::White);
    assert_eq!(scoped.scope.repertoire_ids.len(), 1);
    assert!(scoped.scope.repertoire_ids.contains(&f.id));
    assert_eq!(summary.total, 3);
    assert_eq!(summary.unseen, 1);
    assert_eq!(summary.learning, 1);
    assert_eq!(summary.learnt, 1);
    assert_eq!(summary.uncertain, 0);
    assert_eq!(summary.due, 1);
    assert_eq!(summary.next_due_at, None);
    assert_eq!(summary.learnt_percent, Some(100.0 / 3.0));
    assert_eq!(summary.confident_percent, Some(100.0 / 3.0));
}

#[test]
fn migration_v2_is_read_only_and_old_runs_do_not_become_learning_kpis() {
    let mut f = Fixture::new();
    let path = f.dir.join("v2-learning.json");
    let mut legacy = trainer::repertoire::StatsData::default();
    legacy.runs.push(trainer::repertoire::RunStats {
        repertoire_id: f.id.clone(),
        mode: TrainingMode::White,
        completions: 42,
        last: Some(now()),
        recent: Vec::new(),
    });
    legacy.legacy_lines.insert(
        "legacy-line".to_owned(),
        serde_json::json!({"runs": 7, "unknown": [1, 2, 3]}),
    );
    trainer::repertoire::StatsStore::new(&path)
        .save(&mut legacy)
        .unwrap();
    let before = std::fs::read(&path).unwrap();

    f.ui = RepertoireUi::with_stores(f.ui.library_store.clone(), LearningStore::new(&path));

    assert!(f.ui.learning_error.is_none());
    let legacy = f.ui.learning.legacy_practice.as_ref().unwrap();
    assert_eq!(legacy["runs"][0]["completions"], 42);
    assert_eq!(legacy["legacy_lines"]["legacy-line"]["runs"], 7);
    assert!(f.ui.learning.learning.items.is_empty());
    let scoped = f.aggregate();
    assert_eq!(scoped.summary.total, 1);
    assert_eq!(scoped.summary.unseen, 1);
    assert_eq!(scoped.summary.learning, 0);
    assert_eq!(scoped.summary.accuracy_attempts, 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);
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
fn async_start_can_finish_a_real_opening_and_persist_learning() {
    let mut f = Fixture::new();
    f.start();
    let ready = f.completed_job();
    f.deliver(ready);
    assert!(f.ui.training_visible);

    f.ui.debug_moves("e2e4 settle");

    let training = f.ui.training.as_ref().unwrap();
    assert!(training.is_finished());
    assert!(!training.target_available());
    assert!(!training.has_unsaved());
    let saved = f.ui.learning_store.load().unwrap();
    assert_eq!(saved.learning.attempts.len(), 1);
    assert!(saved.learning.attempts[0].event_type.is_independent());
    assert_eq!(
        saved
            .learning
            .items
            .iter()
            .map(|item| item.correct_count)
            .sum::<u64>(),
        1
    );
    assert!(f.ui.error.is_none());
}

#[test]
fn unsaved_learning_blocks_start_and_resolve_restores_the_session() {
    let mut f = Fixture::new();
    f.start();
    let ready = f.completed_job();
    f.deliver(ready);
    let lock = f.dir.join("learning.json.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    f.ui.debug_moves("e2e4");
    assert!(f.ui.has_unsaved());

    let generation = f.ui.training_generation;
    f.ui.start(&mut f.settings);
    assert!(!f.ui.preparing);
    assert_eq!(f.ui.training_generation, generation);
    f.ui.open_library();
    assert!(!f.ui.training_visible);
    f.ui.resolve_unsaved();
    assert!(f.ui.training_visible);
}

#[test]
fn start_uses_the_srs_queue_instead_of_legacy_practice() {
    let mut f = Fixture::new();
    f.settings
        .repertoire_modes
        .insert(f.id.to_string(), (TrainingMode::White, QueueMode::Practice));
    f.ui.select(f.id.clone(), &f.settings);
    assert_eq!(f.ui.side, LearningSide::White);
    let summary = f.aggregate().summary.clone();
    assert_eq!(summary.due, 0);
    assert_eq!(summary.unseen, 1);

    f.start();

    assert_eq!(
        f.settings.repertoire_modes.get(f.id.as_str()),
        Some(&(TrainingMode::White, QueueMode::SpacedRepetition))
    );
    f.deliver(f.completed_job());
    assert!(f.ui.training_visible);
    let saved = f.ui.learning_store.load().unwrap();
    let session = saved.session_snapshots.last().unwrap();
    assert_eq!(session.session.mode, QueueMode::SpacedRepetition);
    assert_eq!(session.queue.len(), 1);
    assert_eq!(session.queue[0].key.expected_uci, "e2e4");
}

#[test]
fn review_mode_and_priority_are_saved_and_start_without_due_items() {
    let mut f = Fixture::new();
    f.settings.repertoire_modes.insert(
        f.id.to_string(),
        (TrainingMode::White, QueueMode::LeastRecent),
    );
    f.settings
        .repertoire_priorities
        .insert(f.id.to_string(), LearningPriority::CommonPositionsFirst);
    f.ui.select(f.id.clone(), &f.settings);
    assert_eq!(f.ui.queue, QueueMode::LeastRecent);
    assert_eq!(f.ui.priority, LearningPriority::CommonPositionsFirst);

    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now());
    let keys = trainer::repertoire::active_learning_keys(&f.ui.library, &scope).unwrap();
    let mut data = LearningStoreData::default();
    for key in keys {
        let item = data.learning.ensure_item(key);
        item.introduced = true;
        item.correct_streak = 3;
        item.next_review_at = Some(now() + 86_400);
        item.previous_interval_seconds = Some(86_400);
    }
    std::fs::write(
        f.ui.learning_store.path(),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
    f.ui.learning = f.ui.learning_store.load().unwrap();
    let summary = f.aggregate().summary.clone();
    assert_eq!(summary.due, 0);
    assert_eq!(summary.unseen, 0);

    f.start();

    assert_eq!(
        f.settings.repertoire_modes.get(f.id.as_str()),
        Some(&(TrainingMode::White, QueueMode::LeastRecent))
    );
    assert_eq!(
        f.settings.repertoire_priorities.get(f.id.as_str()),
        Some(&LearningPriority::CommonPositionsFirst)
    );
    f.deliver(f.completed_job());
    assert!(f.ui.training_visible);
    let saved = f.ui.learning_store.load().unwrap();
    let session = saved.session_snapshots.last().unwrap();
    assert_eq!(session.session.mode, QueueMode::LeastRecent);
    assert_eq!(
        session.stats_scope.learning_priority,
        LearningPriority::CommonPositionsFirst
    );
    assert_eq!(session.queue.len(), 1);
}

#[test]
fn shared_reset_previews_impact_and_cancel_is_a_no_op_before_confirm() {
    let mut f = Fixture::new();
    let mut library = f.ui.library.clone();
    library.repertoire_mut(&f.id).unwrap().pgn =
        chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 *").game.to_pgn();
    let second = library
        .create("Second shared", &chess_core::parse_str("1. e4 c5 *").game)
        .unwrap();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());
    let pgn_before =
        f.ui.library
            .repertoires
            .iter()
            .map(|repertoire| repertoire.pgn.clone())
            .collect::<Vec<_>>();

    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now());
    let keys = trainer::repertoire::active_learning_keys(&f.ui.library, &scope).unwrap();
    assert_eq!(keys.len(), 2);
    let mut data = LearningStoreData::default();
    for key in keys {
        let item = data.learning.ensure_item(key);
        item.introduced = true;
        item.correct_streak = 3;
        item.next_review_at = Some(now() + 86_400);
    }
    std::fs::write(
        f.ui.learning_store.path(),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
    f.ui.learning = f.ui.learning_store.load().unwrap();
    let before = std::fs::read(f.ui.learning_store.path()).unwrap();

    f.ui.begin_shared_reset(scope.clone());
    f.deliver(f.completed_job());
    let state = f.ui.shared_reset.as_ref().unwrap();
    let impact = state.impact.as_ref().unwrap();
    assert_eq!(impact.reset_count, 2);
    assert_eq!(impact.shared_count, 1);
    assert_eq!(
        impact
            .affected_repertoires
            .iter()
            .map(|repertoire| repertoire.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["Async opening", "Second shared"])
    );
    f.ui.cancel_shared_reset();
    assert!(f.ui.shared_reset.is_none());
    assert_eq!(std::fs::read(f.ui.learning_store.path()).unwrap(), before);
    assert!(matches!(f.ui.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

    f.ui.begin_shared_reset(scope.clone());
    f.deliver(f.completed_job());
    f.ui.confirm_shared_reset();
    assert!(!f.ui.shared_reset.as_ref().unwrap().busy);
    assert_eq!(std::fs::read(f.ui.learning_store.path()).unwrap(), before);
    assert!(matches!(f.ui.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

    f.ui.shared_reset.as_mut().unwrap().confirmed = true;
    f.ui.confirm_shared_reset();
    assert!(f.ui.shared_reset.as_ref().unwrap().busy);
    f.deliver(f.completed_job());
    assert!(f.ui.shared_reset.is_none());
    assert!(
        f.ui.learning
            .learning
            .items
            .iter()
            .all(|item| item.is_unseen())
    );
    let second_scope = StatsScope::for_repertoire(second, LearningSide::White, now());
    let second_summary = aggregate_scope(
        &f.ui.library,
        &f.ui.learning.learning,
        &second_scope,
        &SrsSettings::default(),
    )
    .unwrap();
    assert_eq!(second_summary.summary.unseen, 1);
    assert_eq!(f.aggregate().summary.unseen, 2);
    assert_eq!(
        f.ui.library
            .repertoires
            .iter()
            .map(|repertoire| repertoire.pgn.clone())
            .collect::<Vec<_>>(),
        pgn_before
    );

    f.ui.begin_shared_reset(scope);
    let stale = f.completed_job();
    f.ui.select(f.id.clone(), &f.settings);
    f.deliver(stale);
    assert!(f.ui.shared_reset.is_none());
    assert!(f.ui.error.is_none());
}

#[test]
fn review_order_reset_changes_only_the_cycle_cursor() {
    let mut f = Fixture::new();
    f.ui.library.repertoire_mut(&f.id).unwrap().pgn =
        chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 *").game.to_pgn();
    f.ui.queue = QueueMode::ReviewInOrder;
    f.ui.priority = LearningPriority::DepthFirst;
    f.start();
    f.deliver(f.completed_job());
    f.ui.debug_moves("e2e4 settle");
    f.ui.open_library();
    let before = f.ui.learning.learning.clone();
    let cursor = f.ui.review_snapshot().unwrap();
    assert_eq!(cursor.current_index, 1);
    assert_eq!(cursor.completed_keys.len(), 1);

    f.ui.reset_review_cursor();
    assert!(f.ui.review_reset_pending);
    f.deliver(f.completed_job());

    assert!(!f.ui.review_reset_pending);
    assert_eq!(f.ui.learning.learning, before);
    let cursor = f.ui.review_snapshot().unwrap();
    assert_eq!(cursor.current_index, 0);
    assert_eq!(cursor.cycle, 0);
    assert!(cursor.completed_keys.is_empty());
}

#[test]
fn review_reset_controls_keep_statistics_and_library_visible() {
    let mut f = Fixture::new();
    let mut settings = f.settings.clone();
    let mut actions = Vec::new();
    {
        let mut run = |queue| {
            f.ui.queue = queue;
            f.ui.debug_layout.reset_review = None;
            let ctx = egui::Context::default();
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        f.ui.show_workspace(ui, &mut settings, &mut actions);
                    });
                },
            )
            .drop_without_applying_deltas();
            let table = f.ui.debug_layout.table.expect("library table");
            assert!(
                table.top() < 480.0,
                "table is outside the viewport: {table:?}"
            );
            assert!(table.height() > 0.0);
            f.ui.debug_layout.reset_review.is_some()
        };

        assert!(run(QueueMode::ReviewInOrder));
        assert!(!run(QueueMode::LeastRecent));
    }
}

#[test]
fn due_start_has_priority_and_there_is_no_hidden_early_or_empty_start() {
    let mixed = LearningAggregate {
        total: 2,
        unseen: 1,
        due: 1,
        ..LearningAggregate::default()
    };
    assert_eq!(start_kind(&mixed), StartKind::Review);
    let introduce = LearningAggregate {
        total: 2,
        unseen: 1,
        learning: 1,
        ..LearningAggregate::default()
    };
    assert_eq!(start_kind(&introduce), StartKind::IntroduceNew);
    let future = LearningAggregate {
        total: 1,
        learnt: 1,
        learnt_percent: Some(100.0),
        confident_percent: Some(100.0),
        next_due_at: Some(now() + 86_400),
        ..LearningAggregate::default()
    };
    assert_eq!(start_kind(&future), StartKind::Unavailable);
    assert_eq!(
        start_kind(&LearningAggregate::default()),
        StartKind::Unavailable
    );
    assert_eq!(format_percent(None), "—");
}

#[test]
fn partial_srs_continue_has_priority_over_introduce_and_keeps_snapshot() {
    let mut f = Fixture::new();
    f.ui.library.repertoire_mut(&f.id).unwrap().pgn =
        chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *")
            .game
            .to_pgn();
    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now())
        .with_max_depth(Some(20));
    let keys = trainer::repertoire::active_learning_keys(&f.ui.library, &scope).unwrap();
    let key_for = |expected: &str| {
        keys.iter()
            .find(|key| key.expected_uci == expected)
            .unwrap()
            .clone()
    };
    let mut learning = trainer::repertoire::LearningData::default();
    for expected in ["e2e4", "g1f3"] {
        let item = learning.ensure_item(key_for(expected));
        item.introduced = true;
        item.next_review_at = Some(now());
    }
    let mut snapshot = LearningSessionSnapshot::new(
        "partial-srs-continue",
        &f.ui.library,
        &learning,
        scope,
        SrsSettings::default(),
    )
    .unwrap();
    let attempt = snapshot
        .play_uci(&f.ui.library, "e2e4", "partial-e4", now(), Some(100))
        .unwrap();
    let _ = learning
        .apply_attempt(attempt, &SrsSettings::default())
        .unwrap();
    let attempt = snapshot
        .advance_context(&f.ui.library, "partial-context", now())
        .unwrap();
    let _ = learning
        .apply_attempt(attempt, &SrsSettings::default())
        .unwrap();
    assert_eq!(snapshot.context_index, 1);
    assert_ne!(snapshot.phase, LearningSessionPhase::Finished);
    for item in &mut learning.items {
        item.next_review_at = Some(now() + 86_400);
    }
    std::fs::write(
        f.ui.learning_store.path(),
        serde_json::to_vec_pretty(&LearningStoreData {
            sessions: vec![snapshot.session.clone()],
            session_snapshots: vec![snapshot.clone()],
            learning,
            ..LearningStoreData::default()
        })
        .unwrap(),
    )
    .unwrap();
    f.ui.reload_learning();
    f.ui.aggregates.clear();

    let summary = f.aggregate().summary.clone();
    assert_eq!(summary.due, 0);
    assert!(summary.unseen > 0);
    assert_eq!(f.ui.selected_start_kind(&summary), StartKind::Review);
    let unfinished = f.ui.learning.session_snapshots[0].clone();
    f.ui.learning.session_snapshots[0].phase = LearningSessionPhase::Finished;
    assert_eq!(f.ui.selected_start_kind(&summary), StartKind::IntroduceNew);
    f.ui.learning.session_snapshots[0] = unfinished;

    let snapshot_count = f.ui.learning.session_snapshots.len();
    f.ui.start(&mut f.settings);
    assert!(f.ui.preparing);
    f.deliver(f.completed_job());

    let training = f.ui.training.as_ref().unwrap();
    assert_eq!(training.session_id(), "partial-srs-continue");
    let saved = f.ui.learning_store.load().unwrap();
    assert_eq!(saved.session_snapshots.len(), snapshot_count);
    assert_eq!(
        saved
            .session_snapshot("partial-srs-continue")
            .unwrap()
            .context_index,
        1
    );
}

#[test]
fn aggregate_reuses_pending_work_across_minutes_and_ignores_evicted_results() {
    let mut f = Fixture::new();
    let mut key = f.ui.current_aggregate_key().unwrap();
    key.minute -= 2;
    f.ui.aggregates.insert(key.clone(), None);

    f.ui.request_aggregate(f.ui.current_aggregate_key().unwrap());

    assert_eq!(f.ui.aggregates.len(), 1);
    assert_eq!(f.ui.aggregates.get(&key), Some(&None));
    assert!(matches!(f.ui.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now());
    let scoped = aggregate_scope(
        &f.ui.library,
        &f.ui.learning.learning,
        &scope,
        &SrsSettings::default(),
    )
    .unwrap();
    f.deliver(Event::Aggregated(key.clone(), Box::new(Ok(scoped))));
    assert!(matches!(
        f.ui.aggregates.get(&key),
        Some(Some(Ok(scoped))) if scoped.summary.total == 1
    ));
    f.ui.aggregates.clear();
    f.deliver(Event::Aggregated(
        key,
        Box::new(Err("stale failure".to_owned())),
    ));
    assert!(f.ui.aggregates.is_empty());
    assert!(f.ui.error.is_none());
}

#[test]
fn variant_count_is_pgn_routes_not_learning_total() {
    let mut f = Fixture::new();
    f.ui.library.repertoire_mut(&f.id).unwrap().pgn =
        chess_core::parse_str("1. e4 e5 (1... c5) *").game.to_pgn();
    let library = f.ui.library.clone();
    assert!(f.ui.persist(library));
    f.deliver(f.completed_job());
    assert_eq!(f.ui.variant_counts[&f.id], 2);
    assert_eq!(f.aggregate().summary.total, 1);

    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now());
    let keys = trainer::repertoire::active_learning_keys(&f.ui.library, &scope).unwrap();
    let mut learning = trainer::repertoire::LearningData::default();
    for key in keys {
        let item = learning.ensure_item(key);
        item.introduced = true;
        item.correct_streak = 3;
        item.next_review_at = Some(now() + 86_400);
    }
    f.ui.learning.learning = learning;
    f.ui.learning.revision += 1;

    assert_eq!(f.aggregate().summary.learnt, 1);
    assert_eq!(f.ui.variant_counts[&f.id], 2);
    assert!(matches!(f.ui.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn deleting_a_repertoire_preserves_global_learning() {
    let mut f = Fixture::new();
    let scope = StatsScope::for_repertoire(f.id.clone(), LearningSide::White, now());
    let key = trainer::repertoire::active_learning_keys(&f.ui.library, &scope)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let mut data = LearningStoreData {
        revision: 7,
        ..LearningStoreData::default()
    };
    let item = data.learning.ensure_item(key.clone());
    item.introduced = true;
    item.correct_streak = 2;
    std::fs::write(
        f.ui.learning_store.path(),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
    f.ui.learning = f.ui.learning_store.load().unwrap();
    let before = std::fs::read(f.ui.learning_store.path()).unwrap();

    assert!(f.ui.debug_delete(&f.settings));

    assert!(f.ui.library.repertoire(&f.id).is_none());
    assert_eq!(std::fs::read(f.ui.learning_store.path()).unwrap(), before);
    let saved = f.ui.learning_store.load().unwrap();
    assert!(saved.learning.item(&key).is_some());
    assert_eq!(saved.learning.items.len(), 1);
}

#[test]
fn importing_and_saving_a_repertoire_does_not_change_source_pgn() {
    let mut f = Fixture::new();
    let path = f.dir.join("source.pgn");
    let source = b"[Event \"Source\"]\n\n1. e4 e5 (1... c5) 2. Nf3 {keep} *\n";
    std::fs::write(&path, source).unwrap();
    let cancel = AtomicBool::new(false);

    let preview = read_import(&path, None, f.ui.library.clone(), &cancel).unwrap();
    assert!(f.ui.persist(preview.library));

    assert_eq!(std::fs::read(&path).unwrap(), source);
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
fn training_scope_uses_repertoire_depth_and_custom_context_policy() {
    let mut f = Fixture::new();
    let repertoire = f.ui.library.repertoire_mut(&f.id).unwrap();
    repertoire.pgn = chess_core::parse_str("1. e4 e5 2. Nf3 Nc6 *").game.to_pgn();
    repertoire.set_max_plies(2);
    let policy = trainer::repertoire::LearningContextPolicy {
        use_fixed_piece_speed: false,
        move_speed_adjustment: 0.25,
        consider_leadup_moves: true,
        ..Default::default()
    };
    f.settings
        .repertoire_context_policies
        .insert(f.id.to_string(), policy);

    let scoped = f.aggregate();
    assert_eq!(scoped.scope.max_depth, Some(2));
    assert_eq!(scoped.summary.total, 1);

    f.start();
    f.deliver(f.completed_job());

    let saved = f.ui.learning_store.load().unwrap();
    let session = saved.session_snapshots.last().unwrap();
    assert_eq!(session.stats_scope.max_depth, Some(2));
    assert_eq!(session.queue.len(), 1);
    assert_eq!(session.queue[0].key.expected_uci, "e2e4");
    assert_eq!(session.context_policy, policy);
    assert!(f.ui.matches_review_snapshot(session));
}

#[test]
fn context_control_changes_are_saved_from_the_workspace() {
    let mut f = Fixture::new();
    let ctx = egui::Context::default();
    let (_, shapes) = render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 0.0, Vec::new());
    let header = text_click_position(&shapes, "Context and move speed");
    let (_, shapes) =
        render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 1.0, click_events(header));
    let fixed = text_click_position(&shapes, "Use fixed piece movement speed");

    let (actions, _) =
        render_repertoire_frame(&mut f, &ctx, 1400.0, 900.0, 2.0, click_events(fixed));

    assert!(
        actions
            .iter()
            .any(|action| matches!(action, RepertoireAction::SettingsChanged))
    );
    let policy = f
        .settings
        .repertoire_context_policies
        .get(f.id.as_str())
        .unwrap();
    assert!(!policy.use_fixed_piece_speed);
}
