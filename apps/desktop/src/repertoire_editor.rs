use chess_core::shakmaty::{Chess, Color, Position, san::SanPlus, uci::UciMove};
use chess_core::{Game, MoveKind, NodeId};
use egui::{RichText, Ui};
use trainer::repertoire::{
    LearningData, LearningItemKey, LearningSide, Library, LibraryStore, MoveLearningAnalytics,
    MoveLearningStatus, Repertoire, RepertoireError, RepertoireId, RepertoireTree, SrsSettings,
    StatsScope, scoped_move_learning_analytics,
};

use crate::board::{BoardState, BoardView};
use crate::i18n::tr;
use crate::settings::Settings;

#[derive(Debug)]
pub enum RepertoireEditorAction {
    Close,
    Cancel,
    Applied {
        repertoire_id: RepertoireId,
        library: Library,
    },
    Error(String),
}

pub struct RepertoireEditorUi {
    original: Library,
    draft: Library,
    repertoire_id: RepertoireId,
    tree: RepertoireTree,
    selected: NodeId,
    board: BoardState,
    learning: Option<LearningData>,
    analytics_cache: Option<Result<Option<MoveLearningAnalytics>, String>>,
    error: Option<String>,
}

impl RepertoireEditorUi {
    pub fn new(library: &Library, repertoire_id: RepertoireId) -> Result<Self, RepertoireError> {
        let repertoire = library
            .repertoire(&repertoire_id)
            .ok_or_else(|| RepertoireError::NotFound(repertoire_id.clone()))?;
        let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
        let selected = tree.root();
        Ok(Self {
            original: library.clone(),
            draft: library.clone(),
            repertoire_id,
            tree,
            selected,
            board: BoardState::default(),
            learning: None,
            analytics_cache: None,
            error: None,
        })
    }

    pub fn set_learning(&mut self, learning: &LearningData) {
        self.learning = Some(learning.clone());
        self.analytics_cache = None;
    }

