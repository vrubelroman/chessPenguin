use std::time::Duration;

use chess_core::shakmaty::{Chess, Color, Move, Position, Square, uci::UciMove};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText, Ui, vec2};
use trainer::repertoire::{
    Candidate, QueueMode, Repertoire, RepertoireTree, SessionError, SrsPolicy, StatsData,
    StatsStore, TrainingMode, TrainingScope, TrainingSession, TrainingVerdict,
};

use crate::assets::Sound;
use crate::board::{Arrow, BoardState, BoardView};
use crate::i18n::{tr, trf};
use crate::settings::Settings;

const WRONG_SHOWN: Duration = Duration::from_millis(400);

pub enum TrainingAction {
    Back,
    Sound(Sound),
    Toast(String),
}

struct Flash {
    after: Chess,
    from: Square,
    to: Square,
    until: web_time::Instant,
    wrong: bool,
}

pub struct TrainingUi {
    repertoire: Repertoire,
    saved_stats: StatsData,
    discard_requested: bool,
    mode: TrainingMode,
    queue_mode: QueueMode,
    policy: SrsPolicy,
    session: TrainingSession,
    store: StatsStore,
    board: BoardState,
    flipped: bool,
    positions: Vec<Chess>,
    position_path_len: usize,
    position_task_start: usize,
    task_start: usize,
    task_origin: Chess,
    view_ply: usize,
    flash: Option<Flash>,
    hint: Option<(Square, Square)>,
    message: Option<String>,
    save_error: Option<String>,
    confirm_discard: bool,
    completed_targets: usize,
    target_limit: usize,
}

