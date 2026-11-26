//! The «Lines» screen of opening training (spec §6.3): the repertoire's tree, read only,
//! to choose what to train and which branches to leave out.

use std::collections::BTreeSet;
use std::sync::mpsc;

use chess_core::shakmaty::Color;
use chess_core::{Game, NodeId};
use egui::{Key, KeyboardShortcut, Modifiers, RichText, Ui, vec2};
use trainer::repertoire::{
    LearningData, LearningItemKey, LearningSide, Library, Repertoire, RepertoireId, StatsScope,
    active_learning_memberships,
};

use crate::board::BoardState;
use crate::i18n::{tr, trf};
use crate::opening_layout::{self as layout, GAP, pgn_annotations_at};
use crate::settings::Settings;

const SELECTED_H: f32 = 92.0;
const TOOL_H: f32 = 28.0;
const MAIN_W: f32 = 170.0;

pub enum LinesAction {
    Back,
    OpenSettings,
    /// Train the lines through this move (empty: every line).
    Train {
        through: Vec<String>,
        label: String,
    },
    /// Switch a branch off (`true`) or on again, from the move at this path.
    SetDisabled {
        path: Vec<String>,
        disabled: bool,
    },
}

/// The repertoire's own moves with their paths and which of them are learnt; computed
/// off the frame.
struct Counts {
    moves: Vec<(Vec<String>, LearningItemKey)>,
    learnt: BTreeSet<LearningItemKey>,
}

pub struct OpeningLinesUi {
    id: RepertoireId,
    name: String,
    side: LearningSide,
    repertoire: Repertoire,
    game: Game,
    lines: Vec<Vec<String>>,
    cursor: NodeId,
    scroll: bool,
    orientation: Color,
    board: BoardState,
    counts: Option<Counts>,
    counting: Option<mpsc::Receiver<Result<Counts, String>>>,
    error: Option<String>,
}

impl OpeningLinesUi {
    pub fn new(
        library: &Library,
        learning: &LearningData,
        id: RepertoireId,
        side: LearningSide,
        cursor: Option<Vec<String>>,
    ) -> Option<Self> {
        let repertoire = library.repertoire(&id)?.clone();
        let game = repertoire.game();
        let cursor = cursor
            .and_then(|path| layout::node_at(&game, &path))
            .unwrap_or(NodeId::ROOT);
        let lines = layout::lines(&game, |path| repertoire.is_prefix_disabled(path));
        let mut this = Self {
            id,
            name: repertoire.name.clone(),
            side,
            repertoire,
            game,
            lines,
            cursor,
            scroll: true,
            orientation: layout::orientation(side),
            board: BoardState::default(),
            counts: None,
            counting: None,
            error: None,
        };
        this.count(library, learning);
        Some(this)
    }

    pub fn id(&self) -> &RepertoireId {
        &self.id
    }

    pub fn cursor_path(&self) -> Vec<String> {
        layout::uci_path(&self.game, self.cursor)
    }

    /// The library changed (a branch switched off or on): recount, keep the move.
    pub fn refresh(&mut self, library: &Library, learning: &LearningData) {
        if let Some(repertoire) = library.repertoire(&self.id) {
            self.repertoire = repertoire.clone();
            self.name = repertoire.name.clone();
            self.lines = layout::lines(&self.game, |path| repertoire.is_prefix_disabled(path));
        }
        self.count(library, learning);
    }

