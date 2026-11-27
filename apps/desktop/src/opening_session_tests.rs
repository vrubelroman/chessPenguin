use std::path::PathBuf;

use super::*;
use chess_core::shakmaty::Position;
use trainer::repertoire::{LearningStoreData, active_learning_memberships};

#[test]
fn status_growth_does_not_move_training_controls() {
    let mut positions = Vec::new();
    for message in [
        "Your turn",
        "Hint\nReplay the shown move\nNo independent success",
    ] {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                layout::fixed_block(ui, 144.0, |ui| {
                    ui.label(message);
                });
                positions.push(ui.button("Hint").rect.top());
            });
        });
        output.drop_without_applying_deltas();
    }
    assert_eq!(positions[0], positions[1]);
    assert!(positions[0] >= 144.0);
}

const NOW: i64 = 1_800_000_000;
static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "cp-opening-session-{}-{}-{name}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn store(&self) -> LearningStore {
        LearningStore::new(self.0.join("learning.json"))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    _dir: TestDir,
    library: Library,
    scope: StatsScope,
    store: LearningStore,
    ui: OpeningSessionUi,
}

impl Fixture {
    /// A daily session asking exactly `due`: every other move of the side is known and
    /// not due.
    fn new(pgn: &str, side: LearningSide, due: &[&str], name: &str) -> Self {
        Self::with(pgn, side, due, &[], name, SessionKind::Daily)
    }

    /// `due` are due, `new` never shown, every other move known and not due.
    fn with(
        pgn: &str,
        side: LearningSide,
        due: &[&str],
        new: &[&str],
        name: &str,
        kind: SessionKind,
    ) -> Self {
        let dir = TestDir::new(name);
        let mut library = Library::default();
        let id = library
            .create("Test", &chess_core::parse_str(pgn).game)
            .unwrap();
        let scope = StatsScope::for_repertoire(id, side, NOW);
        let mut learning = LearningData::default();
        for membership in active_learning_memberships(&library, &scope).unwrap() {
            let uci = membership.key.expected_uci.as_str();
            if new.contains(&uci) {
                continue;
            }
            let item = learning.ensure_item(membership.key.clone());
            item.introduced = true;
            if due.contains(&uci) {
                item.next_review_at = Some(NOW);
            } else {
                item.correct_streak = 1;
                item.correct_count = 1;
                item.last_correct_at = Some(NOW - 24 * 60 * 60);
                item.next_review_at = Some(NOW + 10 * 24 * 60 * 60);
            }
        }
        let bytes = serde_json::to_vec_pretty(&LearningStoreData {
            learning,
            ..LearningStoreData::default()
        })
        .unwrap();
        std::fs::write(dir.0.join("learning.json"), bytes).unwrap();
        let store = dir.store();
        let mut ui =
            OpeningSessionUi::new(library.clone(), store.clone(), scope.clone(), kind).unwrap();
        ui.board_shown = true;
        ui.synchronous_commits = true;
        ui.feedback = Duration::ZERO;
        Self {
            _dir: dir,
            library,
            scope,
            store,
            ui,
        }
    }

    fn attempts(&self) -> Vec<TrainingAttempt> {
        self.store.load().unwrap().learning.attempts
    }
}

fn key_for(library: &Library, scope: &StatsScope, expected: &str) -> LearningItemKey {
    active_learning_memberships(library, scope)
        .unwrap()
        .into_iter()
        .find(|membership| membership.key.expected_uci == expected)
        .unwrap()
        .key
}

fn normal() -> Duration {
    Duration::from_millis(250)
}

fn answer_current(f: &mut Fixture, at: Instant, happened_at: i64, id: &str) {
    let expected = f.ui.session.current_expected_uci().unwrap().to_owned();
    assert!(f.ui.play_uci_with_id_at(&expected, id.to_owned(), at, happened_at,));
    tick_with_prime(&mut f.ui, at, Duration::ZERO, happened_at);
}

fn board_frame(f: &mut Fixture, ctx: &egui::Context, time: f64, settings: &Settings) {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 800.0),
            )),
            time: Some(time),
            ..Default::default()
        },
        |ui| f.ui.board_ui(ui, settings),
    )
    .drop_without_applying_deltas();
}

fn tick_with_prime(ui: &mut OpeningSessionUi, at: Instant, animation: Duration, happened_at: i64) {
    ui.tick_at(at, animation, happened_at);
    if ui.pending_automatic.is_none() {
        return;
    }
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    let settings = Settings::default();
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 800.0),
            )),
            time: Some(0.0),
            ..Default::default()
        },
        |root| ui.board_ui(root, &settings),
    )
    .drop_without_applying_deltas();
    assert!(ui.board_shown);
    assert!(ui.board.animation_at(0.0).is_none());
    ui.tick_at(at, animation, happened_at);
}

fn automatic_duration(f: &Fixture) -> Duration {
    match &f.ui.playback {
        Some(Playback::Automatic { duration, .. }) => *duration,
        _ => panic!("automatic playback expected"),
    }
}

fn assert_current_slide(
    f: &Fixture,
    time: f64,
    expected_duration: Duration,
    from: Square,
    to: Square,
) {
    let (duration, slides, remaining) =
        f.ui.board
            .animation_at(time)
            .expect("the current BoardView transition");
    assert_eq!(remaining, 1.0, "the first visual frame must be at from");
    assert!(
        (duration - f64::from(expected_duration.as_secs_f32())).abs() < 1e-6,
        "animation {duration} does not match scheduler {}",
        expected_duration.as_secs_f32()
    );
    assert!(
        slides
            .iter()
            .any(|(_, slide_from, slide_to)| *slide_from == from && *slide_to == to),
        "missing {from}{to} in {slides:?}"
    );
}

fn assert_adaptive_board_slides(f: &mut Fixture, first_ms: u128, last_ms: u128) {
    let ctx = egui::Context::default();
    let settings = Settings::default();
    let start = Instant::now();
    f.ui.board_shown = false;
    f.ui.tick_at(start, normal(), NOW);
    board_frame(f, &ctx, 0.0, &settings);
    assert!(f.ui.board.animation_at(0.0).is_none());

    f.ui.tick_at(start, normal(), NOW);
    let first = automatic_duration(f);
    assert_eq!(first.as_millis(), first_ms);
    assert_eq!(f.ui.playback_deadline(), Some(start + first));
    assert_eq!(f.ui.session.context_index, 0);
    board_frame(f, &ctx, 0.01, &settings);
    assert_current_slide(f, 0.01, first, Square::E2, Square::E4);
    assert_eq!(f.ui.state(), Some(State::Watch));

    tick_with_prime(&mut f.ui, start + first, normal(), NOW);
    let last = automatic_duration(f);
    assert_eq!(last.as_millis(), last_ms);
    assert_eq!(f.ui.session.context_index, 1);
    board_frame(f, &ctx, 0.02, &settings);
    assert_current_slide(f, 0.02, last, Square::E7, Square::E5);
    assert_eq!(f.ui.state(), Some(State::Watch));

    tick_with_prime(&mut f.ui, start + first + last, normal(), NOW);
    assert_eq!(f.ui.session.context_index, 2);
    assert!(f.ui.can_input());
    board_frame(f, &ctx, 10.0, &settings);
    assert!(
        f.ui.board.animation_at(10.0).is_none(),
        "the final position must not start a settings-duration slide"
    );
    assert_eq!(f.attempts().len(), 2);
}

#[test]
fn adaptive_board_animation_matches_actual_slides_and_starts_at_from() {
    let mut fast = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        &["g1f3"],
        "adaptive-fast",
    );
    let e4 = key_for(&fast.library, &fast.scope, "e2e4");
    fast.ui.learning.ensure_item(e4).correct_streak = 3;
    fast.ui.session.context_policy.use_fixed_piece_speed = false;
    fast.ui.session.context_policy.move_speed_adjustment = 1.5;
    assert_adaptive_board_slides(&mut fast, 83, 166);

    let mut slow = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        &["g1f3"],
        "adaptive-slow",
    );
    slow.ui.session.context_policy.use_fixed_piece_speed = false;
    slow.ui.session.context_policy.move_speed_adjustment = 0.0;
    assert_adaptive_board_slides(&mut slow, 1000, 1000);
}

#[test]
fn next_target_continues_the_shown_line_without_reverse_slide() {
    fn settle_to_input(f: &mut Fixture, at: Instant) {
        let mut at = at;
        for _ in 0..32 {
            if f.ui.can_input() {
                return;
            }
            let next = f.ui.playback_deadline().unwrap_or(at);
            tick_with_prime(&mut f.ui, next, normal(), NOW);
            at = next;
        }
        panic!("training did not reach independent input");
    }

    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
        LearningSide::Black,
        &["e7e5", "b8c6", "a7a6"],
        "next-target-prime",
    );
    f.ui.session.context_policy.use_fixed_piece_speed = false;
    f.ui.session.context_policy.move_speed_adjustment = 0.0;
    let start = Instant::now();
    settle_to_input(&mut f, start);
    assert_eq!(f.ui.session.current_expected_uci(), Some("e7e5"));
    answer_current(
        &mut f,
        start + Duration::from_secs(1),
        NOW,
        "first-target-e5",
    );

    settle_to_input(&mut f, start + Duration::from_secs(1) + normal());
    assert_eq!(f.ui.session.current_expected_uci(), Some("b8c6"));
    answer_current(
        &mut f,
        start + Duration::from_secs(3),
        NOW + 1,
        "second-target-nc6",
    );

    let prime_at = start + Duration::from_secs(3) + normal();
    // Every old move keeps its event, but a long visible prefix requires
    // only one store transaction before the next opponent move.
    let before_reuse = f.store.load().unwrap();
    for _ in 0..8 {
        f.ui.tick_at(prime_at, normal(), NOW);
        if f.ui.pending_automatic.is_some() {
            break;
        }
    }
    assert!(f.ui.pending_automatic.is_some());
    let after_reuse = f.store.load().unwrap();
    assert_eq!(after_reuse.revision, before_reuse.revision + 1);
    let added = &after_reuse.learning.attempts[before_reuse.learning.attempts.len()..];
    assert_eq!(added.len(), 4);
    assert!(
        added
            .iter()
            .all(|event| event.event_type == TrainingEventType::AutoContext)
    );
    assert_eq!(f.ui.session.context_index, 4);
    assert!(!f.ui.board_shown);
    let ctx = egui::Context::default();
    let settings = Settings::default();
    board_frame(&mut f, &ctx, 10.0, &settings);
    assert!(
        f.ui.board.animation_at(10.0).is_none(),
        "prime must not animate the previous target backwards"
    );

    f.ui.tick_at(prime_at, normal(), NOW);
    let duration = automatic_duration(&f);
    assert_eq!(duration, Duration::from_millis(1000));
    board_frame(&mut f, &ctx, 10.01, &settings);
    assert_current_slide(&f, 10.01, duration, Square::F1, Square::B5);
    let (_, slides, _) = f.ui.board.animation_at(10.01).unwrap();
    assert!(
        slides
            .iter()
            .all(|(_, from, to)| !(*from == Square::F3 && *to == Square::G1)),
        "next target started with a reverse slide: {slides:?}"
    );
    assert!(matches!(
        &f.ui.playback,
        Some(Playback::Automatic { remaining, .. }) if remaining.is_empty()
    ));
}