    pub fn has_unsaved(&self) -> bool {
        self.draft.repertoire(&self.repertoire_id) != self.original.repertoire(&self.repertoire_id)
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        settings: &Settings,
        library_store: &LibraryStore,
    ) -> Vec<RepertoireEditorAction> {
        let mut actions = Vec::new();
        let mut commands = Vec::new();
        self.show_header(ui, &mut commands);
        let available = ui.available_rect_before_wrap();
        if available.width() < 720.0 {
            let board_bottom = available.top() + (available.height() * 0.46).max(100.0);
            let board_rect = egui::Rect::from_min_max(
                available.min,
                egui::pos2(available.right(), board_bottom.min(available.bottom())),
            );
            let panel_rect = egui::Rect::from_min_max(
                egui::pos2(available.left(), board_rect.bottom()),
                available.max,
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(board_rect), |ui| {
                self.show_board(ui, settings);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(panel_rect), |ui| {
                ui.set_clip_rect(panel_rect);
                self.show_panel(ui, &mut commands);
            });
        } else {
            let panel_width = (available.width() * 0.44).clamp(320.0, 500.0);
            let panel_rect = egui::Rect::from_min_max(
                egui::pos2(available.right() - panel_width, available.top()),
                available.max,
            );
            let board_rect = egui::Rect::from_min_max(
                available.min,
                egui::pos2(panel_rect.left(), available.bottom()),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(board_rect), |ui| {
                self.show_board(ui, settings);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(panel_rect), |ui| {
                ui.set_clip_rect(panel_rect);
                self.show_panel(ui, &mut commands);
            });
        }
        for command in commands {
            match command {
                EditorCommand::Close => actions.push(RepertoireEditorAction::Close),
                EditorCommand::Cancel => {
                    self.cancel();
                    actions.push(RepertoireEditorAction::Cancel);
                }
                EditorCommand::Save => match self.save(library_store) {
                    Ok(library) => actions.push(RepertoireEditorAction::Applied {
                        repertoire_id: self.repertoire_id.clone(),
                        library,
                    }),
                    Err(error) => {
                        self.error = Some(error.clone());
                        actions.push(RepertoireEditorAction::Error(error));
                    }
                },
                EditorCommand::Select(node) => self.select(node),
                EditorCommand::SetMainAnswer => {
                    if let Err(error) = self.set_selected_main_answer() {
                        self.error = Some(error.clone());
                        actions.push(RepertoireEditorAction::Error(error));
                    }
                }
                EditorCommand::ClearMainAnswer => {
                    self.clear_selected_main_answer();
                }
                EditorCommand::SetDisabled(disabled) => {
                    if let Err(error) = self.set_selected_disabled(disabled) {
                        self.error = Some(error.clone());
                        actions.push(RepertoireEditorAction::Error(error));
                    }
                }
                EditorCommand::SetDepth(depth) => {
                    if let Some(repertoire) = self.draft.repertoire_mut(&self.repertoire_id) {
                        repertoire.set_max_plies(depth);
                        self.analytics_cache = None;
                        self.error = None;
                    }
                }
            }
        }
        actions
    }

    fn show_header(&self, ui: &mut Ui, commands: &mut Vec<EditorCommand>) {
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("Close")).clicked() {
                commands.push(EditorCommand::Close);
            }
            if let Some(repertoire) = self.draft.repertoire(&self.repertoire_id) {
                ui.add(egui::Label::new(RichText::new(&repertoire.name).heading()).truncate())
                    .on_hover_text(&repertoire.name);
            }
            if ui.button(tr("Cancel")).clicked() {
                commands.push(EditorCommand::Cancel);
            }
            ui.add_enabled_ui(self.has_unsaved(), |ui| {
                if crate::widgets::primary(ui, tr("Save")).clicked() {
                    commands.push(EditorCommand::Save);
                }
            });
        });
        ui.separator();
    }

    fn show_board(&mut self, ui: &mut Ui, settings: &Settings) {
        let position = self
            .tree
            .position(self.selected)
            .cloned()
            .unwrap_or_default();
        let last_move = self
            .tree
            .game()
            .game_move(self.selected)
            .and_then(|game_move| match game_move.kind {
                MoveKind::Normal(mv) => Some((
                    mv.from().unwrap_or_else(|| mv.to()),
                    crate::board::ui_target(&mv),
                )),
                MoveKind::Null => None,
            });
        let side = ui
            .available_width()
            .min(ui.available_height())
            .max(1.0)
            .floor();
        let available = ui.available_size();
        let offset = egui::vec2(
            ((available.x - side) / 2.0).max(0.0).floor(),
            ((available.y - side) / 2.0).max(0.0).floor(),
        );
        let rect = egui::Rect::from_min_size(ui.cursor().min + offset, egui::vec2(side, side));
        let orientation = settings
            .repertoire_modes
            .get(self.repertoire_id.as_str())
            .map_or(Color::White, |(mode, _)| match mode {
                trainer::repertoire::TrainingMode::Black => Color::Black,
                trainer::repertoire::TrainingMode::White
                | trainer::repertoire::TrainingMode::Both => Color::White,
            });
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            BoardView {
                pos: &position,
                orientation,
                last_move,
                arrows: &[],
                circles: &[],
                badge: None,
                interactive: false,
                show_coords: settings.show_coords,
                theme: settings.board_theme.theme(),
                pieces: settings.piece_set,
                animation: settings.animation.seconds(),
                premove: None,
                auto_queen: false,
                alert: None,
            }
            .show(ui, &mut self.board);
        });
    }

    fn show_panel(&mut self, ui: &mut Ui, commands: &mut Vec<EditorCommand>) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(error) = self.error.clone() {
                    ui.add(
                        egui::Label::new(RichText::new(&error).color(ui.visuals().error_fg_color))
                            .truncate(),
                    )
                    .on_hover_text(error);
                    ui.separator();
                }
                if let Some(repertoire) = self.draft.repertoire(&self.repertoire_id) {
                    let mut depth = repertoire.max_plies;
                    ui.horizontal(|ui| {
                        ui.label(tr("Depth"));
                        if ui
                            .add(egui::DragValue::new(&mut depth).range(0..=u32::MAX))
                            .changed()
                        {
                            commands.push(EditorCommand::SetDepth(depth));
                        }
                        ui.label(tr("plies"));
                    });
                }
                ui.separator();
                self.show_selected(ui, commands);
                ui.separator();
                self.show_analytics(ui);
                ui.separator();
                self.show_tree(ui, commands);
            });
    }

    fn show_selected(&self, ui: &mut Ui, commands: &mut Vec<EditorCommand>) {
        let Some((position_fen4, expected_uci, _)) = self.selected_move_parts() else {
            ui.label(tr("Select a move in the PGN tree"));
            return;
        };
        let game = self.tree.game();
        if let Some(label) = game.move_label(self.selected) {
            ui.strong(crate::san::show(&label));
        }
        let main_answer = game
            .parent(self.selected)
            .and_then(|parent| self.effective_main_answer(parent));
        let is_main = main_answer.as_deref() == Some(expected_uci.as_str());
        if let Some(main_answer) = main_answer {
            let label = game
                .position_before(self.selected)
                .and_then(|position| san_for_uci(position, &main_answer))
                .unwrap_or_else(|| "—".to_owned());
            ui.label(format!("{}: {label}", tr("Main answer")));
        }
        let has_override = self
            .draft
            .repertoire(&self.repertoire_id)
            .and_then(|repertoire| repertoire.main_answer_override(&position_fen4))
            .is_some();
        let disabled = self
            .draft
            .repertoire(&self.repertoire_id)
            .zip(self.tree.path(self.selected))
            .is_some_and(|(repertoire, path)| repertoire.is_prefix_disabled(path));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!is_main, egui::Button::new(tr("Make main answer")))
                .clicked()
            {
                commands.push(EditorCommand::SetMainAnswer);
            }
            if ui
                .add_enabled(has_override, egui::Button::new(tr("Clear override")))
                .clicked()
            {
                commands.push(EditorCommand::ClearMainAnswer);
            }
            if ui
                .button(if disabled {
                    tr("Enable")
                } else {
                    tr("Disable")
                })
                .clicked()
            {
                commands.push(EditorCommand::SetDisabled(!disabled));
            }
        });
        if disabled {
            ui.label(tr("Disabled for training"));
        }
    }

    fn show_tree(&self, ui: &mut Ui, commands: &mut Vec<EditorCommand>) {
        ui.strong(tr("PGN tree"));
        let root = self.tree.root();
        if ui
            .add(
                egui::Button::new(tr("Initial position"))
                    .selected(self.selected == root)
                    .truncate(),
            )
            .clicked()
        {
            commands.push(EditorCommand::Select(root));
        }
        for row in tree_rows(&self.tree) {
            let Some(label) = self.tree.game().move_label(row.node) else {
                continue;
            };
            let mut label = crate::san::show(&label).into_owned();
            if row.variation {
                label = format!("↳ {label}");
            }
            if self
                .tree
                .game()
                .parent(row.node)
                .and_then(|parent| self.effective_main_answer(parent))
                .zip(self.tree.game().game_move(row.node).and_then(|mv| mv.uci()))
                .is_some_and(|(main, uci)| main == uci)
            {
                label.push_str(&format!(" · {}", tr("main")));
            }
            if self
                .draft
                .repertoire(&self.repertoire_id)
                .zip(self.tree.path(row.node))
                .is_some_and(|(repertoire, path)| repertoire.is_prefix_disabled(path))
            {
                label.push_str(&format!(" · {}", tr("disabled")));
            }
            ui.horizontal(|ui| {
                let indentation = (row.indentation as f32 * 14.0).min(ui.available_width() * 0.35);
                ui.add_space(indentation);
                if ui
                    .add(
                        egui::Button::new(label)
                            .selected(self.selected == row.node)
                            .truncate(),
                    )
                    .clicked()
                {
                    commands.push(EditorCommand::Select(row.node));
                }
            });
        }
    }

    fn show_analytics(&mut self, ui: &mut Ui) {
        ui.strong(tr("Move analytics"));
        if self.selected == self.tree.root() {
            ui.label(tr("Select a move to see its learning analytics"));
            return;
        }
        if self.learning.is_none() {
            ui.label(tr("Shared learning data is not connected"));
            return;
        }
        match self.selected_analytics() {
            Ok(Some(analytics)) => {
                show_move_analytics(ui, &analytics, self.tree.game(), self.selected)
            }
            Ok(None) => {
                ui.label(tr("Learning analytics is kept for the current main answer"));
            }
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
    }

    fn select(&mut self, node: NodeId) {
        if self.tree.game().is_attached(node) {
            self.selected = node;
            self.board.clear();
            self.analytics_cache = None;
        }
    }

    fn cancel(&mut self) {
        self.draft = self.original.clone();
        self.selected = self.tree.root();
        self.board.clear();
        self.analytics_cache = None;
        self.error = None;
    }

    fn save(&mut self, library_store: &LibraryStore) -> Result<Library, String> {
        let draft = self
            .draft
            .repertoire(&self.repertoire_id)
            .cloned()
            .ok_or_else(|| tr("Repertoire not found").to_owned())?;
        let original = self
            .original
            .repertoire(&self.repertoire_id)
            .cloned()
            .ok_or_else(|| tr("Repertoire not found").to_owned())?;
        let mut updated = original;
        updated.max_plies = draft.max_plies;
        updated.disabled_prefixes = draft.disabled_prefixes;
        updated.main_answer_overrides = draft.main_answer_overrides;
        let mut saved = self.original.clone();
        let slot = saved
            .repertoire_mut(&self.repertoire_id)
            .ok_or_else(|| tr("Repertoire not found").to_owned())?;
        *slot = updated;
        library_store
            .save(&mut saved)
            .map_err(|error| error.to_string())?;
        self.original = saved.clone();
        self.draft = saved.clone();
        self.analytics_cache = None;
        self.error = None;
        Ok(saved)
    }

    fn selected_move_parts(&self) -> Option<(String, String, LearningSide)> {
        let game = self.tree.game();
        let parent = game.parent(self.selected)?;
        let expected_uci = game.game_move(self.selected)?.uci()?;
        let position_fen4 = self.tree.position_key(parent)?.to_owned();
        let side = LearningSide::from(game.position(parent)?.turn());
        Some((position_fen4, expected_uci, side))
    }

    fn set_selected_main_answer(&mut self) -> Result<(), String> {
        let (position_fen4, expected_uci, side) = self
            .selected_move_parts()
            .ok_or_else(|| tr("Select a move in the PGN tree").to_owned())?;
        self.assign_main_answer_checked(&position_fen4, &expected_uci, side)
    }

    fn assign_main_answer_checked(
        &mut self,
        position_fen4: &str,
        expected_uci: &str,
        side: LearningSide,
    ) -> Result<(), String> {
        let repertoire = self
            .draft
            .repertoire_mut(&self.repertoire_id)
            .ok_or_else(|| tr("Repertoire not found").to_owned())?;
        repertoire
            .set_main_answer_override(position_fen4, expected_uci, side)
            .map_err(|_| {
                tr("The selected move cannot be the main answer for this position").to_owned()
            })?;
        self.analytics_cache = None;
        self.error = None;
        Ok(())
    }

    fn clear_selected_main_answer(&mut self) {
        let Some((position_fen4, _, _)) = self.selected_move_parts() else {
            return;
        };
        if let Some(repertoire) = self.draft.repertoire_mut(&self.repertoire_id) {
            repertoire.clear_main_answer_override(&position_fen4);
            self.analytics_cache = None;
            self.error = None;
        }
    }

    fn set_selected_disabled(&mut self, disabled: bool) -> Result<(), String> {
        let path = self
            .tree
            .path(self.selected)
            .filter(|path| !path.is_empty())
            .map(<[String]>::to_vec)
            .ok_or_else(|| tr("Select a move in the PGN tree").to_owned())?;
        let repertoire = self
            .draft
            .repertoire_mut(&self.repertoire_id)
            .ok_or_else(|| tr("Repertoire not found").to_owned())?;
        if disabled {
            repertoire
                .set_prefix_disabled(&path, true)
                .map_err(|_| tr("The selected PGN branch cannot be disabled").to_owned())?;
        } else {
            let blocking: Vec<Vec<String>> = repertoire
                .disabled_prefixes
                .iter()
                .filter(|prefix| path.starts_with(prefix))
                .cloned()
                .collect();
            for prefix in blocking {
                repertoire
                    .set_prefix_disabled(&prefix, false)
                    .map_err(|_| tr("The selected PGN branch cannot be enabled").to_owned())?;
            }
        }
        self.analytics_cache = None;
        self.error = None;
        Ok(())
    }

    fn effective_main_answer(&self, parent: NodeId) -> Option<String> {
        let repertoire = self.draft.repertoire(&self.repertoire_id)?;
        effective_main_answer(repertoire, &self.tree, parent)
    }

    fn selected_analytics(&mut self) -> Result<Option<MoveLearningAnalytics>, String> {
        if self.analytics_cache.is_none() {
            let result = self.compute_selected_analytics();
            self.analytics_cache = Some(result);
        }
        self.analytics_cache.clone().unwrap_or(Ok(None))
    }

    fn compute_selected_analytics(&self) -> Result<Option<MoveLearningAnalytics>, String> {
        let Some(learning) = &self.learning else {
            return Ok(None);
        };
        let Some((position_fen4, expected_uci, side)) = self.selected_move_parts() else {
            return Ok(None);
        };
        let key = LearningItemKey::local(position_fen4, expected_uci);
        let mut scope = StatsScope::for_repertoire(self.repertoire_id.clone(), side, now());
        scope.only_active = false;
        let analytics =
            scoped_move_learning_analytics(&self.draft, learning, &scope, &SrsSettings::default())
                .map_err(|error| error.to_string())?;
        Ok(analytics.into_iter().find(|item| item.key == key))
    }
}