impl TrainingUi {
    #[cfg(test)]
    pub fn new(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        store: StatsStore,
        flipped: bool,
    ) -> Result<Self, SessionError> {
        let policy = initial_policy(repertoire, &scope, mode, queue_mode, &stats);
        Self::new_with_policy(
            repertoire, scope, mode, queue_mode, stats, store, flipped, policy,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_policy(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        store: StatsStore,
        flipped: bool,
        policy: SrsPolicy,
    ) -> Result<Self, SessionError> {
        let target_limit = target_count(repertoire, &scope, mode, queue_mode, policy, &stats);
        let saved_stats = stats.clone();
        let session = if queue_mode == QueueMode::SpacedRepetition {
            TrainingSession::scoped_srs(repertoire, scope, mode, stats, now(), policy)?
        } else {
            TrainingSession::scoped(repertoire, scope, mode, queue_mode, stats, now())?
        };
        let task_origin = if session.scope().include_context {
            repertoire.game().start().position().clone()
        } else {
            session.scope_start_position().clone()
        };
        Ok(Self {
            repertoire: repertoire.clone(),
            saved_stats,
            discard_requested: false,
            mode,
            queue_mode,
            policy,
            session,
            store,
            board: BoardState::default(),
            flipped: match mode {
                TrainingMode::White => false,
                TrainingMode::Black => true,
                TrainingMode::Both => flipped,
            },
            positions: Vec::new(),
            position_path_len: usize::MAX,
            position_task_start: usize::MAX,
            task_start: 0,
            task_origin,
            view_ply: 0,
            flash: None,
            hint: None,
            message: None,
            save_error: None,
            confirm_discard: false,
            completed_targets: 0,
            target_limit,
        })
    }

    fn is_path_practice(&self) -> bool {
        self.mode == TrainingMode::Both || self.queue_mode == QueueMode::Practice
    }

    pub fn repertoire_id(&self) -> &trainer::repertoire::RepertoireId {
        &self.repertoire.id
    }

    pub fn target_available(&self) -> bool {
        !self.session.is_done()
            && (self.session.target().is_some()
                || (self.is_path_practice() && !self.session.candidates().is_empty()))
    }

    pub fn stats(&self) -> &StatsData {
        &self.saved_stats
    }

    pub fn has_unsaved(&self) -> bool {
        self.save_error.is_some()
    }

    pub fn is_finished(&self) -> bool {
        self.session.is_done()
    }

    fn refresh_positions(&mut self) {
        if self.position_path_len == self.session.path().len()
            && self.position_task_start == self.task_start
        {
            return;
        }
        self.position_path_len = self.session.path().len();
        self.position_task_start = self.task_start;
        self.positions.clear();
        // Replay the actual section from its full starting position. FEN4 is an
        // identity key only: it deliberately omits move counters and history.
        let mut position = self.task_origin.clone();
        self.positions.push(position.clone());
        for played in &self.session.path()[self.task_start..] {
            let Some(mv) = played
                .uci
                .parse::<UciMove>()
                .ok()
                .and_then(|uci| uci.to_move(&position).ok())
            else {
                self.positions.clear();
                self.positions.push(self.session.position().clone());
                self.view_ply = 0;
                return;
            };
            position.play_unchecked(mv);
            self.positions.push(position.clone());
        }
        self.view_ply = self.view_ply.min(self.positions.len().saturating_sub(1));
        if self.view_ply == 0 && !self.positions.is_empty() {
            self.view_ply = self.positions.len() - 1;
        }
    }

    fn go_end(&mut self) {
        self.view_ply = self.positions.len().saturating_sub(1);
    }

    fn save(&mut self) {
        match self.store.save(self.session.stats_mut()) {
            Ok(()) => {
                self.saved_stats = self.session.stats().clone();
                self.save_error = None;
            }
            Err(error) => self.save_error = Some(error.to_string()),
        }
    }

    fn play(&mut self, mv: Move) {
        if self.is_finished() || self.flash.is_some() || self.save_error.is_some() {
            return;
        }
        let mut after = self.session.position().clone();
        after.play_unchecked(mv);
        let from = mv.from().unwrap_or_else(|| mv.to());
        let to = crate::board::ui_target(&mv);
        let verdict = self.session.play(mv, now());
        match verdict {
            Ok(TrainingVerdict::Right | TrainingVerdict::Finished) => {
                self.completed_targets += usize::from(self.mode != TrainingMode::Both);
                self.hint = None;
                self.message = Some(
                    if self.queue_mode == QueueMode::SpacedRepetition
                        && self.session.attempts().last().is_some_and(|a| a.hinted)
                    {
                        tr("Accepted · it will be repeated sooner").to_owned()
                    } else {
                        tr("Move accepted").to_owned()
                    },
                );
                self.update_task_start();
                self.save();
                self.refresh_positions();
                self.go_end();
            }
            Ok(TrainingVerdict::Wrong) => {
                self.message = Some(tr("This move is not in the enabled repertoire. Try another move or show a hint.").to_owned());
                self.flash = Some(Flash {
                    after,
                    from,
                    to,
                    until: web_time::Instant::now() + WRONG_SHOWN,
                    wrong: true,
                });
                self.save();
            }
            Ok(TrainingVerdict::TryAgain) => {
                self.message =
                    Some(tr("Choose a move from your repertoire or show a hint.").to_owned());
            }
            Ok(TrainingVerdict::Done) => {}
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn restart(&mut self) {
        if self.save_error.is_some() {
            return;
        }
        match self.session.restart(now()) {
            Ok(()) => self.after_run_reset(),
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn next_path(&mut self) {
        if self.save_error.is_some() || !self.is_path_practice() {
            return;
        }
        match self.session.next_path(now()) {
            Ok(()) => self.after_run_reset(),
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn after_run_reset(&mut self) {
        self.completed_targets = 0;
        self.target_limit = target_count(
            &self.repertoire,
            self.session.scope(),
            self.mode,
            self.queue_mode,
            self.policy,
            self.session.stats(),
        );
        self.task_start = 0;
        self.task_origin = if self.session.scope().include_context {
            self.repertoire.game().start().position().clone()
        } else {
            self.session.scope_start_position().clone()
        };
        self.flash = None;
        self.hint = None;
        self.message = None;
        self.position_path_len = usize::MAX;
        self.position_task_start = usize::MAX;
        self.view_ply = 0;
        self.board.clear();
        self.refresh_positions();
        self.go_end();
    }

    fn update_task_start(&mut self) {
        if self.is_path_practice() {
            return;
        }
        if self.session.target().is_some() {
            self.task_origin = if self.session.scope().include_context {
                self.repertoire.game().start().position().clone()
            } else {
                self.session.position().clone()
            };
        }
        self.task_start = match self.session.target() {
            Some(target) if self.session.scope().include_context => {
                self.session.path().len().saturating_sub(target.path.len())
            }
            Some(_) => self.session.path().len(),
            None => self.task_start,
        };
    }

    fn keyboard(&mut self, ui: &Ui, actions: &mut Vec<TrainingAction>) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let mut command = None;
        ui.input_mut(|input| {
            if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W))
                || (!self.board.is_busy() && input.consume_key(Modifiers::NONE, Key::Escape))
            {
                command = Some("back");
            } else if input.consume_key(Modifiers::NONE, Key::F) {
                command = Some("flip");
            } else if input.consume_key(Modifiers::NONE, Key::L)
                || input.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::A))
            {
                command = Some("engine");
            } else if self.is_finished() && input.consume_key(Modifiers::NONE, Key::Enter) {
                command = Some(if self.is_path_practice() {
                    "next"
                } else {
                    "restart"
                });
            } else if input.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                command = Some("left");
            } else if input.consume_key(Modifiers::NONE, Key::ArrowRight) {
                command = Some("right");
            } else if input.consume_key(Modifiers::NONE, Key::Home) {
                command = Some("home");
            } else if input.consume_key(Modifiers::NONE, Key::End) {
                command = Some("end");
            }
        });
        match command {
            Some("back") => {
                if self.save_error.is_some() {
                    self.confirm_discard = true;
                } else {
                    actions.push(TrainingAction::Back);
                }
            }
            Some("flip") => self.flipped = !self.flipped,
            Some("engine") => actions.push(TrainingAction::Toast(
                tr("The engine is off while training").to_owned(),
            )),
            Some("restart") => self.restart(),
            Some("next") => self.next_path(),
            Some("left") => self.view_ply = self.view_ply.saturating_sub(1),
            Some("right") => {
                self.view_ply = (self.view_ply + 1).min(self.positions.len().saturating_sub(1));
            }
            Some("home") => self.view_ply = 0,
            Some("end") => self.go_end(),
            _ => {}
        }
    }

    fn board(&mut self, ui: &mut Ui, settings: &Settings, actions: &mut Vec<TrainingAction>) {
        self.refresh_positions();
        let normal = self
            .positions
            .get(self.view_ply)
            .cloned()
            .unwrap_or_else(|| self.session.position().clone());
        let (position, alert) = match &self.flash {
            Some(flash) => (
                flash.after.clone(),
                flash.wrong.then_some((flash.from, flash.to)),
            ),
            None => (normal, None),
        };
        let last_move = self.flash.as_ref().map(|f| (f.from, f.to)).or_else(|| {
            self.view_ply.checked_sub(1).and_then(|index| {
                let mv = self.session.path().get(self.task_start + index)?;
                let before = self.positions.get(index)?;
                let mv = mv.uci.parse::<UciMove>().ok()?.to_move(before).ok()?;
                Some((
                    mv.from().unwrap_or_else(|| mv.to()),
                    crate::board::ui_target(&mv),
                ))
            })
        });
        let arrows: Vec<Arrow> = self
            .hint
            .map(|(from, to)| Arrow {
                from,
                to,
                color: Color32::from_rgba_unmultiplied(21, 120, 27, 190),
                weight: 1.0,
            })
            .into_iter()
            .collect();
        let area = ui.available_rect_before_wrap();
        let side = area.width().min(area.height()).max(1.0).floor();
        let left = area.left() + ((area.width() - side) / 2.0).max(0.0).floor();
        let rect = egui::Rect::from_min_size(egui::pos2(left, area.top()), vec2(side, side));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            let view = BoardView {
                pos: &position,
                orientation: if self.flipped {
                    Color::Black
                } else {
                    Color::White
                },
                last_move,
                arrows: &arrows,
                circles: &[],
                badge: None,
                interactive: !self.is_finished()
                    && self.flash.is_none()
                    && self.save_error.is_none()
                    && self.view_ply + 1 == self.positions.len(),
                show_coords: settings.show_coords,
                theme: settings.board_theme.theme(),
                pieces: settings.piece_set,
                animation: settings.animation.seconds(),
                premove: None,
                auto_queen: false,
                alert,
            };
            if let Some(mv) = view.show(ui, &mut self.board) {
                if self.view_ply + 1 == self.positions.len() {
                    self.play(mv);
                    if self.flash.is_some() {
                        if self.flash.as_ref().is_some_and(|f| f.wrong) {
                            actions.push(TrainingAction::Sound(Sound::Wrong));
                        }
                    } else if self.is_finished() {
                        actions.push(TrainingAction::Sound(Sound::Success));
                    } else {
                        actions.push(TrainingAction::Sound(Sound::Move));
                    }
                } else {
                    self.go_end();
                }
            }
        });
    }