    fn count(&mut self, library: &Library, learning: &LearningData) {
        let library = library.clone();
        let learning = learning.clone();
        let scope = StatsScope::for_repertoire(self.id.clone(), self.side, layout::now());
        let (tx, rx) = mpsc::channel();
        self.counting = Some(rx);
        if let Err(error) = crate::platform::spawn("repertoire-lines-count", move || {
            let result = active_learning_memberships(&library, &scope)
                .map(|memberships| {
                    let learnt = memberships
                        .iter()
                        .filter(|membership| {
                            learning
                                .item(&membership.key)
                                .is_some_and(|item| item.is_learnt())
                        })
                        .map(|membership| membership.key.clone())
                        .collect();
                    Counts {
                        moves: memberships
                            .into_iter()
                            .map(|membership| (membership.path, membership.key))
                            .collect(),
                        learnt,
                    }
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        }) {
            self.counting = None;
            self.error = Some(error.to_string());
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.counting else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(counts)) => {
                self.counts = Some(counts);
                self.counting = None;
            }
            Ok(Err(error)) => {
                self.error = Some(error);
                self.counting = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.counting = None,
        }
    }

    /// The prefix that switches this path off, if any.
    fn disabled_by(&self, path: &[String]) -> Option<Vec<String>> {
        self.repertoire
            .disabled_prefixes
            .iter()
            .filter(|prefix| path.starts_with(prefix))
            .min_by_key(|prefix| prefix.len())
            .cloned()
    }

    fn goto(&mut self, node: NodeId) {
        if self.game.node(node).is_some() {
            self.cursor = node;
            self.scroll = true;
        }
    }

    fn keyboard(&mut self, ui: &Ui, actions: &mut Vec<LinesAction>) {
        if ui.ctx().egui_wants_keyboard_input() || crate::app::overlay_open(ui.ctx()) {
            return;
        }
        let mut command = None;
        ui.input_mut(|input| {
            for (key, name) in [
                (Key::ArrowLeft, "back"),
                (Key::ArrowRight, "forward"),
                (Key::ArrowUp, "previous"),
                (Key::ArrowDown, "next"),
                (Key::Home, "home"),
                (Key::End, "end"),
                (Key::F, "flip"),
                (Key::Enter, "train"),
                (Key::Escape, "back_screen"),
            ] {
                if command.is_none()
                    && (input.consume_key(Modifiers::NONE, key)
                        || input.consume_key(Modifiers::SHIFT, key))
                {
                    command = Some(name);
                }
            }
            if command.is_none()
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W))
            {
                command = Some("back_screen");
            }
        });
        let game = &self.game;
        let cursor = self.cursor;
        match command {
            Some("back") => {
                if let Some(parent) = game.parent(cursor) {
                    self.goto(parent);
                }
            }
            Some("forward") => {
                if let Some(next) = game.main_child(cursor) {
                    self.goto(next);
                }
            }
            Some("previous") => {
                if let Some(other) = game.prev_sibling(cursor) {
                    self.goto(other);
                }
            }
            Some("next") => {
                if let Some(other) = game.next_sibling(cursor) {
                    self.goto(other);
                }
            }
            Some("home") => self.goto(NodeId::ROOT),
            Some("end") => {
                let end = game.line_end(cursor);
                self.goto(end);
            }
            Some("flip") => self.orientation = self.orientation.other(),
            Some("train") => actions.push(self.train_action()),
            Some("back_screen") => actions.push(LinesAction::Back),
            _ => {}
        }
    }

    fn train_action(&self) -> LinesAction {
        let through = self.cursor_path();
        let label = self.path_label(self.cursor);
        LinesAction::Train { through, label }
    }

    /// "… 4. Ba4 Nf6": the moves to a node in the notation of the application.
    fn path_label(&self, node: NodeId) -> String {
        layout::path_text(&self.game, node)
    }

    fn below(&self, path: &[String]) -> Option<(usize, usize, usize)> {
        let counts = self.counts.as_ref()?;
        let lines = self
            .lines
            .iter()
            .filter(|line| line.starts_with(path))
            .count();
        let mine: Vec<_> = counts
            .moves
            .iter()
            .filter(|(move_path, _)| move_path.starts_with(path))
            .collect();
        let learnt = mine
            .iter()
            .filter(|(_, key)| counts.learnt.contains(key))
            .count();
        Some((lines, mine.len(), learnt))
    }