#[derive(Clone, Copy)]
enum EditorCommand {
    Close,
    Cancel,
    Save,
    Select(NodeId),
    SetMainAnswer,
    ClearMainAnswer,
    SetDisabled(bool),
    SetDepth(u32),
}

#[derive(Clone, Copy)]
struct TreeRow {
    node: NodeId,
    indentation: u32,
    variation: bool,
}

fn tree_rows(tree: &RepertoireTree) -> Vec<TreeRow> {
    fn append(game: &Game, parent: NodeId, indentation: u32, rows: &mut Vec<TreeRow>) {
        for (index, child) in game.children(parent).iter().copied().enumerate() {
            let variation = index > 0;
            let child_indentation = indentation + u32::from(variation);
            rows.push(TreeRow {
                node: child,
                indentation: child_indentation,
                variation,
            });
            append(game, child, child_indentation, rows);
        }
    }

    let mut rows = Vec::new();
    append(tree.game(), tree.root(), 0, &mut rows);
    rows
}

fn effective_main_answer(
    repertoire: &Repertoire,
    tree: &RepertoireTree,
    parent: NodeId,
) -> Option<String> {
    let position_fen4 = tree.position_key(parent)?;
    let mut candidates = Vec::new();
    for transposition in tree.transpositions(parent) {
        if tree.path(*transposition).is_none() {
            continue;
        }
        for child in tree.game().children(*transposition) {
            if tree.path(*child).is_none() {
                continue;
            }
            let Some(uci) = tree.game().game_move(*child).and_then(|mv| mv.uci()) else {
                continue;
            };
            if !candidates.contains(&uci) {
                candidates.push(uci);
            }
        }
    }
    repertoire
        .main_answer_override(position_fen4)
        .filter(|override_uci| candidates.iter().any(|uci| uci == override_uci))
        .map(str::to_owned)
        .or_else(|| candidates.into_iter().next())
}