    fn task_card(&self, ui: &mut Ui) {
        let frame = card(ui, self.flash.as_ref().is_some_and(|f| f.wrong));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let position = self.session.position();
            let heading = if self.is_path_practice() {
                tr("Continue the variation").to_owned()
            } else {
                trf(
                    "Task {current} of {total}",
                    &[
                        (
                            "current",
                            &(self.completed_targets + 1).min(self.target_limit.max(1)),
                        ),
                        ("total", &self.target_limit),
                    ],
                )
            };
            ui.label(RichText::new(heading).strong().size(15.0));
            ui.label(
                RichText::new(if position.turn() == Color::White {
                    tr("♔ White to move")
                } else {
                    tr("♚ Black to move")
                })
                .strong(),
            );
            ui.label(tr(if self.mode == TrainingMode::Both {
                "Play both sides. Any enabled move is accepted."
            } else {
                "Your turn: play any move from your repertoire. The program will reply."
            }));
            if self.session.candidates().len() > 1 {
                ui.label(tr("Several continuations are available. Choose the variation you want to practise."));
            }
        });
    }

    fn user_card(&mut self, ui: &mut Ui) {
        let alert = self.flash.as_ref().is_some_and(|f| f.wrong);
        card(ui, alert).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(trf(
                    "Mistakes: {n}",
                    &[("n", &self.session.total_mistakes())],
                ))
                .size(15.0),
            );
            if let Some(message) = &self.message {
                ui.label(RichText::new(message).color(if alert {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().text_color()
                }));
            }
            if self.session.hinted() {
                ui.label(RichText::new(tr("With hint")).weak());
            }
            if self.session.may_hint()
                && self.hint.is_none()
                && self.flash.is_none()
                && ui.link(tr("I don't remember — show a move")).clicked()
                && let Some(candidate) = self.session.hint(now())
            {
                self.hint = move_squares(&candidate);
                self.save();
            }
        });
    }

    fn result(&mut self, ui: &mut Ui, actions: &mut Vec<TrainingAction>) {
        card(ui, false).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(if self.is_path_practice() {
                    tr("Branch complete")
                } else {
                    tr("Training complete")
                })
                .strong()
                .size(17.0),
            );
            if self.is_path_practice() {
                let played = self
                    .session
                    .path()
                    .iter()
                    .filter(|mv| !mv.automatic)
                    .count();
                ui.label(trf(
                    "Your moves: {n} · mistakes: {m}",
                    &[("n", &played), ("m", &self.session.total_mistakes())],
                ));
                ui.label(if self.session.hinted() {
                    tr("With hint")
                } else {
                    tr("Without hint")
                });
            } else {
                let correct = self.session.attempts().iter().filter(|a| a.correct).count();
                ui.label(trf(
                    "Tasks: {n} · correct: {c} · wrong attempts: {w} · with hint: {h}",
                    &[
                        ("n", &self.completed_targets),
                        ("c", &correct),
                        ("w", &self.session.total_mistakes()),
                        (
                            "h",
                            &self.session.attempts().iter().filter(|a| a.hinted).count(),
                        ),
                    ],
                ));
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if self.is_path_practice() {
                    if crate::widgets::primary(ui, tr("Next variation"))
                        .on_hover_text("Enter")
                        .clicked()
                    {
                        self.next_path();
                    }
                    if ui.button(tr("Repeat path")).clicked() {
                        self.restart();
                    }
                } else if crate::widgets::primary(ui, tr("↻ Again"))
                    .on_hover_text("Enter")
                    .clicked()
                {
                    self.restart();
                }
                if ui.button(tr("Back to repertoire")).clicked() {
                    actions.push(TrainingAction::Back);
                }
            });
        });
    }

    fn notation(&mut self, ui: &mut Ui) {
        let mut clicked = None;
        let mut marks: std::collections::HashMap<(String, String), (u32, bool)> =
            std::collections::HashMap::new();
        for attempt in self.session.attempts() {
            let mark = marks
                .entry((
                    attempt.position_fen4.clone(),
                    if self.mode == TrainingMode::Both {
                        String::new()
                    } else {
                        attempt.expected_uci.clone()
                    },
                ))
                .or_default();
            mark.0 += u32::from(!attempt.correct);
            mark.1 |= attempt.hinted;
        }
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let path = &self.session.path()[self.task_start..];
                let mut index = 0;
                while index < path.len() {
                    let white = self
                        .positions
                        .get(index)
                        .is_some_and(|p| p.turn() == Color::White);
                    let number = self.positions.get(index).map_or(1, |p| p.fullmoves().get());
                    ui.horizontal_wrapped(|ui| {
                        ui.add_sized(
                            [36.0, 24.0],
                            egui::Label::new(
                                RichText::new(if white {
                                    format!("{number}.")
                                } else {
                                    format!("{number}…")
                                })
                                .weak(),
                            ),
                        );
                        for color in [Color::White, Color::Black] {
                            if index >= path.len() {
                                break;
                            }
                            let is_white = self
                                .positions
                                .get(index)
                                .is_some_and(|p| p.turn() == Color::White);
                            if is_white != (color == Color::White) {
                                continue;
                            }
                            let played = &path[index];
                            let global = self.task_start + index;
                            let mut text = played.san.clone();
                            if let Some((mistakes, hinted)) = marks.get(&(
                                played.position_before.clone(),
                                if self.mode == TrainingMode::Both {
                                    String::new()
                                } else {
                                    played.uci.clone()
                                },
                            )) {
                                if *mistakes > 0 {
                                    text.push_str(&format!(" ×{mistakes}"));
                                }
                                if *hinted {
                                    text.push_str(" *");
                                }
                            }
                            let response = ui.selectable_label(self.view_ply == global + 1, text);
                            let was_clicked = response.clicked();
                            if played.automatic {
                                response.on_hover_text(tr("Automatic move/context"));
                            }
                            if was_clicked {
                                clicked = Some(global + 1 - self.task_start);
                            }
                            index += 1;
                        }
                    });
                }
            });
        if let Some(ply) = clicked {
            self.view_ply = ply;
        }
    }

    fn save_strip(&mut self, ui: &mut Ui) {
        let Some(error) = self.save_error.clone() else {
            return;
        };
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, ui.visuals().error_fg_color))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(trf("Could not save changes: {error}", &[("error", &error)]))
                        .color(ui.visuals().error_fg_color),
                );
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("Retry")).clicked() {
                        self.save();
                    }
                    if self.confirm_discard {
                        if ui.button(tr("Leave without saving")).clicked() {
                            self.discard_requested = true;
                        }
                    } else if ui.button(tr("Back to repertoire")).clicked() {
                        self.confirm_discard = true;
                    }
                });
                if self.confirm_discard {
                    ui.label(
                        RichText::new(tr(
                            "Only the events that could not be saved will be discarded",
                        ))
                        .weak(),
                    );
                }
            });
    }

    pub fn show(&mut self, ui: &mut Ui, settings: &Settings, name: &str) -> Vec<TrainingAction> {
        let mut actions = Vec::new();
        if let Some(flash) = &self.flash {
            let left = flash
                .until
                .saturating_duration_since(web_time::Instant::now());
            if left.is_zero() {
                self.flash = None;
            } else {
                ui.ctx().request_repaint_after(left);
            }
        }
        self.keyboard(ui, &mut actions);
        egui::Panel::top("repertoire_training_header").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(format!("← {}", tr("Back to repertoire")))
                    .clicked()
                {
                    if self.save_error.is_some() {
                        self.confirm_discard = true;
                    } else {
                        actions.push(TrainingAction::Back);
                    }
                }
                ui.separator();
                ui.label(mode_label(self.mode));
            });
            ui.add(egui::Label::new(RichText::new(name).heading()).truncate())
                .on_hover_text(name);
            ui.add_space(2.0);
        });
        let width = ui.available_width();
        let height = ui.available_height();
        let stacked = width < 900.0 && height > width;
        let side = if stacked {
            egui::Panel::bottom("repertoire_training_side_stacked")
                .resizable(true)
                .default_size(320.0)
                .size_range(300.0..=(height - 220.0).max(300.0))
                .show(ui, |ui| self.side(ui, &mut actions))
        } else {
            egui::Panel::right("repertoire_training_side")
                .resizable(true)
                .default_size((width * 0.36).clamp(280.0, 430.0))
                .size_range(240.0..=(width - 220.0).max(240.0))
                .show(ui, |ui| self.side(ui, &mut actions))
        };
        crate::widgets::check_panel("repertoire_training_side", &side.response);
        egui::CentralPanel::default().show(ui, |ui| self.board(ui, settings, &mut actions));
        if self.save_error.is_some() && !self.discard_requested {
            if actions
                .iter()
                .any(|action| matches!(action, TrainingAction::Back))
            {
                self.confirm_discard = true;
            }
            actions.retain(|action| !matches!(action, TrainingAction::Back));
        }
        if std::mem::take(&mut self.discard_requested) {
            actions.push(TrainingAction::Back);
        }
        actions
    }

    fn side(&mut self, ui: &mut Ui, actions: &mut Vec<TrainingAction>) {
        let notation_height = (ui.available_height() - 230.0).max(120.0);
        egui::ScrollArea::vertical()
            .id_salt("repertoire_training_controls")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                self.save_strip(ui);
                if self.is_finished() {
                    self.result(ui, actions);
                } else {
                    self.task_card(ui);
                    self.user_card(ui);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (label, ply) in [
                        ("⏮", 0),
                        ("◀", self.view_ply.saturating_sub(1)),
                        (
                            "▶",
                            (self.view_ply + 1).min(self.positions.len().saturating_sub(1)),
                        ),
                        ("⏭", self.positions.len().saturating_sub(1)),
                    ] {
                        if ui
                            .add_sized([30.0, 26.0], egui::Button::new(label))
                            .clicked()
                        {
                            self.view_ply = ply;
                        }
                    }
                    if ui
                        .add_sized([30.0, 26.0], egui::Button::new("🔃"))
                        .on_hover_text(tr("Flip board (F)"))
                        .clicked()
                    {
                        self.flipped = !self.flipped;
                    }
                    if !self.is_finished() && ui.button(tr("↻ Restart")).clicked() {
                        self.restart();
                    }
                });
                if self.view_ply + 1 < self.positions.len()
                    && ui
                        .link(tr("Viewing an earlier position · back to training — End"))
                        .clicked()
                {
                    self.go_end();
                }
                ui.separator();
                ui.allocate_ui(vec2(ui.available_width(), notation_height), |ui| {
                    self.notation(ui);
                });
            });
    }

    pub fn debug_hint(&mut self) {
        if let Some(candidate) = self.session.hint(now()) {
            self.hint = move_squares(&candidate);
            self.save();
        }
    }

    pub fn debug_moves(&mut self, moves: &str) {
        for text in moves.split_whitespace() {
            match text {
                "hint" => self.debug_hint(),
                "next" => self.next_path(),
                "repeat" => self.restart(),
                _ => {}
            }
            if matches!(text, "hint" | "next" | "repeat") {
                continue;
            }
            self.flash = None;
            self.refresh_positions();
            let mv = text
                .parse::<UciMove>()
                .ok()
                .and_then(|uci| uci.to_move(self.session.position()).ok());
            if let Some(mv) = mv {
                self.play(mv);
            } else {
                self.message = Some(tr("Illegal move in the current position").to_owned());
            }
        }
        self.refresh_positions();
        self.go_end();
    }
}