fn emitted_sounds(f: &mut Fixture, ctx: &egui::Context, settings: &Settings) -> Vec<Sound> {
    let mut sounds = Vec::new();
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(960.0, 600.0),
            )),
            ..Default::default()
        },
        |ui| {
            sounds.extend(
                f.ui.show(ui, settings, "Test")
                    .into_iter()
                    .filter_map(|action| {
                        if let SessionAction::Sound(sound) = action {
                            Some(sound)
                        } else {
                            None
                        }
                    }),
            );
        },
    )
    .drop_without_applying_deltas();
    sounds
}

#[test]
fn accepted_move_emits_once_and_next_task_only_animates_the_opponent_reply() {
    for side in [LearningSide::White, LearningSide::Black] {
        for animation in [
            crate::settings::Animation::Normal,
            crate::settings::Animation::Off,
        ] {
            for replay in [false, true] {
                let (due, reply, wrong) = match side {
                    LearningSide::White => (["e2e4", "g1f3"], "e7e5", "d2d4"),
                    LearningSide::Black => (["e7e5", "b8c6"], "g1f3", "d7d5"),
                };
                let mut f =
                    Fixture::new("1. e4 e5 2. Nf3 Nc6 *", side, &due, "continuous-feedback");
                tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
                let ctx = egui::Context::default();
                let settings = Settings {
                    animation,
                    ..Settings::default()
                };
                let at = Instant::now() + Duration::from_secs(60);
                let duration = Duration::from_secs_f32(animation.seconds());
                f.ui.animation = duration;
                f.ui.pending_sounds.clear();
                f.ui.visual_log.clear();
                if replay {
                    assert!(f.ui.play_uci_with_id_at(wrong, "wrong".into(), at - WRONG_SHOWN, NOW));
                    f.ui.tick_at(at, duration, NOW);
                    assert_eq!(emitted_sounds(&mut f, &ctx, &settings), [Sound::Wrong]);
                }
                assert!(f.ui.play_uci_with_id_at(due[0], "accepted".into(), at, NOW));
                let accepted_position = fen4(&f.ui.board_position);
                assert_eq!(emitted_sounds(&mut f, &ctx, &settings), [Sound::Move]);
                for _ in 0..8 {
                    f.ui.tick_at(at + duration, duration, NOW);
                    assert_eq!(
                        fen4(&f.ui.board_position),
                        accepted_position,
                        "accepted move must not rewind"
                    );
                    if f.ui.pending_automatic.is_some() {
                        break;
                    }
                }
                assert!(f.ui.pending_automatic.is_some());
                // The prime frame is silent; a separate later frame emits the
                // opponent move, so queue coalescing cannot conceal a duplicate.
                assert!(emitted_sounds(&mut f, &ctx, &settings).is_empty());
                assert_eq!(emitted_sounds(&mut f, &ctx, &settings), [Sound::Move]);
                assert_eq!(f.ui.visual_log, [reply]);
                f.ui.debug_settle();
                assert_eq!(f.ui.session.current_expected_uci(), Some(due[1]));
                let attempts = f.attempts();
                assert_eq!(
                    attempts
                        .iter()
                        .filter(|event| event.event_type.is_independent())
                        .count(),
                    1
                );
                assert!(attempts.iter().any(|event| event.expected_uci == due[0]
                    && event.event_type == TrainingEventType::AutoContext));
                assert_eq!(
                    attempts
                        .iter()
                        .filter(|event| event.event_type == TrainingEventType::ReplayAfterError)
                        .count(),
                    usize::from(replay)
                );
            }
        }
    }
}

#[test]
fn shown_context_requires_the_same_history_and_never_bypasses_a_question() {
    let mut f = Fixture::new(
        "1. Nf3 Nf6 2. g3 g6 3. Bg2 *",
        LearningSide::White,
        &["f1g2"],
        "context-identity",
    );
    let presentation =
        f.ui.session
            .presentation_with_learning(&f.library, &f.ui.learning)
            .unwrap();
    f.ui.board_position = parse_position(&presentation.automatic_context[3].to).unwrap();
    let id = f.ui.session.current_target().unwrap().repertoire_id.clone();
    // Even with an identical final position, a different move order must not
    // reuse context. A matching FEN alone cannot establish repetition history.
    f.ui.shown_prefix = Some(ShownPrefix {
        repertoire_id: id.clone(),
        path: vec!["g2g3".into(), "g7g6".into(), "g1f3".into(), "g8f6".into()],
    });
    assert!(!f.ui.reuse_shown_context(&presentation, Instant::now()));
    assert!(f.attempts().is_empty());
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        &["g1f3"],
        "context-question",
    );
    let presentation =
        f.ui.session
            .presentation_with_learning(&f.library, &f.ui.learning)
            .unwrap();
    f.ui.board_position = parse_position(&presentation.automatic_context[1].to).unwrap();
    f.ui.shown_prefix = Some(ShownPrefix {
        repertoire_id: f.ui.session.current_target().unwrap().repertoire_id.clone(),
        path: vec!["e2e4".into(), "e7e5".into()],
    });
    f.ui.session.context_policy.consider_leadup_moves = true;
    let question =
        f.ui.session
            .presentation_with_learning(&f.library, &f.ui.learning)
            .unwrap();
    assert_eq!(
        question.phase,
        LearningSessionPhase::AwaitingContextIndependent
    );
    assert!(!f.ui.reuse_shown_context(&question, Instant::now()));
    f.ui.prepare_presentation(Instant::now());
    assert!(f.ui.can_input());
    assert_eq!(f.ui.session.current_expected_uci(), Some("e2e4"));
    assert!(f.attempts().is_empty());
}

fn assert_annotation_badge(ui: &OpeningSessionUi, text: &str, square: Square) {
    let annotation = ui.displayed_annotations();
    let badge = annotation.badge.expect("PGN move badge");
    assert_eq!(badge.text, text);
    assert_eq!(badge.square, square);
}

#[test]
fn pgn_annotations_follow_visible_user_and_automatic_moves_not_the_next_task() {
    let mut f = Fixture::with(
        "1. e4!! {[%cal Re2e4,Ge7e5] [%csl Ye4,Re5]} e5?! {[%cal Bg1f3]} 2. Nf3! {[%csl Gf3]} Nc6 *",
        LearningSide::White,
        &["e2e4", "g1f3"],
        &[],
        "visible-pgn-annotations",
        SessionKind::ReviewAll,
    );
    let original =
        f.ui.library
            .repertoire(f.scope.repertoire_ids.iter().next().unwrap())
            .unwrap()
            .pgn
            .clone();
    let start = Instant::now();
    f.ui.tick_at(start, normal(), NOW);
    assert!(f.ui.displayed_annotations().badge.is_none());
    assert!(f.ui.displayed_annotations().arrows.is_empty());
    // A hint reveals the expected move, not that future node's annotations.
    assert!(f.ui.request_hint_with_id_at("annotation-hint".into(), start, NOW));
    assert!(f.ui.displayed_annotations().badge.is_none());
    assert!(f.ui.displayed_annotations().arrows.is_empty());
    assert!(f.ui.play_uci_with_id_at("e2e4", "annotation-e4".into(), start, NOW));
    assert_eq!(
        f.ui.session.current_target().unwrap().key.expected_uci,
        "g1f3"
    );
    assert_annotation_badge(&f.ui, "!!", Square::E4);
    let annotation = f.ui.displayed_annotations();
    assert_eq!(annotation.arrows.len(), 2);
    assert_eq!(annotation.circles.len(), 2);
    assert!(
        annotation
            .arrows
            .iter()
            .any(|arrow| arrow.from == Square::E2
                && arrow.to == Square::E4
                && arrow.color == crate::game_view::shape_color(chess_core::ShapeColor::Red))
    );
    // Verify the helper's data actually reaches the rendered BoardView.
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    let output = pointer_board_frame(&mut f, &ctx, 1.0, vec![], &Settings::default());
    assert!(output.shapes.iter().any(
        |shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "!!")
    ));
    output.drop_without_applying_deltas();
    f.ui.tick_at(start + normal(), normal(), NOW);
    assert_annotation_badge(&f.ui, "!!", Square::E4);
    assert!(f.ui.visual_log.is_empty(), "reused e4 must not play again");
    for _ in 0..8 {
        tick_with_prime(&mut f.ui, start + normal(), normal(), NOW);
        if matches!(f.ui.playback, Some(Playback::Automatic { .. })) {
            break;
        }
    }
    assert_eq!(f.ui.visual_log, ["e7e5"]);
    assert_annotation_badge(&f.ui, "?!", Square::E5);
    assert_eq!(f.ui.displayed_annotations().arrows[0].from, Square::G1);
    f.ui.tick_at(f.ui.playback_deadline().unwrap(), normal(), NOW);
    assert!(f.ui.can_input());
    assert_annotation_badge(&f.ui, "?!", Square::E5);
    assert!(
        f.ui.displayed_annotations().circles.is_empty(),
        "future Nf3 circle leaked before the answer"
    );
    assert!(f.ui.play_uci_with_id_at(
        "g1f3",
        "annotation-nf3".into(),
        start + Duration::from_secs(1),
        NOW
    ));
    assert_annotation_badge(&f.ui, "!", Square::F3);
    assert_eq!(f.ui.displayed_annotations().circles[0].0, Square::F3);
    f.ui.debug_settle();
    assert_annotation_badge(&f.ui, "!", Square::F3);
    assert_eq!(
        f.ui.library
            .repertoire(f.scope.repertoire_ids.iter().next().unwrap())
            .unwrap()
            .pgn,
        original
    );
}