fn show_move_analytics(
    ui: &mut Ui,
    analytics: &MoveLearningAnalytics,
    game: &Game,
    selected: NodeId,
) {
    egui::Grid::new("repertoire_editor_move_analytics")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label(tr("Status"));
            ui.label(move_status_label(analytics.status));
            ui.end_row();
            ui.label(tr("Accuracy"));
            ui.label(
                analytics
                    .accuracy_percent
                    .map_or_else(|| "—".to_owned(), |value| format!("{value:.0} %")),
            );
            ui.end_row();
            ui.label(tr("Correct streak"));
            ui.label(analytics.correct_streak.to_string());
            ui.end_row();
            ui.label(tr("Wrong streak"));
            ui.label(analytics.wrong_streak.to_string());
            ui.end_row();
            ui.label(tr("Next due"));
            ui.label(format_time(analytics.next_review_at));
            ui.end_row();
            ui.label(tr("Response median"));
            ui.label(
                analytics
                    .response_time_median_ms
                    .map_or_else(|| "—".to_owned(), format_duration_ms),
            );
            ui.end_row();
        });
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{}:", tr("Wrong moves")));
        if analytics.wrong_move_histogram.is_empty() {
            ui.label("—");
        } else {
            for (uci, stats) in &analytics.wrong_move_histogram {
                let label = game
                    .position_before(selected)
                    .and_then(|position| san_for_uci(position, uci))
                    .unwrap_or_else(|| tr("Unknown move").to_owned());
                ui.label(format!("{label} × {}", stats.count));
            }
        }
    });
}