#[cfg(test)]
fn initial_policy(
    repertoire: &Repertoire,
    scope: &TrainingScope,
    mode: TrainingMode,
    queue_mode: QueueMode,
    stats: &StatsData,
) -> SrsPolicy {
    if queue_mode != QueueMode::SpacedRepetition {
        return SrsPolicy::Due;
    }
    let tree = RepertoireTree::new(repertoire);
    let due = stats.srs_queue(&tree, &repertoire.id, mode, now(), scope, SrsPolicy::Due);
    if due.is_empty() {
        SrsPolicy::Introduce
    } else {
        SrsPolicy::Due
    }
}

fn target_count(
    repertoire: &Repertoire,
    scope: &TrainingScope,
    mode: TrainingMode,
    queue_mode: QueueMode,
    policy: SrsPolicy,
    stats: &StatsData,
) -> usize {
    if mode == TrainingMode::Both || queue_mode == QueueMode::Practice {
        return usize::MAX;
    }
    let tree = RepertoireTree::new(repertoire);
    if queue_mode == QueueMode::SpacedRepetition {
        stats
            .srs_queue(&tree, &repertoire.id, mode, now(), scope, policy)
            .len()
    } else {
        stats
            .scoped_queue(&tree, &repertoire.id, mode, queue_mode, now(), scope)
            .len()
    }
}