#[test]
fn pgn_annotations_clear_for_wrong_moves_and_survive_failed_async_save_and_retry() {
    let mut f = Fixture::new(
        "1. e4!! {[%csl Ge4]} *",
        LearningSide::White,
        &["e2e4"],
        "annotations-retry",
    );
    let start = Instant::now();
    f.ui.tick_at(start, normal(), NOW);
    assert!(f.ui.play_uci_with_id_at("d2d4", "annotations-wrong".into(), start, NOW));
    assert!(f.ui.displayed_annotations().badge.is_none());
    assert!(f.ui.displayed_annotations().circles.is_empty());
    f.ui.tick_at(start + WRONG_SHOWN, normal(), NOW);
    assert!(f.ui.can_input());
    assert!(f.ui.displayed_annotations().badge.is_none());
    // Another wrong move during replay must not inherit the revealed e4 badge.
    assert!(f.ui.play_uci_with_id_at(
        "d2d4",
        "annotations-wrong-replay".into(),
        start + WRONG_SHOWN,
        NOW
    ));
    assert!(f.ui.displayed_annotations().badge.is_none());
    f.ui.tick_at(start + WRONG_SHOWN * 2, normal(), NOW);
    let store = f.ui.store.clone();
    f.ui.synchronous_commits = false;
    f.ui.store = LearningStore::with_legacy(store.path(), store.path());
    assert!(f.ui.play_uci_with_id_at(
        "e2e4",
        "annotations-replay".into(),
        start + WRONG_SHOWN * 2,
        NOW
    ));
    assert!(f.ui.has_unsaved());
    assert_annotation_badge(&f.ui, "!!", Square::E4);
    wait_for_commit(&mut f.ui);
    assert!(f.ui.save_error.is_some());
    assert_annotation_badge(&f.ui, "!!", Square::E4);
    f.ui.store = store;
    f.ui.retry_pending(Instant::now());
    wait_for_commit(&mut f.ui);
    assert_annotation_badge(&f.ui, "!!", Square::E4);
    assert_eq!(
        f.attempts()
            .iter()
            .filter(|attempt| attempt.event_type.is_independent())
            .count(),
        1
    );
    assert_eq!(
        f.attempts().last().unwrap().event_type,
        TrainingEventType::ReplayAfterError
    );
}

#[test]
fn alternative_answer_credits_actual_move_and_completed_answer_is_silent_and_neutral() {
    let mut f = Fixture::new(
        "1. e4! (1. d4!! {[%csl Gd4]}) *",
        LearningSide::White,
        &["e2e4", "d2d4"],
        "alternative-answers",
    );
    let start = Instant::now();
    f.ui.tick_at(start, normal(), NOW);
    let original = f.ui.session.current_expected_uci().unwrap().to_owned();
    let (played, badge, square) = if original == "e2e4" {
        ("d2d4", "!!", Square::D4)
    } else {
        ("e2e4", "!", Square::E4)
    };
    let initial_position = f.ui.board_position.clone();
    assert_eq!(
        f.ui.session
            .pending_answers_for_position(&fen4(&initial_position))
            .len(),
        2
    );
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    assert_eq!(f.ui.state(), Some(State::Yours));
    assert!(f.ui.play_uci_with_id_at(played, "actual-alternative".into(), start, NOW));
    assert_annotation_badge(&f.ui, badge, square);
    assert_eq!(f.attempts().len(), 1);
    assert_eq!(f.attempts()[0].expected_uci, played);
    assert_eq!(
        f.attempts()[0].event_type,
        TrainingEventType::CorrectIndependent
    );
    assert_eq!(f.ui.session.current_expected_uci(), Some(original.as_str()));
    assert_eq!(std::mem::take(&mut f.ui.pending_sounds), [Sound::Move]);
    f.ui.tick_at(start + normal(), normal(), NOW);
    assert_eq!(f.ui.board_position, initial_position);
    // S3a: the other answer of this position is still asked.
    let played_san = if played == "d2d4" { "d4" } else { "e4" };
    assert_eq!(f.ui.state(), Some(State::YoursMore(played_san.to_owned())));
    assert!(f.ui.play_uci_with_id_at(played, "repeat-alternative".into(), start + normal(), NOW));
    assert!(matches!(f.ui.playback, Some(Playback::Alternative { .. })));
    assert_eq!(f.ui.state(), Some(State::Alternative));
    assert_annotation_badge(&f.ui, badge, square);
    assert!(f.ui.save_error.is_none() && f.ui.error.is_none());
    assert!(emitted_sounds(&mut f, &ctx, &Settings::default()).is_empty());
    f.ui.tick_at(start + normal() + WRONG_SHOWN, normal(), NOW);
    assert_eq!(f.ui.board_position, initial_position);
    assert!(f.ui.can_input());
    assert!(f.ui.displayed_annotations().badge.is_none());
    // «Now the other one: play …» shows the answer: recorded as shown, not as an
    // error and not as known.
    let attempts = f.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].event_type, TrainingEventType::Previewed);
    assert_eq!(attempts[1].expected_uci, original);
    assert_eq!(f.ui.state(), Some(State::Alternative));
    assert_eq!(f.ui.revealed_uci().as_deref(), Some(original.as_str()));
    assert_eq!(f.ui.tally.mistakes, 0);
    assert_eq!(f.ui.session.remaining(), 1);
    assert!(emitted_sounds(&mut f, &ctx, &Settings::default()).is_empty());
}

#[test]
fn move_input_uses_the_startup_answer_snapshot_in_independent_and_replay_phases() {
    let mut f = Fixture::new(
        "1. e4 (1. d4) *",
        LearningSide::White,
        &["e2e4"],
        "cached-repertoire-answers",
    );
    let start = Instant::now();
    f.ui.tick_at(start, normal(), NOW);
    let key = key_for(&f.library, &f.scope, "d2d4");
    assert!(f.ui.repertoire_answers.contains(&key));
    // The production library is immutable for the UI's lifetime. Change it
    // only here to detect any accidental rebuilding in the input hot path.
    let id = f.scope.repertoire_ids.iter().next().unwrap();
    f.ui.library
        .repertoire_mut(id)
        .unwrap()
        .set_prefix_disabled(&["d2d4".to_owned()], true)
        .unwrap();
    assert!(
        !active_learning_keys(&f.ui.library, &f.scope)
            .unwrap()
            .contains(&key)
    );
    assert!(f.ui.play_uci_with_id_at("d2d4", "cached-independent".into(), start, NOW));
    assert!(matches!(f.ui.playback, Some(Playback::Alternative { .. })));
    assert!(f.attempts().is_empty());
    f.ui.tick_at(start + WRONG_SHOWN, normal(), NOW);
    // The answer was revealed after the other move: no second preview.
    assert_eq!(f.ui.session.phase, LearningSessionPhase::PreviewedReplay);
    assert!(
        !f.ui
            .request_hint_with_id_at("cached-preview".into(), start + WRONG_SHOWN, NOW)
    );
    assert!(f.ui.play_uci_with_id_at("d2d4", "cached-replay".into(), start + WRONG_SHOWN, NOW));
    assert!(matches!(f.ui.playback, Some(Playback::Alternative { .. })));
    assert_eq!(f.attempts().len(), 1);
    assert_eq!(f.attempts()[0].event_type, TrainingEventType::Previewed);
    assert!(f.ui.pending_sounds.is_empty());
}

#[test]
fn known_unscheduled_answer_during_hint_is_neutral_but_outside_move_still_wrong() {
    let mut f = Fixture::new(
        "1. e4! (1. d4!! {[%csl Gd4]}) *",
        LearningSide::White,
        &["e2e4"],
        "hint-alternative",
    );
    let start = Instant::now();
    f.ui.tick_at(start, normal(), NOW);
    assert!(f.ui.request_hint_with_id_at("preview-alternative".into(), start, NOW));
    let attempts = f.attempts().len();
    let snapshot = f.ui.session.clone();
    assert!(f.ui.play_uci_with_id_at("d2d4", "unscheduled-alternative".into(), start, NOW));
    assert!(matches!(f.ui.playback, Some(Playback::Alternative { .. })));
    assert_annotation_badge(&f.ui, "!!", Square::D4);
    assert_eq!(f.ui.session, snapshot);
    assert!(f.ui.pending_sounds.is_empty());
    f.ui.tick_at(start + WRONG_SHOWN, normal(), NOW);
    assert_eq!(f.attempts().len(), attempts);
    assert!(f.ui.can_input());
    assert_eq!(f.ui.state(), Some(State::Alternative));
    assert!(f.ui.play_uci_with_id_at(
        "g1f3",
        "outside-repertoire".into(),
        start + WRONG_SHOWN,
        NOW
    ));
    assert!(matches!(f.ui.playback, Some(Playback::Wrong { .. })));
    assert_eq!(f.ui.pending_sounds, [Sound::Wrong]);
    assert!(f.ui.displayed_annotations().badge.is_none());
    assert_eq!(f.attempts().len(), attempts);
}