    fn selected_block(&self, ui: &mut Ui, actions: &mut Vec<LinesAction>) {
        let path = self.cursor_path();
        let disabled = self.disabled_by(&path);
        layout::fixed_block(ui, SELECTED_H, |ui| {
            let inner = layout::card_inner(ui, SELECTED_H);
            layout::card(ui, false, false).show(ui, |ui| {
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                ui.spacing_mut().item_spacing.y = 4.0;
                if self.cursor == NodeId::ROOT {
                    layout::line(ui, RichText::new(tr("Start position")).strong());
                } else {
                    layout::line_left(ui, &self.path_label(self.cursor), true);
                }
                if disabled.is_some() {
                    layout::line(
                        ui,
                        RichText::new(tr("Not trained")).color(ui.visuals().warn_fg_color),
                    );
                } else {
                    let text = match self.below(&path) {
                        Some((lines, mine, learnt)) => trf(
                            "Below: {n} lines · {y} your moves · learned {x}",
                            &[("n", &lines), ("y", &mine), ("x", &learnt)],
                        ),
                        None => tr("Counting moves…").to_owned(),
                    };
                    layout::line(ui, RichText::new(text).weak());
                }
                ui.horizontal(|ui| {
                    let label = if self.cursor == NodeId::ROOT {
                        tr("Review all lines")
                    } else {
                        tr("Train this branch")
                    };
                    let width = MAIN_W.min(ui.available_width() / 2.0 - 4.0);
                    let can_train = disabled.is_none() && !self.lines.is_empty();
                    if ui
                        .add_enabled_ui(can_train, |ui| {
                            crate::widgets::primary_wide(ui, label, width)
                        })
                        .inner
                        .on_hover_text("Enter")
                        .clicked()
                    {
                        actions.push(self.train_action());
                    }
                    if self.cursor != NodeId::ROOT {
                        let (label, action) = match &disabled {
                            Some(prefix) => (
                                tr("Train again"),
                                LinesAction::SetDisabled {
                                    path: prefix.clone(),
                                    disabled: false,
                                },
                            ),
                            None => (
                                tr("Don't train"),
                                LinesAction::SetDisabled {
                                    path: path.clone(),
                                    disabled: true,
                                },
                            ),
                        };
                        if ui
                            .add(
                                egui::Button::new(label)
                                    .truncate()
                                    .min_size(vec2(width, 30.0)),
                            )
                            .clicked()
                        {
                            actions.push(action);
                        }
                    }
                });
            });
        });
    }