fn move_status_label(status: MoveLearningStatus) -> &'static str {
    tr(match status {
        MoveLearningStatus::Unseen => "Unseen",
        MoveLearningStatus::Learning => "Learning",
        MoveLearningStatus::LearntFresh => "Learnt",
        MoveLearningStatus::Uncertain => "Uncertain",
    })
}

fn san_for_uci(position: &Chess, uci: &str) -> Option<String> {
    let mv = uci.parse::<UciMove>().ok()?.to_move(position).ok()?;
    Some(crate::san::show(&SanPlus::from_move(position.clone(), mv).to_string()).into_owned())
}

fn format_duration_ms(milliseconds: f64) -> String {
    if milliseconds >= 1000.0 {
        format!("{:.2} s", milliseconds / 1000.0)
    } else {
        format!("{milliseconds:.0} ms")
    }
}

fn format_time(at: Option<i64>) -> String {
    at.and_then(|at| chrono::DateTime::from_timestamp(at, 0))
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_owned())
}

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use trainer::repertoire::{LearningStore, LearningStoreData, WrongMoveStats, fen4};

    use super::*;

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cp-repertoire-editor-{}-{}-{name}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn library() -> (Library, RepertoireId) {
        let mut library = Library::default();
        let id = library
            .create(
                "King and queen pawn",
                &chess_core::parse_str("1. e4 (1. d4) e5 *").game,
            )
            .unwrap();
        (library, id)
    }

    fn child(editor: &RepertoireEditorUi, index: usize) -> NodeId {
        editor.tree.game().children(editor.tree.root())[index]
    }

    #[test]
    fn main_answer_assignment_is_validated_and_clear_restores_pgn_default() {
        let (library, id) = library();
        let mut editor = RepertoireEditorUi::new(&library, id.clone()).unwrap();
        let root_fen = fen4(editor.tree.game().start().position());
        let pgn = library.repertoire(&id).unwrap().pgn.clone();

        let wrong_side = editor
            .assign_main_answer_checked(&root_fen, "d2d4", LearningSide::Black)
            .unwrap_err();
        assert!(!wrong_side.contains("d2d4"));
        let missing_move = editor
            .assign_main_answer_checked(&root_fen, "c2c4", LearningSide::White)
            .unwrap_err();
        assert!(!missing_move.contains("c2c4"));
        assert!(
            editor
                .draft
                .repertoire(&id)
                .unwrap()
                .main_answer_overrides
                .is_empty()
        );

        editor.select(child(&editor, 1));
        editor.set_selected_main_answer().unwrap();
        assert_eq!(
            editor
                .draft
                .repertoire(&id)
                .unwrap()
                .main_answer_override(&root_fen),
            Some("d2d4")
        );
        assert_eq!(
            editor.effective_main_answer(editor.tree.root()).as_deref(),
            Some("d2d4")
        );
        assert_eq!(editor.tree.game().children(editor.tree.root()).len(), 2);
        assert_eq!(editor.draft.repertoire(&id).unwrap().pgn, pgn);

        editor.clear_selected_main_answer();
        assert_eq!(
            editor
                .draft
                .repertoire(&id)
                .unwrap()
                .main_answer_override(&root_fen),
            None
        );
        assert_eq!(
            editor.effective_main_answer(editor.tree.root()).as_deref(),
            Some("e2e4")
        );
        assert_eq!(editor.tree.game().children(editor.tree.root()).len(), 2);
        assert_eq!(editor.draft.repertoire(&id).unwrap().pgn, pgn);
    }

    #[test]
    fn depth_disable_and_override_roundtrip_preserve_source_and_shared_learning() {
        let dir = TestDir::new("roundtrip");
        let source = dir.join("source.pgn");
        let source_bytes = b"[Event \"Original\"]\n\n1. e4 (1. d4) e5 *\n";
        std::fs::write(&source, source_bytes).unwrap();
        let (mut library, id) = library();
        let second = library
            .create(
                "Shared transposition",
                &chess_core::parse_str("1. e4 (1. d4) e5 *").game,
            )
            .unwrap();
        library.repertoire_mut(&id).unwrap().path = Some(source.clone());
        let original_pgn = library.repertoire(&id).unwrap().pgn.clone();
        let library_store = LibraryStore::new(dir.join("library.json"));
        library_store.save(&mut library).unwrap();

        let mut learning = LearningData::default();
        let root_fen = fen4(library.repertoire(&id).unwrap().game().start().position());
        let key = LearningItemKey::local(root_fen.clone(), "d2d4");
        let shared_item = {
            let item = learning.ensure_item(key.clone());
            item.introduced = true;
            item.correct_streak = 4;
            item.correct_count = 7;
            item.wrong_count = 2;
            item.next_review_at = Some(1_900_000_000);
            item.independent_response_times_ms = vec![500, 700, 900];
            item.wrong_move_histogram.insert(
                "c2c4".into(),
                WrongMoveStats {
                    count: 2,
                    last_at: 1_800_000_000,
                },
            );
            item.clone()
        };
        let learning_store = LearningStore::new(dir.join("learning.json"));
        let learning_data = LearningStoreData {
            learning: learning.clone(),
            ..LearningStoreData::default()
        };
        std::fs::write(
            learning_store.path(),
            serde_json::to_vec_pretty(&learning_data).unwrap(),
        )
        .unwrap();
        let learning_bytes = std::fs::read(learning_store.path()).unwrap();

        let mut editor = RepertoireEditorUi::new(&library, id.clone()).unwrap();
        editor.set_learning(&learning);
        editor.select(child(&editor, 1));
        editor.set_selected_main_answer().unwrap();
        editor.set_selected_disabled(true).unwrap();
        editor.draft.repertoire_mut(&id).unwrap().set_max_plies(7);
        assert_eq!(
            editor
                .draft
                .repertoire(&id)
                .unwrap()
                .main_answer_override(&root_fen),
            Some("d2d4")
        );
        assert_eq!(
            editor.effective_main_answer(editor.tree.root()).as_deref(),
            Some("d2d4")
        );

        let analytics = editor.selected_analytics().unwrap().unwrap();
        assert_eq!(analytics.key, key);
        assert_eq!(analytics.correct_streak, 4);
        assert_eq!(analytics.correct_count, 7);
        assert_eq!(analytics.response_time_median_ms, Some(700.0));

        let saved = editor.save(&library_store).unwrap();
        assert!(!editor.has_unsaved());
        assert_eq!(saved.repertoire(&id).unwrap().max_plies, 7);
        assert_eq!(saved.repertoire(&id).unwrap().disabled_prefixes.len(), 1);
        assert_eq!(saved.repertoire(&id).unwrap().pgn, original_pgn);
        assert!(saved.repertoire(&second).is_some());
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(
            std::fs::read(learning_store.path()).unwrap(),
            learning_bytes
        );

        editor.set_selected_disabled(false).unwrap();
        assert_eq!(
            editor
                .draft
                .repertoire(&id)
                .unwrap()
                .main_answer_override(&root_fen),
            Some("d2d4")
        );
        let saved = editor.save(&library_store).unwrap();
        assert!(saved.repertoire(&id).unwrap().disabled_prefixes.is_empty());
        assert_eq!(saved.repertoire(&id).unwrap().pgn, original_pgn);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(
            std::fs::read(learning_store.path()).unwrap(),
            learning_bytes
        );

        let loaded = library_store.load().unwrap();
        assert_eq!(loaded.repertoire(&id).unwrap().max_plies, 7);
        assert!(loaded.repertoire(&id).unwrap().disabled_prefixes.is_empty());
        assert_eq!(loaded.repertoire(&id).unwrap().pgn, original_pgn);
        let restored_learning = learning_store.load().unwrap();
        assert_eq!(restored_learning.learning.item(&key), Some(&shared_item));
    }

    #[test]
    fn cancel_restores_the_whole_draft_without_saving() {
        let dir = TestDir::new("cancel");
        let (mut library, id) = library();
        let library_store = LibraryStore::new(dir.join("library.json"));
        library_store.save(&mut library).unwrap();
        let before = std::fs::read(library_store.path()).unwrap();
        let original = library.repertoire(&id).unwrap().clone();
        let mut editor = RepertoireEditorUi::new(&library, id.clone()).unwrap();
        editor.select(child(&editor, 1));
        editor.set_selected_main_answer().unwrap();
        editor.set_selected_disabled(true).unwrap();
        editor.draft.repertoire_mut(&id).unwrap().set_max_plies(3);
        assert!(editor.has_unsaved());

        editor.cancel();

        assert!(!editor.has_unsaved());
        assert_eq!(editor.draft.repertoire(&id), Some(&original));
        assert_eq!(editor.selected, editor.tree.root());
        assert_eq!(std::fs::read(library_store.path()).unwrap(), before);
    }

    #[test]
    fn rav_rows_are_indented_without_flattening_the_mainline() {
        let (library, id) = library();
        let editor = RepertoireEditorUi::new(&library, id).unwrap();
        let rows = tree_rows(&editor.tree);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].indentation, 0);
        assert!(!rows[0].variation);
        assert_eq!(rows[1].indentation, 0);
        assert!(!rows[1].variation);
        assert_eq!(rows[2].indentation, 1);
        assert!(rows[2].variation);
    }
}