#[test]
fn authored_wrong_branch_has_its_own_annotations_only_after_it_is_played() {
    let mut f = Fixture::new(
        "1. e4! (1. d4?? {[%csl Rd4]}) e5 *",
        LearningSide::White,
        &["e2e4"],
        "wrong-branch-annotations",
    );
    let start = Instant::now();
    f.ui.tick_at(start, normal(), NOW);
    assert!(f.ui.displayed_annotations().badge.is_none());
    assert!(f.ui.play_uci_with_id_at("d2d4", "authored-wrong-branch".into(), start, NOW));
    assert_annotation_badge(&f.ui, "??", Square::D4);
    assert_eq!(
        f.ui.displayed_annotations().circles[0],
        (
            Square::D4,
            crate::game_view::shape_color(chess_core::ShapeColor::Red)
        )
    );
    f.ui.tick_at(start + WRONG_SHOWN, normal(), NOW);
    assert!(f.ui.displayed_annotations().badge.is_none());
    assert!(f.ui.displayed_annotations().circles.is_empty());
}

#[test]
fn pgn_annotation_cursor_distinguishes_transposed_paths_and_repertoire_sources() {
    let mut f = Fixture::new(
        "1. Nf3 (1. g3 g6 2. Nf3 Nf6? {[%csl Rf6]} 3. Bg2) Nf6 2. g3 g6! {[%csl Gg6]} 3. Bg2 *",
        LearningSide::White,
        &["f1g2"],
        "annotations-transposition",
    );
    let id = f.ui.session.current_target().unwrap().repertoire_id.clone();
    let mut position_keys = Vec::new();
    for (path, badge, square) in [
        (["g1f3", "g8f6", "g2g3", "g7g6"], "!", Square::G6),
        (["g2g3", "g7g6", "g1f3", "g8f6"], "?", Square::F6),
    ] {
        f.ui.set_displayed_path(Some(&ShownPrefix {
            repertoire_id: id.clone(),
            path: path.map(str::to_owned).to_vec(),
        }));
        let (_, node) = f.ui.displayed_node.as_ref().expect("exact PGN branch");
        f.ui.board_position = f.ui.annotation_games[&id].position(*node).unwrap().clone();
        position_keys.push(fen4(&f.ui.board_position));
        assert_annotation_badge(&f.ui, badge, square);
        assert_eq!(f.ui.displayed_annotations().circles[0].0, square);
    }
    assert_eq!(
        position_keys[0], position_keys[1],
        "fixture must really transpose"
    );
    let other_id =
        f.ui.library
            .create("Other annotations", &Game::new())
            .unwrap();
    f.ui.annotation_games.insert(
        other_id.clone(),
        chess_core::parse_str("1. Nf3 Nf6 2. g3 g6?? {[%csl Yg6]} *").game,
    );
    f.ui.set_displayed_path(Some(&ShownPrefix {
        repertoire_id: other_id.clone(),
        path: ["g1f3", "g8f6", "g2g3", "g7g6"].map(str::to_owned).to_vec(),
    }));
    let (_, node) = f.ui.displayed_node.as_ref().unwrap();
    f.ui.board_position = f.ui.annotation_games[&other_id]
        .position(*node)
        .unwrap()
        .clone();
    assert_annotation_badge(&f.ui, "??", Square::G6);
}

#[test]
fn debug_moves_settles_prime_between_targets() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
        LearningSide::Black,
        &["e7e5", "b8c6", "a7a6"],
        "debug-prime-between-targets",
    );
    f.ui.debug_moves("e7e5 settle b8c6 settle");
    assert!(f.ui.can_input());
    assert_eq!(f.ui.session.current_expected_uci(), Some("a7a6"));
    f.ui.debug_moves("a7a6 settle");
    assert!(f.ui.is_finished());
    assert_eq!(
        f.attempts()
            .iter()
            .filter(|attempt| attempt.event_type.is_independent())
            .count(),
        3
    );
}

#[test]
fn automatic_context_has_three_separate_normal_timed_steps() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::Black,
        &["b8c6"],
        "context-normal",
    );
    let start = Instant::now();
    tick_with_prime(&mut f.ui, start, normal(), NOW);
    assert_eq!(f.ui.visual_log, ["e2e4"]);
    assert_eq!(f.ui.session.context_index, 0);
    assert!(!f.ui.can_input());

    tick_with_prime(&mut f.ui, start + Duration::from_millis(249), normal(), NOW);
    assert_eq!(f.ui.session.context_index, 0);
    assert!(f.attempts().is_empty());

    tick_with_prime(&mut f.ui, start + normal(), normal(), NOW);
    assert_eq!(f.ui.session.context_index, 1);
    assert_eq!(f.ui.visual_log, ["e2e4", "e7e5"]);
    tick_with_prime(&mut f.ui, start + normal(), normal(), NOW);
    assert_eq!(f.ui.session.context_index, 1);

    tick_with_prime(&mut f.ui, start + normal() * 2, normal(), NOW);
    assert_eq!(f.ui.session.context_index, 2);
    assert_eq!(f.ui.visual_log, ["e2e4", "e7e5", "g1f3"]);
    tick_with_prime(&mut f.ui, start + normal() * 3, normal(), NOW);
    assert_eq!(f.ui.session.context_index, 3);
    assert_eq!(
        f.ui.session.phase,
        LearningSessionPhase::AwaitingIndependent
    );
    assert!(f.ui.can_input());
    assert_eq!(f.attempts().len(), 3);
    assert_eq!(
        trainer::repertoire::fen4(&f.ui.board_position),
        key_for(&f.library, &f.scope, "b8c6").position_fen4
    );
}

#[test]
fn animation_off_commits_every_context_step_without_skipping_store_calls() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::Black,
        &["b8c6"],
        "context-off",
    );
    tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
    assert_eq!(f.ui.visual_log, ["e2e4", "e7e5", "g1f3"]);
    assert_eq!(f.ui.session.context_index, 3);
    assert!(f.ui.can_input());
    let attempts = f.attempts();
    assert_eq!(attempts.len(), 3);
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt.actual_uci.as_deref())
            .collect::<Vec<_>>(),
        [Some("e2e4"), Some("e7e5"), Some("g1f3")]
    );
    assert!(attempts.iter().all(|attempt| {
        attempt.event_type == TrainingEventType::AutoContext && attempt.response_time_ms.is_none()
    }));
}

#[test]
fn legal_input_is_ignored_during_autoplay() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::Black,
        &["b8c6"],
        "autoplay-input",
    );
    let start = Instant::now();
    tick_with_prime(&mut f.ui, start, normal(), NOW);
    assert!(!f.ui.play_uci_with_id_at(
        "e7e5",
        "blocked-input".to_owned(),
        start + Duration::from_millis(20),
        NOW,
    ));
    assert_eq!(f.ui.session.context_index, 0);
    assert!(f.attempts().is_empty());
    assert_eq!(f.ui.visual_log, ["e2e4"]);
}

#[test]
fn wrong_answer_replays_without_a_second_independent_error() {
    let mut f = Fixture::with(
        "1. e4 *",
        LearningSide::White,
        &["e2e4"],
        &[],
        "wrong-replay",
        SessionKind::ReviewAll,
    );
    let start = Instant::now();
    tick_with_prime(&mut f.ui, start, normal(), NOW);
    assert!(f.ui.play_uci_with_id_at(
        "d2d4",
        "wrong-d4".to_owned(),
        start + Duration::from_millis(1500),
        NOW,
    ));
    assert!(matches!(f.ui.playback, Some(Playback::Wrong { .. })));
    tick_with_prime(
        &mut f.ui,
        start + Duration::from_millis(1500) + WRONG_SHOWN,
        normal(),
        NOW,
    );
    assert_eq!(
        f.ui.session.phase,
        LearningSessionPhase::AwaitingReplayAfterError
    );
    assert_eq!(
        f.ui.presentation.as_ref().unwrap().revealed_move.as_deref(),
        Some("e2e4")
    );

    assert!(f.ui.play_uci_with_id_at(
        "d2d4",
        "wrong-replay-d4".to_owned(),
        start + Duration::from_secs(3),
        NOW,
    ));
    assert_eq!(f.attempts().len(), 1);
    tick_with_prime(
        &mut f.ui,
        start + Duration::from_secs(3) + WRONG_SHOWN,
        normal(),
        NOW,
    );
    assert!(f.ui.play_uci_with_id_at(
        "e2e4",
        "replay-e4".to_owned(),
        start + Duration::from_secs(4),
        NOW,
    ));
    tick_with_prime(
        &mut f.ui,
        start + Duration::from_secs(4) + normal(),
        normal(),
        NOW,
    );
    assert!(f.ui.is_finished());
    let attempts = f.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0].event_type,
        TrainingEventType::IncorrectIndependent
    );
    assert_eq!(attempts[0].response_time_ms, Some(1500));
    assert_eq!(attempts[1].event_type, TrainingEventType::ReplayAfterError);
    assert_eq!(attempts[1].response_time_ms, None);
    let item = f
        .store
        .load()
        .unwrap()
        .learning
        .item(&key_for(&f.library, &f.scope, "e2e4"))
        .unwrap()
        .clone();
    assert_eq!(item.wrong_count, 1);
    assert_eq!(item.correct_count, 0);
}