    fn side(&mut self, ui: &mut Ui, actions: &mut Vec<LinesAction>) {
        let rect = ui.available_rect_before_wrap();
        let footer_h = TOOL_H + 12.0;
        let footer = egui::Rect::from_min_max(
            egui::pos2(rect.left(), (rect.bottom() - footer_h).max(rect.top())),
            rect.max,
        );
        let body = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), footer.top()));
        let mut clicked = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.set_clip_rect(body.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing.y = GAP;
            ui.add_space(4.0);
            let opening = layout::opening_text(&self.game, self.cursor);
            layout::fixed_block(ui, 20.0, |ui| {
                layout::line(ui, RichText::new(opening).weak());
            });
            let tree = (ui.available_height() - SELECTED_H - GAP).max(0.0);
            let scroll = std::mem::take(&mut self.scroll);
            let game = &self.game;
            let cursor = self.cursor;
            let response = layout::fixed_block(ui, tree, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("opening_lines_tree")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        crate::notation::Notation {
                            game,
                            cursor,
                            scroll_to_cursor: scroll,
                            marks: None,
                            preview: None,
                            translations: None,
                            editor: None,
                            read_only: true,
                            dim_until: None,
                            mistakes: None,
                        }
                        .show(ui)
                    })
                    .inner
            });
            clicked = response.clicked;
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            self.selected_block(ui, actions);
        });
        if let Some(node) = clicked {
            self.cursor = node;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
            ui.separator();
            ui.horizontal(|ui| {
                let icon = |s: &str| {
                    egui::Button::new(RichText::new(s).size(18.0)).min_size(vec2(0.0, TOOL_H))
                };
                let game = &self.game;
                let mut target = None;
                if ui.add(icon("⏮")).clicked() {
                    target = Some(NodeId::ROOT);
                }
                if ui.add(icon("◀")).clicked() {
                    target = game.parent(self.cursor);
                }
                if ui.add(icon("▶")).clicked() {
                    target = game.main_child(self.cursor);
                }
                if ui.add(icon("⏭")).clicked() {
                    target = Some(game.line_end(self.cursor));
                }
                ui.separator();
                if ui
                    .add(icon("🔃"))
                    .on_hover_text(tr("Flip board (F)"))
                    .clicked()
                {
                    self.orientation = self.orientation.other();
                }
                if let Some(target) = target {
                    self.goto(target);
                }
            });
        });
    }

    pub fn show(&mut self, ui: &mut Ui, settings: &Settings) -> Vec<LinesAction> {
        self.poll();
        if self.counting.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut actions = Vec::new();
        self.keyboard(ui, &mut actions);
        let back = if ui.available_width() < 600.0 {
            "⬅"
        } else {
            tr("⬅ Repertoires")
        };
        let subtitle = trf(
            "Lines · {n} lines · {y} your moves",
            &[
                ("n", &self.lines.len()),
                (
                    "y",
                    &self
                        .counts
                        .as_ref()
                        .map_or_else(|| "…".to_owned(), |counts| counts.moves.len().to_string()),
                ),
            ],
        );
        let name = self.name.clone();
        let mut header = (false, false);
        layout::screen(
            ui,
            self,
            |_, ui| header = crate::widgets::section_header_sub(ui, back, &name, &subtitle),
            |this, ui| this.side(ui, &mut actions),
            |this, ui| {
                let position = layout::position_of(&this.game, this.cursor);
                let marks = pgn_annotations_at(&this.game, this.cursor);
                let last = layout::last_move(&this.game, this.cursor);
                layout::still_board(
                    ui,
                    &mut this.board,
                    settings,
                    &position,
                    this.orientation,
                    last,
                    &marks,
                );
            },
        );
        if header.0 {
            actions.push(LinesAction::Back);
        }
        if header.1 {
            actions.push(LinesAction::OpenSettings);
        }
        actions
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn debug_select(&mut self, path: &[String]) {
        if let Some(node) = layout::node_at(&self.game, path) {
            self.goto(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_ui(pgn: &str) -> (Library, OpeningLinesUi) {
        let mut library = Library::default();
        let id = library
            .create("Lines", &chess_core::parse_str(pgn).game)
            .unwrap();
        let mut ui = OpeningLinesUi::new(
            &library,
            &LearningData::default(),
            id,
            LearningSide::White,
            None,
        )
        .unwrap();
        let deadline = web_time::Instant::now() + std::time::Duration::from_secs(5);
        while ui.counting.is_some() && web_time::Instant::now() < deadline {
            ui.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        (library, ui)
    }

    #[test]
    fn counts_below_a_move_and_branches_switched_off() {
        let (mut library, mut ui) = lines_ui("1. e4 e5 (1... c5 2. Nf3) 2. Nf3 (2. Bc4) *");
        assert_eq!(ui.below(&[]), Some((3, 4, 0)));
        let c5 = ["e2e4".to_owned(), "c7c5".to_owned()];
        assert_eq!(ui.below(&c5), Some((1, 1, 0)));
        ui.debug_select(&c5);
        assert_eq!(ui.cursor_path(), c5);
        assert_eq!(ui.path_label(ui.cursor), "1. e4 c5");
        assert!(matches!(
            ui.train_action(),
            LinesAction::Train { through, .. } if through == c5
        ));
        let id = ui.id().clone();
        library
            .repertoire_mut(&id)
            .unwrap()
            .set_prefix_disabled(&c5, true)
            .unwrap();
        ui.refresh(&library, &LearningData::default());
        let deadline = web_time::Instant::now() + std::time::Duration::from_secs(5);
        while ui.counting.is_some() && web_time::Instant::now() < deadline {
            ui.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(ui.lines.len(), 2);
        assert_eq!(
            ui.disabled_by(&[c5[0].clone(), c5[1].clone(), "g1f3".into()]),
            Some(c5.to_vec())
        );
        assert_eq!(ui.below(&[]), Some((2, 3, 0)));
    }
}