fn card(ui: &Ui, alert: bool) -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .corner_radius(4)
        .stroke(egui::Stroke::new(
            if alert { 1.5 } else { 1.0 },
            if alert {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke.color
            },
        ))
}

fn move_squares(candidate: &Candidate) -> Option<(Square, Square)> {
    Some((
        candidate.mv.from().unwrap_or_else(|| candidate.mv.to()),
        crate::board::ui_target(&candidate.mv),
    ))
}

pub fn mode_label(mode: TrainingMode) -> &'static str {
    tr(match mode {
        TrainingMode::White => "White",
        TrainingMode::Black => "Black",
        TrainingMode::Both => "Both",
    })
}

pub fn now() -> i64 {
    chrono::Local::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::parse_str;

    fn training(pgn: &str, mode: TrainingMode, name: &str) -> TrainingUi {
        training_queue(
            pgn,
            mode,
            name,
            QueueMode::ReviewInOrder,
            TrainingScope::whole(),
        )
    }
    fn training_queue(
        pgn: &str,
        mode: TrainingMode,
        name: &str,
        queue: QueueMode,
        scope: TrainingScope,
    ) -> TrainingUi {
        let game = parse_str(pgn).game;
        let mut library = trainer::repertoire::Library::default();
        let id = library.create("Test", &game).unwrap();
        let store = StatsStore::new(std::env::temp_dir().join(format!(
            "cp-desktop-training-{}-{name}-{}.json",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )));
        TrainingUi::new(
            library.repertoire(&id).unwrap(),
            scope,
            mode,
            queue,
            StatsData::default(),
            store,
            false,
        )
        .unwrap()
    }

    fn assert_board_matches_session(training: &TrainingUi) {
        let shown = training.positions.last().unwrap();
        assert_eq!(
            trainer::repertoire::fen4(shown),
            training.session.position_key()
        );
        assert_eq!(training.view_ply + 1, training.positions.len());
        assert_eq!(shown.fullmoves(), training.session.position().fullmoves());
        assert_eq!(shown.halfmoves(), training.session.position().halfmoves());
    }

    #[test]
    fn practice_accepts_users_branch_and_keeps_board_in_sync_for_both_single_colors() {
        let pgn = "1. e4 (1. d4 d5 2. c4) e5 (1... c5 2. Nf3) 2. Bc4 *";
        let mut white = training_queue(
            pgn,
            TrainingMode::White,
            "practice-white",
            QueueMode::Practice,
            TrainingScope::whole(),
        );
        assert!(white.target_available());
        assert!(white.session.may_hint());
        white.debug_moves("d2d4 c2c4");
        assert!(white.is_finished());
        assert_eq!(white.session.total_mistakes(), 0);
        assert_board_matches_session(&white);
        white.debug_moves("next");
        assert!(!white.is_finished());
        assert_board_matches_session(&white);

        let mut black = training_queue(
            pgn,
            TrainingMode::Black,
            "practice-black",
            QueueMode::Practice,
            TrainingScope::whole().with_context(false),
        );
        assert!(black.target_available());
        black.debug_moves("c7c5");
        assert!(black.is_finished());
        assert_board_matches_session(&black);
        black.debug_moves("repeat");
        assert!(!black.is_finished());
        assert_board_matches_session(&black);
    }

    #[test]
    fn replay_preserves_fen_counters_and_move_numbers() {
        let mut training = training(
            "[SetUp \"1\"]\n[FEN \"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 7 17\"]\n\n17. Nf3 Nf6 18. Nc3 Nc6 *",
            TrainingMode::Both,
            "fen-counters",
        );
        training.debug_moves("g1f3 g8f6 b1c3 b8c6");
        assert!(training.is_finished());
        assert_board_matches_session(&training);
        assert_eq!(
            training
                .positions
                .iter()
                .map(|p| p.fullmoves().get())
                .collect::<Vec<_>>(),
            vec![17, 17, 18, 18, 19]
        );
        assert_eq!(training.positions[0].halfmoves(), 7);
        assert_eq!(training.positions.last().unwrap().halfmoves(), 11);
    }

    #[test]
    fn explicit_due_does_not_silently_introduce_new_targets() {
        let mut training = training("1. e4 e5 *", TrainingMode::White, "explicit-due");
        let due = TrainingUi::new_with_policy(
            &training.repertoire,
            TrainingScope::whole(),
            TrainingMode::White,
            QueueMode::SpacedRepetition,
            StatsData::default(),
            training.store.clone(),
            false,
            SrsPolicy::Due,
        )
        .unwrap();
        assert!(!due.target_available());
        training.debug_moves("e2e4");
        assert!(training.is_finished());
    }

    #[test]
    fn context_hidden_sections_replay_from_their_actual_start() {
        let seed = training(
            "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
            TrainingMode::White,
            "hidden-context",
        );
        let mut training = TrainingUi::new(
            &seed.repertoire,
            TrainingScope::from_prefix(vec!["e2e4".into(), "e7e5".into(), "g1f3".into()])
                .with_context(false),
            TrainingMode::White,
            QueueMode::ReviewInOrder,
            StatsData::default(),
            seed.store.clone(),
            false,
        )
        .unwrap();
        training.debug_moves("g1f3");
        assert_board_matches_session(&training);
        assert_eq!(training.positions[0].fullmoves().get(), 3);
        training.debug_moves("f1b5");
        assert!(training.is_finished());
        assert_board_matches_session(&training);
        assert_eq!(training.positions.last().unwrap().fullmoves().get(), 4);
    }

    #[test]
    fn one_side_targets_have_independent_paths_and_current_boards() {
        let mut training = training(
            "1. e4 c5 2. Nf3 Nc6 3. d4 cxd4 *",
            TrainingMode::White,
            "sections",
        );
        training.debug_moves("e2e4");
        assert_eq!(training.session.target().unwrap().expected_uci, "g1f3");
        assert_eq!(training.task_start, 2);
        assert_board_matches_session(&training);

        training.debug_moves("g1f3");
        assert_eq!(training.session.target().unwrap().expected_uci, "d2d4");
        assert_eq!(training.task_start, 6);
        assert_board_matches_session(&training);

        training.debug_moves("d2d4");
        assert!(training.is_finished());
        assert_board_matches_session(&training);
        let section = &training.session.path()[training.task_start..];
        assert_eq!(section.last().unwrap().uci, "c5d4");
    }

    #[test]
    fn pgn_answers_one_side_but_both_has_no_automatic_moves() {
        let pgn = "1. e4 e5 2. Nf3 Nc6 *";
        let mut black = training(pgn, TrainingMode::Black, "pgn-black");
        assert_eq!(black.session.path().len(), 1);
        assert_eq!(black.session.path()[0].uci, "e2e4");
        assert!(black.session.path()[0].automatic);
        black.debug_moves("e7e5");
        assert!(
            black
                .session
                .path()
                .iter()
                .any(|played| { played.uci == "g1f3" && played.automatic })
        );
        assert!(
            black
                .session
                .path()
                .iter()
                .filter(|played| !played.automatic)
                .all(|played| played.uci == "e7e5")
        );

        let mut both = training(pgn, TrainingMode::Both, "pgn-both");
        assert!(both.session.path().is_empty());
        both.debug_moves("e2e4 e7e5 g1f3 b8c6");
        assert!(both.is_finished());
        assert!(both.session.path().iter().all(|played| !played.automatic));
    }

    #[test]
    fn both_next_path_and_result_finish_on_the_actual_last_move() {
        let mut training = training(
            "1. e4 e5 (1... c5) 2. Nf3 (2. c4) *",
            TrainingMode::Both,
            "next-path",
        );
        training.debug_moves("e2e4 c7c5");
        assert!(training.is_finished());
        assert_board_matches_session(&training);

        training.debug_moves("next e2e4 e7e5 c2c4");
        assert!(training.is_finished());
        assert_eq!(training.session.path().last().unwrap().uci, "c2c4");
        assert_board_matches_session(&training);
    }

    #[test]
    fn late_mainline_scope_end_is_accepted() {
        let game =
            parse_str("1. e4 e5 (1... c5) 2. Nf3 (2. c4) Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O *").game;
        let mut library = trainer::repertoire::Library::default();
        let id = library.create("Scoped", &game).unwrap();
        let repertoire = library.repertoire(&id).unwrap();
        let end = [
            "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6",
        ]
        .map(str::to_owned)
        .to_vec();
        let tree = RepertoireTree::new(repertoire);
        let actual = tree
            .game()
            .mainline()
            .into_iter()
            .filter_map(|node| tree.game().game_move(node).and_then(|mv| mv.uci()))
            .take(end.len())
            .collect::<Vec<_>>();
        assert_eq!(actual, end);
        assert!(tree.node_at_path(&end).is_some());
        let scope =
            TrainingScope::between(["e2e4", "e7e5", "g1f3"].map(str::to_owned).to_vec(), end);
        TrainingSession::scoped(
            repertoire,
            scope,
            TrainingMode::White,
            QueueMode::ReviewInOrder,
            StatsData::default(),
            0,
        )
        .unwrap();
    }
}