#[test]
fn preview_is_idempotent_and_shown_move_is_not_success() {
    let mut f = Fixture::with(
        "1. e4 *",
        LearningSide::White,
        &["e2e4"],
        &[],
        "preview",
        SessionKind::ReviewAll,
    );
    let start = Instant::now();
    tick_with_prime(&mut f.ui, start, normal(), NOW);
    assert!(f.ui.request_hint_with_id_at(
        "preview-e4".to_owned(),
        start + Duration::from_millis(100),
        NOW,
    ));
    assert_eq!(f.ui.session.phase, LearningSessionPhase::PreviewedReplay);
    assert!(!f.ui.request_hint_with_id_at(
        "preview-e4-again".to_owned(),
        start + Duration::from_millis(200),
        NOW,
    ));
    assert_eq!(f.attempts().len(), 1);
    assert!(f.ui.play_uci_with_id_at(
        "e2e4",
        "play-preview-e4".to_owned(),
        start + Duration::from_secs(1),
        NOW,
    ));
    tick_with_prime(
        &mut f.ui,
        start + Duration::from_secs(1) + normal(),
        normal(),
        NOW,
    );
    assert!(f.ui.is_finished());
    assert_eq!(f.ui.tally.hints, 1);
    let data = f.store.load().unwrap();
    assert_eq!(data.learning.attempts.len(), 2);
    assert_eq!(
        data.learning.attempts[0].event_type,
        TrainingEventType::Previewed
    );
    assert_eq!(
        data.learning.attempts[1].event_type,
        TrainingEventType::AutoContext
    );
    let item = data
        .learning
        .item(&key_for(&f.library, &f.scope, "e2e4"))
        .unwrap();
    assert_eq!(item.correct_count, 0);
    assert_eq!(item.wrong_count, 0);
    assert!(item.independent_response_times_ms.is_empty());
}

#[test]
fn duplicate_store_response_does_not_advance_the_ui_twice() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::Black,
        &["b8c6"],
        "duplicate",
    );
    let start = Instant::now();
    tick_with_prime(&mut f.ui, start, normal(), NOW);
    assert!(
        f.ui.advance_context_with_id_at("duplicate-context".to_owned(), start + normal(), NOW,)
    );
    assert_eq!(f.ui.session.context_index, 1);
    assert_eq!(f.attempts().len(), 1);

    f.ui.completed_attempt_ids.remove("duplicate-context");
    assert!(f.ui.advance_context_with_id_at(
        "duplicate-context".to_owned(),
        start + normal() * 2,
        NOW,
    ));
    assert_eq!(f.ui.session.context_index, 1);
    assert_eq!(f.attempts().len(), 1);
    assert!(!f.ui.visual_log.contains(&"g1f3".to_owned()));
}

#[test]
fn store_error_is_visible_blocking_and_retryable() {
    let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "store-error");
    let start = Instant::now();
    tick_with_prime(&mut f.ui, start, normal(), NOW);
    let valid_store = f.ui.store.clone();
    f.ui.store = LearningStore::with_legacy(valid_store.path(), valid_store.path());
    assert!(f.ui.play_uci_with_id_at(
        "e2e4",
        "save-error-e4".to_owned(),
        start + Duration::from_millis(100),
        NOW,
    ));
    assert!(f.ui.has_unsaved());
    assert!(f.ui.save_error.is_some());
    assert!(!f.ui.can_input());
    assert!(!f.ui.is_finished());
    assert!(f.attempts().is_empty());
    assert!(!f.ui.play_uci_with_id_at(
        "d2d4",
        "blocked-after-save-error".to_owned(),
        start + Duration::from_millis(200),
        NOW,
    ));

    f.ui.store = valid_store;
    f.ui.retry_pending(start + Duration::from_millis(300));
    assert!(!f.ui.has_unsaved());
    assert!(matches!(f.ui.playback, Some(Playback::Accepted { .. })));
    tick_with_prime(
        &mut f.ui,
        start + Duration::from_millis(300) + normal(),
        normal(),
        NOW,
    );
    assert!(f.ui.is_finished());
    assert_eq!(f.attempts().len(), 1);
}

/// Runs one frame of the whole screen at a window size; returns the side panel's rect.
fn screen_frame(
    ui: &mut OpeningSessionUi,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> (Vec<SessionAction>, egui::Rect) {
    let mut actions = Vec::new();
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(0.0),
            events,
            ..Default::default()
        },
        |root| actions = ui.show(root, &Settings::default(), "Test repertoire"),
    )
    .drop_without_applying_deltas();
    let panel = egui::containers::panel::PanelState::load(ctx, egui::Id::new("opening_side"))
        .or_else(|| {
            egui::containers::panel::PanelState::load(ctx, egui::Id::new("opening_side_stacked"))
        })
        .map(|state| state.outer_rect)
        .unwrap_or(egui::Rect::NOTHING);
    (actions, panel)
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
fn the_panel_takes_the_game_screens_place_at_both_reference_sizes() {
    for (size, left) in [(vec2(1400.0, 900.0), 940.0), (vec2(960.0, 600.0), 576.0)] {
        let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "geometry");
        let ctx = egui::Context::default();
        crate::assets::install(&ctx);
        let mut rects = Vec::new();
        for _ in 0..3 {
            rects.push(screen_frame(&mut f.ui, &ctx, size, Vec::new()).1);
        }
        assert!(
            (rects[2].left() - left).abs() <= 1.0,
            "panel at {} in {size:?}",
            rects[2].left()
        );
        assert_eq!(rects[1], rects[2]);
        // A question, a wrong move and its replay: the panel never moves.
        tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
        assert!(f.ui.play_uci_with_id_at("d2d4", "geometry-wrong".into(), Instant::now(), NOW));
        assert_eq!(screen_frame(&mut f.ui, &ctx, size, Vec::new()).1, rects[2]);
        assert!(f.ui.state() == Some(State::Wrong));
    }
}

#[test]
fn a_new_move_is_shown_before_it_is_asked_and_counts_once_played() {
    let mut f = Fixture::with(
        "1. e4 {The king's pawn.} e5 *",
        LearningSide::White,
        &[],
        &["e2e4"],
        "new-move-shown",
        SessionKind::Daily,
    );
    assert_eq!(f.ui.session.remaining(), 1);
    tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
    // Shown at once: the arrow and the comment visible, no hint; nothing recorded
    // until it is played.
    assert!(f.attempts().is_empty());
    assert!(f.ui.session.showing_new);
    assert_eq!(f.ui.state(), Some(State::New));
    assert_eq!(f.ui.revealed_uci().as_deref(), Some("e2e4"));
    assert!(!f.ui.can_hint());
    assert!(f.ui.can_input());
    let (game, node) = f.ui.expected_node().unwrap();
    assert_eq!(
        crate::notation::comment_text(game.node(node).unwrap().comments_after()),
        "The king's pawn."
    );
    let (title, instruction, _) = f.ui.card_texts(&State::New, true);
    assert_eq!(title, tr("New move"));
    assert!(instruction.contains("1. e4"), "{instruction}");
    assert!(f.ui.play_uci_with_id_at("e2e4", "play-new".into(), Instant::now(), NOW));
    assert_eq!(f.ui.state(), Some(State::NewDone));
    tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
    assert!(f.ui.is_finished());
    assert_eq!(f.ui.session.remaining(), 0);
    assert_eq!(f.ui.tally.new_moves, 1);
    assert_eq!(
        f.ui.tally.mistakes + f.ui.tally.hints + f.ui.tally.first_time,
        0
    );
    let attempts = f.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].event_type, TrainingEventType::Previewed);
    assert_eq!(attempts[1].event_type, TrainingEventType::AutoContext);
}

#[test]
fn a_new_move_left_unplayed_stays_new() {
    let mut f = Fixture::with(
        "1. e4 e5 2. Nf3 *",
        LearningSide::White,
        &[],
        &["e2e4", "g1f3"],
        "new-move-unplayed",
        SessionKind::Daily,
    );
    let at = Instant::now();
    tick_with_prime(&mut f.ui, at, Duration::ZERO, NOW);
    assert!(f.ui.play_uci_with_id_at("e2e4", "play-e4".into(), at, NOW));
    let later = at + Duration::from_secs(5);
    for _ in 0..4 {
        tick_with_prime(&mut f.ui, later, Duration::ZERO, NOW);
    }
    assert_eq!(f.ui.state(), Some(State::New));
    assert_eq!(f.ui.session.current_expected_uci(), Some("g1f3"));
    // A wrong move on a shown new move records nothing either.
    assert!(f.ui.play_uci_with_id_at("d2d4", "wrong-nf3".into(), later, NOW));
    tick_with_prime(&mut f.ui, later + WRONG_SHOWN, Duration::ZERO, NOW);
    assert_eq!(f.ui.state(), Some(State::New));
    let mut actions = Vec::new();
    f.ui.request_exit(&mut actions);
    assert!(f.ui.summary_visible());
    let attempts = f.attempts();
    let nf3 = attempts
        .iter()
        .filter(|attempt| attempt.expected_uci == "g1f3")
        .count();
    assert_eq!(
        nf3,
        0,
        "{:?}",
        attempts.iter().map(|a| a.event_type).collect::<Vec<_>>()
    );
    assert_eq!(f.ui.tally.new_moves, 1);
}

#[test]
fn a_daily_session_brings_mistakes_and_hints_back_and_its_numbers_agree() {
    // White's moves: e4, Nf3, Bb5, then O-O after a6 Ba4 Nf6 is new.
    let pgn = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O *";
    let mut f = Fixture::with(
        pgn,
        LearningSide::White,
        &["e2e4", "g1f3", "f1b5"],
        &["b5a4", "e1g1"],
        "daily-numbers",
        SessionKind::Daily,
    );
    let at = Instant::now();
    assert_eq!(f.ui.session.remaining(), 5);
    tick_with_prime(&mut f.ui, at, Duration::ZERO, NOW);
    // e4: a mistake, then the replay. It comes back: still 5 to go.
    assert_eq!(f.ui.session.current_expected_uci(), Some("e2e4"));
    assert!(f.ui.play_uci_with_id_at("d2d4", "e4-wrong".into(), at, NOW));
    tick_with_prime(&mut f.ui, at + WRONG_SHOWN, Duration::ZERO, NOW);
    assert_eq!(f.ui.state(), Some(State::Wrong));
    answer_current(&mut f, at + WRONG_SHOWN, NOW, "e4-replay");
    assert_eq!(f.ui.session.remaining(), 5);
    // Nf3: a hint, then the move. It comes back too.
    f.ui.debug_settle();
    assert_eq!(f.ui.session.current_expected_uci(), Some("g1f3"));
    assert!(f.ui.request_hint_with_id_at("nf3-hint".into(), at, NOW));
    assert_eq!(f.ui.state(), Some(State::Hint));
    answer_current(&mut f, at, NOW, "nf3-after-hint");
    assert_eq!(f.ui.session.remaining(), 5);
    // Bb5 right; the new moves are shown and played.
    f.ui.debug_settle();
    assert_eq!(f.ui.session.current_expected_uci(), Some("f1b5"));
    answer_current(&mut f, at, NOW, "bb5");
    assert_eq!(f.ui.session.remaining(), 4);
    for _ in 0..2 {
        f.ui.debug_settle();
        assert_eq!(f.ui.state(), Some(State::New));
        let uci = f.ui.session.current_expected_uci().unwrap().to_owned();
        answer_current(&mut f, at, NOW, &format!("new-{uci}"));
    }
    assert_eq!(f.ui.session.remaining(), 2);
    // The returned two, answered right this time.
    for expected in ["e2e4", "g1f3"] {
        f.ui.debug_settle();
        assert_eq!(f.ui.session.current_expected_uci(), Some(expected));
        answer_current(&mut f, at, NOW, &format!("again-{expected}"));
    }
    f.ui.debug_settle();
    assert!(f.ui.is_finished());
    assert_eq!(f.ui.state(), Some(State::Summary));
    let tally = &f.ui.tally;
    assert_eq!(
        (
            tally.first_time,
            tally.mistakes,
            tally.hints,
            tally.new_moves
        ),
        (1, 1, 1, 2)
    );
    assert_eq!(
        tally
            .missed
            .iter()
            .map(|target| target.key.expected_uci.as_str())
            .collect::<Vec<_>>(),
        ["e2e4", "g1f3"]
    );
    let independent = f
        .attempts()
        .iter()
        .filter(|attempt| attempt.event_type.is_independent())
        .count();
    // e4 wrong, e4 right, Nf3 right, Bb5 right.
    assert_eq!(independent, 4);
    f.ui.compute_summary();
    let summary = f.ui.summary.as_ref().unwrap();
    assert_eq!(summary.more_new, 0);
    assert!(!summary.next_review.is_empty());
    // A finished session shows no «0 left».
    assert_eq!(f.ui.counter(&State::Summary).0, "");
}

#[test]
fn leaving_keeps_every_answer_and_shows_what_was_done() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 *",
        LearningSide::White,
        &["e2e4", "g1f3"],
        "leave",
    );
    let ctx = egui::Context::default();
    crate::assets::install(&ctx);
    let size = vec2(1400.0, 900.0);
    tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
    // Nothing answered: straight back, no question.
    let mut early = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "leave-early");
    tick_with_prime(&mut early.ui, Instant::now(), Duration::ZERO, NOW);
    screen_frame(&mut early.ui, &ctx, size, Vec::new());
    let (actions, _) = screen_frame(&mut early.ui, &ctx, size, vec![key(Key::Escape)]);
    assert!(matches!(actions.as_slice(), [SessionAction::Back]));
    // One answer: the result, «Session stopped», and the answer is in the store.
    answer_current(&mut f, Instant::now(), NOW, "leave-e4");
    screen_frame(&mut f.ui, &ctx, size, Vec::new());
    let (actions, _) = screen_frame(&mut f.ui, &ctx, size, vec![key(Key::Escape)]);
    assert!(
        actions
            .iter()
            .all(|action| !matches!(action, SessionAction::Back))
    );
    screen_frame(&mut f.ui, &ctx, size, Vec::new());
    assert_eq!(f.ui.shown_state, State::Summary);
    assert_eq!(
        f.ui.card_texts(&State::Summary, true).0,
        tr("Session stopped")
    );
    assert_eq!(f.ui.counter(&State::Summary).0, "1");
    assert!(!f.ui.can_input());
    let before = f.attempts().len();
    let (actions, _) = screen_frame(&mut f.ui, &ctx, size, vec![key(Key::Enter)]);
    assert!(matches!(actions.as_slice(), [SessionAction::Back]));
    assert_eq!(f.attempts().len(), before);
    assert_eq!(
        f.attempts()
            .iter()
            .filter(|attempt| attempt.event_type == TrainingEventType::CorrectIndependent)
            .count(),
        1
    );
}

#[test]
fn skip_puts_the_question_at_once_and_records_the_moves_shown() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 3. Bb5 *",
        LearningSide::White,
        &["f1b5"],
        "skip",
    );
    let at = Instant::now();
    tick_with_prime(&mut f.ui, at, normal(), NOW);
    assert_eq!(f.ui.state(), Some(State::Watch));
    assert_eq!(f.ui.visual_log, ["e2e4"]);
    assert!(f.ui.skip_context(at));
    tick_with_prime(&mut f.ui, at, normal(), NOW);
    assert!(f.ui.can_input());
    assert_eq!(f.ui.state(), Some(State::Yours));
    assert_eq!(f.ui.session.current_expected_uci(), Some("f1b5"));
    let attempts = f.attempts();
    assert_eq!(attempts.len(), 4);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.event_type == TrainingEventType::AutoContext)
    );
    assert_eq!(
        fen4(&f.ui.board_position),
        key_for(&f.library, &f.scope, "f1b5").position_fen4
    );
    assert_eq!(f.ui.visual_log, ["e2e4"], "skipped moves are not animated");
}

#[test]
fn correct_stays_six_hundred_ms_after_the_move() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 *",
        LearningSide::White,
        &["e2e4", "g1f3"],
        "feedback",
    );
    f.ui.feedback = FEEDBACK;
    let at = Instant::now();
    tick_with_prime(&mut f.ui, at, normal(), NOW);
    assert!(f.ui.play_uci_with_id_at("e2e4", "feedback-e4".into(), at, NOW));
    assert_eq!(f.ui.playback_deadline(), Some(at + normal() + FEEDBACK));
    f.ui.tick_at(at + normal(), normal(), NOW);
    assert_eq!(f.ui.state(), Some(State::Correct));
    f.ui.tick_at(at + normal() + FEEDBACK, normal(), NOW);
    assert_ne!(f.ui.state(), Some(State::Correct));
}

#[test]
fn ten_more_new_continues_the_finished_session_with_new_moves_only() {
    let moves = [
        "Nf3", "Nf6", "g3", "g6", "Bg2", "Bg7", "O-O", "O-O", "d3", "d6", "e4", "e5", "Nc3", "Nc6",
        "a3", "a6", "h3", "h6", "Be3", "Be6", "Qd2", "Qd7", "Rab1", "Rab8", "b4", "b5",
    ];
    let mut pgn = String::new();
    for (index, pair) in moves.chunks(2).enumerate() {
        pgn.push_str(&format!("{}. {} {} ", index + 1, pair[0], pair[1]));
    }
    pgn.push('*');
    let mut f = Fixture::with(
        &pgn,
        LearningSide::White,
        &[],
        &[
            "g1f3", "g2g3", "f1g2", "e1g1", "d2d3", "e2e4", "b1c3", "a2a3", "h2h3", "c1e3", "d1d2",
            "a1b1", "b2b4",
        ],
        "more-new",
        SessionKind::Daily,
    );
    assert_eq!(f.ui.session.remaining(), layout::NEW_PER_SESSION);
    for index in 0..layout::NEW_PER_SESSION {
        f.ui.debug_settle();
        assert_eq!(f.ui.state(), Some(State::New), "move {index}");
        answer_current(&mut f, Instant::now(), NOW, &format!("more-{index}"));
    }
    f.ui.debug_settle();
    assert!(f.ui.is_finished());
    f.ui.compute_summary();
    assert_eq!(f.ui.more_new_count(), 3);
    let snapshot = f.ui.snapshot().clone();
    let mut more =
        OpeningSessionUi::more_new(f.library.clone(), f.store.clone(), snapshot, Color::Black)
            .unwrap();
    assert_eq!(more.session.remaining(), 3);
    assert_eq!(more.orientation(), Color::Black);
    assert_eq!(more.session.id(), f.ui.session.id());
    more.synchronous_commits = true;
    more.board_shown = true;
    more.debug_settle();
    assert_eq!(more.state(), Some(State::New));
    assert_eq!(more.session.current_expected_uci(), Some("d1d2"));
}

#[test]
fn a_review_counts_lines_and_announces_the_next_one() {
    let mut f = Fixture::with(
        "1. e4 e5 2. Nf3 (2. Bc4) *",
        LearningSide::White,
        &[],
        &[],
        "review-lines",
        SessionKind::ReviewAll,
    );
    assert_eq!(f.ui.lines.len(), 2);
    tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
    assert_eq!(f.ui.counter(&State::Yours).0, "1/2");
    answer_current(&mut f, Instant::now(), NOW, "review-e4");
    f.ui.debug_settle();
    let at = Instant::now();
    assert!(f.ui.play_uci_with_id_at("g1f3", "review-nf3".into(), at, NOW));
    // Bc4 is in the second line; its context starts again from the beginning.
    f.ui.tick_at(at + Duration::from_secs(1), normal(), NOW);
    assert!(f.ui.pending_automatic.is_some());
    assert_eq!(f.ui.line, 2);
    assert!(f.ui.next_line_banner);
    assert_eq!(f.ui.state(), Some(State::NextLine));
    f.ui.debug_settle();
    assert_eq!(f.ui.session.current_expected_uci(), Some("f1c4"));
    answer_current(&mut f, Instant::now(), NOW, "review-bc4");
    f.ui.debug_settle();
    assert!(f.ui.is_finished());
    assert_eq!(f.ui.counter(&State::Summary).0, "2/2");
    f.ui.compute_summary();
    assert_eq!(f.ui.more_new_count(), 0, "no «10 more new» after a review");
}

#[test]
fn a_new_session_never_resumes_an_unfinished_one() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 *",
        LearningSide::White,
        &["e2e4", "g1f3"],
        "no-resume",
    );
    tick_with_prime(&mut f.ui, Instant::now(), Duration::ZERO, NOW);
    answer_current(&mut f, Instant::now(), NOW, "first-e4");
    let old = f.ui.session_id().to_owned();
    let again = OpeningSessionUi::new(
        f.library.clone(),
        f.store.clone(),
        f.scope.clone(),
        SessionKind::Daily,
    )
    .unwrap();
    assert_ne!(again.session_id(), old);
    // The unfinished snapshot is kept in the file, untouched.
    let data = f.store.load().unwrap();
    assert!(data.session_snapshot(&old).is_some());
    assert_eq!(again.session.remaining(), 1);
}

fn wait_for_commit(ui: &mut OpeningSessionUi) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while ui.commit_in_flight.is_some() {
        assert!(Instant::now() < deadline, "commit worker did not finish");
        ui.poll_commit(Instant::now());
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn async_commit_keeps_frames_responsive_under_store_lock_and_commits_once() {
    let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "async-lock");
    f.ui.synchronous_commits = false;
    tick_with_prime(&mut f.ui, Instant::now(), normal(), NOW);
    let lock_path = f.store.path().with_file_name("learning.json.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    lock.lock().unwrap();
    let unlock = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        drop(lock);
    });
    let start = Instant::now();
    assert!(f.ui.play_uci_with_id_at("e2e4", "async-answer".to_owned(), start, NOW));
    assert!(
        start.elapsed() < Duration::from_millis(300),
        "input waited for storage lock"
    );
    assert!(f.ui.has_unsaved());
    assert!(f.ui.commit_in_flight.is_some());
    assert!(!f.ui.can_input());
    assert!(
        !f.ui
            .play_uci_with_id_at("e2e4", "async-answer".to_owned(), Instant::now(), NOW)
    );
    f.ui.retry_pending(Instant::now());
    let ctx = egui::Context::default();
    let frame = Instant::now();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        f.ui.show(ui, &Settings::default(), "Test");
    })
    .drop_without_applying_deltas();
    assert!(
        frame.elapsed() < Duration::from_millis(300),
        "frame waited for storage lock"
    );
    unlock.join().unwrap();
    wait_for_commit(&mut f.ui);
    assert!(!f.ui.has_unsaved());
    assert!(matches!(f.ui.playback, Some(Playback::Accepted { .. })));
    assert_eq!(f.attempts().len(), 1);
    let revision = f.ui.session.revision;
    f.ui.poll_commit(Instant::now());
    f.ui.retry_pending(Instant::now());
    assert_eq!(f.ui.session.revision, revision);
    assert_eq!(f.attempts().len(), 1);
}

fn pointer_board_frame(
    f: &mut Fixture,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    settings: &Settings,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(800.0, 800.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| f.ui.board_ui(ui, settings),
    );
    output.textures_delta.clear();
    output
}

fn piece_meshes(output: &egui::FullOutput) -> Vec<(egui::TextureId, egui::Rect)> {
    fn collect(shape: &egui::Shape, meshes: &mut Vec<(egui::TextureId, egui::Rect)>) {
        match shape {
            egui::Shape::Mesh(mesh) => meshes.push((mesh.texture_id, mesh.calc_bounds())),
            egui::Shape::Rect(rect) => {
                if let Some(brush) = &rect.brush {
                    meshes.push((brush.fill_texture_id, rect.rect));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, meshes);
                }
            }
            _ => {}
        }
    }
    let mut meshes = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut meshes);
    }
    meshes
}

fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn pointer_move_stays_at_destination_during_async_save_without_a_second_animation() {
    for drag in [true, false] {
        for wrong in [false, true] {
            for animation in [
                crate::settings::Animation::Normal,
                crate::settings::Animation::Off,
            ] {
                let mut f =
                    Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "pointer-pending");
                f.ui.synchronous_commits = false;
                let settings = Settings {
                    animation,
                    ..Settings::default()
                };
                let duration = Duration::from_secs_f32(animation.seconds());
                f.ui.tick_at(Instant::now(), duration, NOW);
                let ctx = egui::Context::default();
                crate::assets::install(&ctx);
                let from = egui::pos2(450.0, 650.0);
                let to = egui::pos2(450.0, if wrong { 550.0 } else { 450.0 });
                let initial = pointer_board_frame(&mut f, &ctx, 1.0, vec![], &settings);
                let pawn = piece_meshes(&initial)
                    .into_iter()
                    .find(|(_, rect)| rect.contains(from))
                    .expect("painted pawn at e2")
                    .0;
                initial.drop_without_applying_deltas();
                pointer_board_frame(
                    &mut f,
                    &ctx,
                    1.01,
                    vec![egui::Event::PointerMoved(from), pointer_button(from, true)],
                    &settings,
                )
                .drop_without_applying_deltas();
                if drag {
                    pointer_board_frame(
                        &mut f,
                        &ctx,
                        1.02,
                        vec![egui::Event::PointerMoved(to)],
                        &settings,
                    )
                    .drop_without_applying_deltas();
                } else {
                    pointer_board_frame(
                        &mut f,
                        &ctx,
                        1.02,
                        vec![pointer_button(from, false)],
                        &settings,
                    )
                    .drop_without_applying_deltas();
                    pointer_board_frame(
                        &mut f,
                        &ctx,
                        1.03,
                        vec![egui::Event::PointerMoved(to), pointer_button(to, true)],
                        &settings,
                    )
                    .drop_without_applying_deltas();
                }
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(f.store.path().with_file_name("learning.json.lock"))
                    .unwrap();
                lock.lock().unwrap();
                let released = pointer_board_frame(
                    &mut f,
                    &ctx,
                    1.04,
                    vec![pointer_button(to, false)],
                    &settings,
                );
                let assert_pawn_at_destination = |output: &egui::FullOutput| {
                    let meshes = piece_meshes(output);
                    assert!(
                        meshes
                            .iter()
                            .any(|(texture, rect)| *texture == pawn && rect.contains(to)),
                        "pawn not painted at destination"
                    );
                    assert!(
                        !meshes
                            .iter()
                            .any(|(texture, rect)| *texture == pawn && rect.contains(from)),
                        "pawn snapped back to e2"
                    );
                };
                assert!(f.ui.has_unsaved(), "drag={drag} wrong={wrong}");
                assert!(!f.ui.can_input());
                assert!(f.ui.pending_sounds.is_empty());
                if drag {
                    assert_pawn_at_destination(&released);
                }
                released.drop_without_applying_deltas();
                let frame = Instant::now();
                let waiting = pointer_board_frame(&mut f, &ctx, 1.05, vec![], &settings);
                assert!(frame.elapsed() < Duration::from_millis(300));
                if drag {
                    assert_pawn_at_destination(&waiting);
                    assert!(
                        f.ui.board
                            .animation_at(1.05)
                            .is_none_or(|(_, slides, _)| slides.is_empty())
                    );
                } else if !duration.is_zero() {
                    assert!(
                        f.ui.board
                            .animation_at(1.05)
                            .unwrap()
                            .1
                            .iter()
                            .any(|(_, source, _)| *source == Square::E2),
                        "click move must still animate"
                    );
                }
                waiting.drop_without_applying_deltas();
                let waiting = pointer_board_frame(&mut f, &ctx, 2.0, vec![], &settings);
                assert_pawn_at_destination(&waiting);
                waiting.drop_without_applying_deltas();
                assert!(
                    f.ui.learning.attempts.is_empty(),
                    "unconfirmed answer affected learning data"
                );
                drop(lock);
                wait_for_commit(&mut f.ui);
                let confirmed = pointer_board_frame(&mut f, &ctx, 2.01, vec![], &settings);
                assert_pawn_at_destination(&confirmed);
                confirmed.drop_without_applying_deltas();
                assert!(
                    f.ui.board.animation_at(2.01).is_none(),
                    "commit restarted the move animation"
                );
                assert_eq!(f.attempts().len(), 1);
                assert_eq!(
                    f.ui.pending_sounds,
                    [if wrong { Sound::Wrong } else { Sound::Move }]
                );
                if wrong {
                    f.ui.tick_at(f.ui.playback_deadline().unwrap(), duration, NOW);
                    pointer_board_frame(&mut f, &ctx, 2.02, vec![], &settings)
                        .drop_without_applying_deltas();
                    assert_eq!(fen4(&f.ui.board_position), fen4(&Chess::default()));
                    if !duration.is_zero() {
                        assert!(
                            f.ui.board
                                .animation_at(2.02)
                                .unwrap()
                                .1
                                .iter()
                                .any(|(_, _, target)| *target == Square::E2),
                            "wrong move must return to source"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn dragged_special_moves_keep_auxiliary_effects_and_retry_does_not_move_again() {
    let cases = [
        (
            "4k3/8/8/8/8/8/8/4K2R w K - 0 1",
            "O-O",
            "e1g1",
            Square::E1,
            Square::G1,
            None,
            false,
        ),
        (
            "4k3/8/8/3p4/4P3/8/8/4K3 w - - 0 1",
            "exd5",
            "e4d5",
            Square::E4,
            Square::D5,
            Some(Square::D5),
            false,
        ),
        (
            "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1",
            "exd6",
            "e5d6",
            Square::E5,
            Square::D6,
            Some(Square::D5),
            false,
        ),
        (
            "4k3/P7/8/8/8/8/8/4K3 w - - 0 1",
            "a8=Q+",
            "a7a8q",
            Square::A7,
            Square::A8,
            None,
            true,
        ),
    ];
    let center = |sq: Square| {
        egui::pos2(
            (u32::from(sq.file()) as f32 + 0.5) * 100.0,
            (7.5 - u32::from(sq.rank()) as f32) * 100.0,
        )
    };
    for (fen, san, uci, from, to, captured, promotion) in cases {
        let pgn = format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n1. {san} *");
        let mut f = Fixture::new(&pgn, LearningSide::White, &[uci], "special-drag");
        f.ui.synchronous_commits = false;
        f.ui.tick_at(Instant::now(), normal(), NOW);
        let valid_store = f.ui.store.clone();
        f.ui.store = LearningStore::with_legacy(valid_store.path(), valid_store.path());
        let ctx = egui::Context::default();
        crate::assets::install(&ctx);
        let settings = Settings::default();
        let initial = pointer_board_frame(&mut f, &ctx, 1.0, vec![], &settings);
        let captured_texture = captured.map(|sq| {
            piece_meshes(&initial)
                .into_iter()
                .find(|(_, rect)| rect.contains(center(sq)))
                .unwrap()
                .0
        });
        initial.drop_without_applying_deltas();
        pointer_board_frame(
            &mut f,
            &ctx,
            1.01,
            vec![
                egui::Event::PointerMoved(center(from)),
                pointer_button(center(from), true),
            ],
            &settings,
        )
        .drop_without_applying_deltas();
        pointer_board_frame(
            &mut f,
            &ctx,
            1.02,
            vec![egui::Event::PointerMoved(center(to))],
            &settings,
        )
        .drop_without_applying_deltas();
        let released = pointer_board_frame(
            &mut f,
            &ctx,
            1.03,
            vec![pointer_button(center(to), false)],
            &settings,
        );
        if uci == "e1g1" {
            let (_, slides, _) = f.ui.board.animation_at(1.03).unwrap();
            assert_eq!(slides.len(), 1);
            assert_eq!(
                (slides[0].1, slides[0].2),
                (Square::H1, Square::F1),
                "only the rook should animate after a dragged castle"
            );
        }
        if let Some(texture) = captured_texture {
            assert!(
                piece_meshes(&released)
                    .iter()
                    .any(|(id, rect)| *id == texture && rect.contains(center(captured.unwrap()))),
                "captured piece must still fade out"
            );
        }
        released.drop_without_applying_deltas();
        if promotion {
            assert!(
                !f.ui.has_unsaved(),
                "promotion must wait for the chosen piece"
            );
            pointer_board_frame(
                &mut f,
                &ctx,
                1.04,
                vec![pointer_button(center(to), true)],
                &settings,
            )
            .drop_without_applying_deltas();
            pointer_board_frame(
                &mut f,
                &ctx,
                1.05,
                vec![pointer_button(center(to), false)],
                &settings,
            )
            .drop_without_applying_deltas();
            assert_eq!(
                f.ui.board_position.board().piece_at(to).unwrap().role,
                chess_core::shakmaty::Role::Queen
            );
        }
        assert!(f.ui.has_unsaved());
        let after = fen4(&f.ui.board_position);
        wait_for_commit(&mut f.ui);
        assert!(f.ui.save_error.is_some());
        assert!(f.attempts().is_empty());
        assert!(f.ui.pending_sounds.is_empty());
        pointer_board_frame(&mut f, &ctx, 1.1, vec![], &settings).drop_without_applying_deltas();
        let waiting = pointer_board_frame(&mut f, &ctx, 2.0, vec![], &settings);
        if let Some(texture) = captured_texture {
            assert!(
                !piece_meshes(&waiting)
                    .iter()
                    .any(|(id, rect)| *id == texture && rect.contains(center(captured.unwrap())))
            );
        }
        waiting.drop_without_applying_deltas();
        f.ui.store = valid_store;
        f.ui.retry_pending(Instant::now());
        f.ui.retry_pending(Instant::now());
        wait_for_commit(&mut f.ui);
        pointer_board_frame(&mut f, &ctx, 2.01, vec![], &settings).drop_without_applying_deltas();
        assert_eq!(fen4(&f.ui.board_position), after);
        assert!(f.ui.board.animation_at(2.01).is_none());
        assert_eq!(f.attempts().len(), 1);
        assert_eq!(f.ui.pending_sounds, [Sound::Move]);
    }
}

#[test]
fn reused_context_retry_stays_silent_and_responsive_and_persists_once() {
    let mut f = Fixture::new(
        "1. e4 e5 2. Nf3 Nc6 *",
        LearningSide::White,
        &["e2e4", "g1f3"],
        "reuse-retry",
    );
    let at = Instant::now();
    tick_with_prime(&mut f.ui, at, normal(), NOW);
    assert!(f.ui.play_uci_with_id_at("e2e4", "own-answer".into(), at, NOW));
    let position = fen4(&f.ui.board_position);
    assert_eq!(std::mem::take(&mut f.ui.pending_sounds), [Sound::Move]);
    let valid_store = f.ui.store.clone();
    f.ui.synchronous_commits = false;
    f.ui.store = LearningStore::with_legacy(valid_store.path(), valid_store.path());
    f.ui.tick_at(at + normal(), normal(), NOW);
    wait_for_commit(&mut f.ui);
    assert!(f.ui.save_error.is_some());
    assert!(matches!(
        f.ui.pending_commit.as_ref().unwrap().kind,
        CommitKind::ReusedContext
    ));
    let reused_id = f.ui.pending_commit.as_ref().unwrap().attempts[0]
        .client_attempt_id
        .clone();
    assert_eq!(fen4(&f.ui.board_position), position);
    assert!(f.ui.pending_sounds.is_empty());
    f.ui.store = valid_store;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.store.path().with_file_name("learning.json.lock"))
        .unwrap();
    lock.lock().unwrap();
    let unlock = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        drop(lock);
    });
    f.ui.retry_pending(Instant::now());
    f.ui.retry_pending(Instant::now());
    let frame = Instant::now();
    assert!(emitted_sounds(&mut f, &egui::Context::default(), &Settings::default()).is_empty());
    assert!(
        frame.elapsed() < Duration::from_millis(300),
        "reused context blocked the frame"
    );
    assert_eq!(fen4(&f.ui.board_position), position);
    assert!(!f.ui.can_input());
    unlock.join().unwrap();
    wait_for_commit(&mut f.ui);
    assert!(f.ui.pending_sounds.is_empty());
    f.ui.debug_settle();
    assert_eq!(f.ui.visual_log, ["e7e5"]);
    assert_eq!(f.ui.session.current_expected_uci(), Some("g1f3"));
    let attempts = f.attempts();
    assert_eq!(attempts.len(), 3);
    assert_eq!(
        attempts
            .iter()
            .filter(|event| event.client_attempt_id == reused_id)
            .count(),
        1
    );
    assert_eq!(
        attempts
            .iter()
            .filter(|event| event.event_type.is_independent())
            .count(),
        1
    );
}

#[test]
fn async_failed_commit_preserves_event_for_retry_without_double_counting() {
    let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "async-retry");
    f.ui.synchronous_commits = false;
    tick_with_prime(&mut f.ui, Instant::now(), normal(), NOW);
    let valid_store = f.ui.store.clone();
    f.ui.store = LearningStore::with_legacy(valid_store.path(), valid_store.path());
    assert!(f.ui.play_uci_with_id_at("e2e4", "async-retry".to_owned(), Instant::now(), NOW));
    wait_for_commit(&mut f.ui);
    assert!(f.ui.save_error.is_some());
    assert!(f.ui.has_unsaved());
    assert!(f.attempts().is_empty());
    f.ui.store = valid_store;
    f.ui.retry_pending(Instant::now());
    f.ui.retry_pending(Instant::now());
    wait_for_commit(&mut f.ui);
    assert!(!f.ui.has_unsaved());
    assert_eq!(f.attempts().len(), 1);
    assert_eq!(f.attempts()[0].client_attempt_id, "async-retry");
}

#[test]
fn a_mistake_on_the_first_move_returns_it_once_and_keeps_the_count() {
    // The next daily session after learning new moves: the learned e4 is due again
    // today, Nf3 and Bb5 are still new. The count is due + new, and a mistake on the
    // first move does not grow it.
    let pgn = "1. e4 e5 2. Nf3 Nc6 3. Bb5 *";
    let mut f = Fixture::with(
        pgn,
        LearningSide::White,
        &["e2e4"],
        &["g1f3", "f1b5"],
        "first-move-wrong",
        SessionKind::Daily,
    );
    let at = Instant::now();
    assert_eq!(f.ui.session.remaining(), 3);
    tick_with_prime(&mut f.ui, at, Duration::ZERO, NOW);
    assert_eq!(f.ui.session.current_expected_uci(), Some("e2e4"));
    assert!(f.ui.play_uci_with_id_at("d2d4", "e4-wrong".into(), at, NOW));
    tick_with_prime(&mut f.ui, at + WRONG_SHOWN, Duration::ZERO, NOW);
    assert_eq!(f.ui.state(), Some(State::Wrong));
    assert_eq!(f.ui.session.remaining(), 3);
    answer_current(&mut f, at + WRONG_SHOWN, NOW, "e4-replay");
    assert_eq!(f.ui.session.remaining(), 3);
    let mut asked = Vec::new();
    for _ in 0..4 {
        f.ui.debug_settle();
        let Some(uci) = f.ui.session.current_expected_uci().map(str::to_owned) else {
            break;
        };
        answer_current(&mut f, at, NOW, &format!("again-{uci}-{}", asked.len()));
        asked.push(uci);
    }
    f.ui.debug_settle();
    assert!(f.ui.is_finished());
    assert_eq!(asked, ["g1f3", "f1b5", "e2e4"]);
    assert_eq!(f.ui.session.remaining(), 0);
    f.ui.compute_summary();
    let summary = f.ui.summary.as_ref().unwrap();
    assert_eq!((summary.unseen, summary.more_new), (0, 0));
    assert!(!summary.next_review.is_empty());
}

#[test]
fn the_summary_shows_learning_growth_when_nothing_is_learned_yet() {
    let mut f = Fixture::with(
        "1. e4 e5 2. Nf3 *",
        LearningSide::White,
        &[],
        &["e2e4", "g1f3"],
        "summary-learning",
        SessionKind::Daily,
    );
    for _ in 0..2 {
        f.ui.debug_moves("answer");
        f.ui.debug_settle();
    }
    assert!(f.ui.is_finished());
    f.ui.compute_summary();
    let summary = f.ui.summary.as_ref().unwrap();
    assert_eq!((summary.learnt_gain, summary.learning_gain), (0, 2));
}
