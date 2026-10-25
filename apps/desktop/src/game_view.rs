//! Game screen: board with eval bar, engine lines with PV preview, notation, toolbar.

use crate::i18n::{tr, trf};
use crate::reference::RefAction;
use std::path::PathBuf;

use chess_core::shakmaty::{Chess, Color, Move, Position, Square};
use chess_core::{
    Annotation, CommentPart, Diagnostic, Game, GameResult, MoveKind, Nag, NodeId, Severity,
    ShapeColor, parse_comment,
};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText, Sense, Ui, vec2};

use crate::analysis::{self, Analysis, PvLine, Status};
use crate::assets::Sound;
use crate::board::{self, Arrow, BoardState, BoardView};
use crate::notation::{
    CommentEditor, EditorOutcome, MOVE_NAGS, Notation, NotationCommand, Slot, toggle_nag,
};
use crate::review::{DialogAction, ReportAction, Review};
use crate::settings::Settings;

/// Where an opened game came from (to return to the same library row, to save into).
#[derive(Clone, Debug)]
pub struct GameSource {
    pub path: PathBuf,
    pub index: usize,
    /// Why the game cannot be saved back there (`None`: a PGN that takes games).
    pub block: Option<crate::own_db::Block>,
}

/// A frozen engine line being stepped through on the board. Never changes the game.
pub struct Preview {
    base: NodeId,
    root: Chess,
    moves: Vec<Move>,
    san: Vec<String>,
    shown: usize,
}

impl Preview {
    fn position(&self) -> Chess {
        let mut pos = self.root.clone();
        for m in &self.moves[..self.shown] {
            pos.play_unchecked(*m);
        }
        pos
    }
}

pub struct GameView {
    pub game: Game,
    pub source: Option<GameSource>,
    /// A new game meant for this database (the table's «+ Game»): Ctrl+S adds it there.
    /// Cleared once the game is in a database.
    pub target: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    cursor: NodeId,
    board: BoardState,
    flipped: bool,
    preview: Option<Preview>,
    scroll_to_cursor: bool,
    /// The comment being written (user-comments.md).
    editor: Option<CommentEditor>,
    /// A game against Maia in progress (play-mode.md).
    pub play: Option<crate::play::Session>,
    /// The game just played: the result strip.
    pub play_end: Option<crate::play::End>,
    /// What the play started with (for the end, a rematch).
    play_start: Option<crate::play::Start>,
    /// «The game is on» asked (leaving during a game); the clocks wait meanwhile.
    /// Where leaving goes, and since when the question is open.
    play_leave: Option<(crate::app::CloseIntent, std::time::Instant)>,
    show_diagnostics: bool,
    engine_settings_open: bool,
    review: Review,
    side_tab: SideTab,
    /// The game this one was opened from (reference tab): the title for "← back to".
    pub previous: Option<String>,
    /// The game's data were asked once (on its first add); not again.
    pub data_asked: bool,
    /// A newer version to offer (its number), next to Settings.
    pub update: Option<String>,
    /// Comments in another language, translated for reading (never into the PGN).
    translation: crate::translate_ui::GameTranslation,
    /// "Translate comments" was chosen (menu, Ctrl+Shift+T): done once the settings are at hand.
    translate_now: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SideTab {
    Notation,
    /// What was played here in a database (`reference.rs`).
    Reference,
    Report,
}

pub enum GameAction {
    Back,
    Sound(Sound),
    CopyText(String, &'static str),
    /// Ctrl+S: into the game's own database, else "Add to database…".
    Save,
    /// After a game against Maia: the same again with colours swapped.
    Rematch,
    /// Leaving after «The game is on» was answered.
    Leave(crate::app::CloseIntent),
    /// Saving into a database, then leaving.
    SaveAndLeave(crate::app::CloseIntent),
    /// After a game against Maia: the play dialog.
    PlayAgain,
    /// Ctrl+Shift+S: choose the database to add the game to.
    AddToDatabase,
    /// F2: players, event, date, result…
    EditGameData,
    ToggleEngine,
    /// Threads/Hash/MultiPV/search time of the active engine changed (settings already updated).
    EngineReconfigure,
    /// Another engine was made active.
    EngineSwitch,
    OpenEngineSettings,
    OpenSettings,
    Toast(String),
    /// A whole-game analysis started: pause the live engine (they would share the CPU).
    PauseLive,
    /// The whole-game analysis ended: resume the live engine if it was paused.
    ResumeLive,
    /// Find the current position in a database (`None`: the last chosen one).
    FindPosition(Option<std::path::PathBuf>),
    /// Game analysis needs an engine that is not there yet (offer to download one).
    NeedEngine,
    /// The reference tab: use / load a database, open one of its games.
    ReferenceChoose(std::path::PathBuf),
    ReferenceLoad(std::path::PathBuf),
    OpenGame(std::path::PathBuf, usize, Chess),
    /// Back to the game this one was opened from.
    ReturnToPrevious,
    /// The window offering a newer version.
    OpenUpdate,
    /// Settings → Translation.
    OpenTranslationSettings,
}

impl GameView {
    pub fn new(game: Game, source: Option<GameSource>, diagnostics: Vec<Diagnostic>) -> Self {
        let flipped = false;
        let review = Review::new(&game);
        Self {
            game,
            source,
            diagnostics,
            cursor: NodeId::ROOT,
            board: BoardState::default(),
            flipped,
            preview: None,
            scroll_to_cursor: true,
            editor: None,
            play: None,
            play_end: None,
            play_start: None,
            play_leave: None,
            show_diagnostics: false,
            engine_settings_open: false,
            review,
            // Dev hook (screenshots): `CP_TAB=reference` opens that tab.
            side_tab: if std::env::var("CP_TAB").as_deref() == Ok("reference") {
                SideTab::Reference
            } else {
                SideTab::Notation
            },
            previous: None,
            data_asked: false,
            target: None,
            update: None,
            translation: Default::default(),
            translate_now: false,
        }
    }

    /// The game is put aside (another one opened): its translation stops.
    pub fn suspend(&mut self) {
        self.translation.suspend();
    }

    /// Copying the PGN always copies the original comments; say so when the translation is shown.
    fn copy_pgn(&self) -> GameAction {
        let note = if self.translation.shown().is_some() {
            tr("PGN copied (comments in the original)")
        } else {
            tr("PGN copied")
        };
        GameAction::CopyText(self.game.to_pgn(), note)
    }

    pub fn title(&self) -> String {
        let g = &self.game;
        // In a game against Maia an unnamed user is «You».
        let user = self.play.as_ref().map(|s| s.user);
        let unnamed = |c: Color| {
            if user == Some(c) {
                tr("You")
            } else {
                c.fold_wb(tr("White"), tr("Black"))
            }
        };
        let white = g
            .tag("White")
            .filter(|s| *s != "?")
            .unwrap_or(unnamed(Color::White));
        // A book chapter imported from ChessBase: its title is in White, Black is empty.
        if g.tag("Black") == Some("") {
            return white.to_string();
        }
        let black = g
            .tag("Black")
            .filter(|s| *s != "?")
            .unwrap_or(unnamed(Color::Black));
        format!("{white} — {black}")
    }

    /// Where the game lives: «🗄 Petrov · No. 13» (🌐 an online database, 🔒 a read-only
    /// one); nothing for a game that is in no database yet.
    pub fn source_line(&self) -> Option<String> {
        if self.source.is_none()
            && let Some(t) = &self.target
        {
            return Some(format!(
                "🗄 {}",
                trf(
                    "{name} · new game",
                    &[("name", &crate::own_db::base_name(t))]
                )
            ));
        }
        let s = self.source.as_ref()?;
        let icon = match &s.block {
            None => "🗄",
            Some(crate::own_db::Block::Online) => "🌐",
            Some(_) => "🔒",
        };
        Some(format!(
            "{icon} {} · № {}",
            crate::own_db::base_name(&s.path),
            s.index + 1
        ))
    }

    /// What Ctrl+S does to this game, for the button's tooltip.
    pub fn save_hint(&self) -> String {
        match (&self.source, &self.target) {
            (Some(s), _) if s.block.is_none() => trf(
                "Save to «{name}» (Ctrl+S)",
                &[("name", &crate::own_db::base_name(&s.path))],
            ),
            (None, Some(t)) => trf(
                "Add to «{name}» (Ctrl+S)",
                &[("name", &crate::own_db::base_name(t))],
            ),
            _ => tr("Add to database… (Ctrl+S)").to_owned(),
        }
    }

    fn subtitle(&self) -> String {
        let g = &self.game;
        let mut parts = Vec::new();
        for t in ["Event", "Site", "Date", "ECO", "Annotator"] {
            if let Some(v) = g
                .tag(t)
                .filter(|v| !v.is_empty() && !v.chars().all(|c| c == '?' || c == '.'))
            {
                parts.push(v.to_string());
            }
        }
        parts.join(" · ")
    }

    /// A move made on the board or picked in the reference tab: goes to it if the
    /// game has it, else adds it (undoable).
    fn play_move(&mut self, m: Move, actions: &mut Vec<GameAction>) {
        if self.play.is_some() {
            self.user_move(m, actions);
            return;
        }
        match self.game.add_move(self.cursor, m) {
            Ok(id) => {
                self.goto(id);
                if let Some(s) = self.sound_for(id) {
                    actions.push(GameAction::Sound(s));
                }
            }
            Err(e) => actions.push(GameAction::Toast(trf("Move not added: {e}", &[("e", &e)]))),
        }
    }

    fn goto(&mut self, id: NodeId) {
        if id != self.cursor {
            // Leaving the move writes the comment being written (user-comments.md §4).
            self.commit_editor();
            self.cursor = id;
            self.board.clear();
            self.scroll_to_cursor = true;
        }
        self.preview = None;
    }

    // ----- comments (docs/design/user-comments.md) ------------------------------------

    /// «4.Ba4» / «4…Nf6» as the editor's title names the move.
    fn move_name(&self, id: NodeId) -> String {
        let Some((num, color)) = self.game.move_number(id) else {
            return String::new();
        };
        let san = self
            .game
            .game_move(id)
            .map(|m| m.san.as_str())
            .unwrap_or("?");
        let san = crate::san::show(san);
        match color {
            Color::White => format!("{num}.{san}"),
            Color::Black => format!("{num}…{san}"),
        }
    }

    /// Opens the editor on this comment (the one open before is written first). The
    /// move becomes the current one.
    fn open_editor(&mut self, slot: Slot) {
        if self.preview.is_some() {
            return;
        }
        if self.editor.as_ref().is_some_and(|e| e.slot == slot) {
            if let Some(e) = self.editor.as_mut() {
                e.focus = true;
            }
            return;
        }
        self.commit_editor();
        let id = slot.node();
        let Some(node) = self.game.node(id) else {
            return;
        };
        let (original, nags, title, short) = match slot {
            Slot::After(NodeId::ROOT) => (
                node.comments_after().to_vec(),
                Vec::new(),
                tr("Comment at the start of the game").to_owned(),
                tr("At the start").to_owned(),
            ),
            Slot::After(_) => (
                node.comments_after().to_vec(),
                node.nags().to_vec(),
                trf("Comment after {move}", &[("move", &self.move_name(id))]),
                trf("After {move}", &[("move", &self.move_name(id))]),
            ),
            Slot::Before(_) => (
                node.comments_before().to_vec(),
                Vec::new(),
                trf("Comment before {move}", &[("move", &self.move_name(id))]),
                trf("Before {move}", &[("move", &self.move_name(id))]),
            ),
        };
        let text = chess_core::comment_display_text(&original);
        let translated = self
            .translation
            .shown()
            .and_then(|m| m.get(&text))
            .is_some_and(|m| matches!(m, crate::translate_ui::Mark::Translated(_)));
        if matches!(slot, Slot::After(_)) && id != NodeId::ROOT {
            self.goto(id);
        }
        self.editor = Some(CommentEditor {
            slot,
            initial: text.clone(),
            text,
            initial_nags: nags.clone(),
            nags,
            title,
            short_title: short,
            translated,
            focus: true,
            scroll: true,
            field_id: None,
        });
    }

    /// Writes the comment being written into the game (one undo step) and closes the
    /// editor; nothing written when nothing changed. Also for the program: before the
    /// game is left or saved.
    pub fn commit_editor(&mut self) {
        let Some(ed) = self.editor.take() else { return };
        if !ed.changed() {
            return;
        }
        let id = ed.slot.node();
        let Some(node) = self.game.node(id) else {
            return;
        };
        match ed.slot {
            Slot::After(_) => {
                let original = node.comments_after().to_vec();
                let comments = if ed.text_changed() {
                    chess_core::with_text(&original, &ed.text)
                } else {
                    original
                };
                let _ = self.game.annotate(id, ed.nags, comments);
            }
            Slot::Before(_) => {
                let original = node.comments_before().to_vec();
                let _ = self
                    .game
                    .set_comments_before(id, chess_core::with_text(&original, &ed.text));
            }
        }
        self.scroll_to_cursor = true;
    }

    fn cancel_editor(&mut self) {
        self.editor = None;
        self.scroll_to_cursor = true;
    }

    /// The editor's text field has the keyboard.
    fn editor_focused(&self, ui: &Ui) -> bool {
        self.editor
            .as_ref()
            .and_then(|e| e.field_id)
            .is_some_and(|id| ui.ctx().memory(|m| m.focused() == Some(id)))
    }

    /// The keys of the editor, taken before the text field sees them: Enter — done,
    /// Shift+Enter — a paragraph, Esc — cancel, Ctrl+S / Ctrl+W — done and then the
    /// action, Ctrl+1…6 — the move's symbol in the draft.
    fn editor_keys(&mut self, ui: &Ui, actions: &mut Vec<GameAction>) {
        if !self.editor_focused(ui) {
            return;
        }
        let ctrl = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let (done, paragraph, cancel, save, back, nag) = ui.input_mut(|i| {
            // Shift+Enter first: a plain Enter would match it too (an extra Shift is
            // ignored by egui's key match).
            let paragraph = i.consume_key(Modifiers::SHIFT, Key::Enter);
            let done = !paragraph
                && (i.consume_key(Modifiers::NONE, Key::Enter)
                    || i.consume_shortcut(&ctrl(Key::Enter)));
            let cancel = i.consume_key(Modifiers::NONE, Key::Escape);
            let save = i.consume_shortcut(&ctrl(Key::S));
            let back = i.consume_shortcut(&ctrl(Key::W));
            let nag = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
            ]
            .into_iter()
            .zip(MOVE_NAGS)
            .find(|(k, _)| i.consume_shortcut(&ctrl(*k)))
            .map(|(_, (nag, _, _))| nag);
            (done, paragraph, cancel, save, back, nag)
        });
        if let Some(nag) = nag
            && let Some(ed) = self.editor.as_mut()
        {
            ed.nags = toggle_nag(&ed.nags, nag);
        }
        if paragraph && let Some(ed) = self.editor.as_mut() {
            // A blank line at the cursor.
            let Some(field) = ed.field_id else { return };
            let mut state = egui::TextEdit::load_state(ui.ctx(), field).unwrap_or_default();
            let at = state
                .cursor
                .char_range()
                .map_or(ed.text.chars().count(), |r| r.primary.index.into());
            let byte = ed
                .text
                .char_indices()
                .nth(at)
                .map_or(ed.text.len(), |(b, _)| b);
            ed.text.insert_str(byte, "\n\n");
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(at + 2),
                )));
            state.store(ui.ctx(), field);
        }
        if cancel {
            self.cancel_editor();
        } else if done || save || back {
            self.commit_editor();
            if save {
                actions.push(GameAction::Save);
            }
            if back {
                actions.push(GameAction::Back);
            }
        }
    }

    /// A symbol put on or taken off a move at once (the menu, Ctrl+1…6): one undo step.
    fn toggle_nag_on(&mut self, id: NodeId, nag: Nag) {
        if id == NodeId::ROOT {
            return;
        }
        let Some(node) = self.game.node(id) else {
            return;
        };
        let nags = toggle_nag(node.nags(), nag);
        let comments = node.comments_after().to_vec();
        let _ = self.game.annotate(id, nags, comments);
    }

    /// Deletes a comment (its board commands and clocks stay); undoable.
    fn delete_comment(&mut self, slot: Slot, actions: &mut Vec<GameAction>) {
        if self.editor.as_ref().is_some_and(|e| e.slot == slot) {
            self.editor = None;
        }
        let id = slot.node();
        let Some(node) = self.game.node(id) else {
            return;
        };
        let done = match slot {
            Slot::After(_) => {
                let comments = chess_core::with_text(node.comments_after(), "");
                self.game.annotate(id, node.nags().to_vec(), comments)
            }
            Slot::Before(_) => {
                let comments = chess_core::with_text(node.comments_before(), "");
                self.game.set_comments_before(id, comments)
            }
        };
        if done.is_ok() {
            actions.push(GameAction::Toast(
                tr("Comment deleted (Ctrl+Z to undo)").to_owned(),
            ));
        }
    }

    /// An arrow or a circle drawn on the board goes into the comment of the current move
    /// (`[%cal]`, `[%csl]`); the same shape again takes it off, another colour replaces it.
    fn apply_shape(&mut self, drawn: board::Drawn) {
        let id = self.cursor;
        let Some(node) = self.game.node(id) else {
            return;
        };
        let (mut arrows, mut squares) = chess_core::shapes(node.comments_after());
        match drawn {
            board::Drawn::Arrow(color, from, to) => {
                let same = arrows.iter().position(|(_, a, b)| *a == from && *b == to);
                match same {
                    Some(i) if arrows[i].0 == color => {
                        arrows.remove(i);
                    }
                    Some(i) => arrows[i].0 = color,
                    None => arrows.push((color, from, to)),
                }
            }
            board::Drawn::Circle(color, sq) => {
                let same = squares.iter().position(|(_, s)| *s == sq);
                match same {
                    Some(i) if squares[i].0 == color => {
                        squares.remove(i);
                    }
                    Some(i) => squares[i].0 = color,
                    None => squares.push((color, sq)),
                }
            }
        }
        let comments = chess_core::with_shapes(node.comments_after(), &arrows, &squares);
        let _ = self.game.annotate(id, node.nags().to_vec(), comments);
    }

    /// Assessment of the move that led to the current position, for the board: the
    /// engine's judgement when the game analysis is shown, otherwise the author's
    /// `!`/`?` symbol.
    fn badge(&self) -> Option<board::Badge> {
        let square = match self.game.game_move(self.cursor)?.kind {
            MoveKind::Normal(m) => move_squares(&m)?.1,
            MoveKind::Null => return None,
        };
        if self.review.visible
            && let Some(mark) = self.review.analysis.marks.get(&self.cursor)
        {
            return Some(board::Badge {
                square,
                text: crate::review::judgement_symbol(mark.judgement),
                color: crate::review::judgement_color(mark.judgement),
            });
        }
        let nag = self
            .game
            .node(self.cursor)?
            .nags()
            .iter()
            .copied()
            .find(|n| n.is_move_assessment())?;
        Some(board::Badge {
            square,
            text: nag.symbol()?,
            color: crate::notation::assessment_color(nag)?,
        })
    }

    fn sound_for(&self, id: NodeId) -> Option<Sound> {
        let mv = self.game.game_move(id)?;
        let pos = self.game.position(id)?;
        Some(if pos.is_checkmate() {
            Sound::Checkmate
        } else if pos.is_check() {
            Sound::Check
        } else if matches!(mv.kind, MoveKind::Normal(m) if m.is_capture()) {
            Sound::Capture
        } else {
            Sound::Move
        })
    }

    /// `input_enabled` is false while a modal dialog is open over the game.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        analysis: &mut Analysis,
        reference: &mut crate::reference::Reference,
        hub: &mut crate::translate_ui::Hub,
        input_enabled: bool,
    ) -> Vec<GameAction> {
        let mut actions = Vec::new();
        self.translation.update(ui.ctx(), &self.game, settings, hub);
        if self.review.poll() {
            actions.push(GameAction::ResumeLive);
            if let Some(e) = &self.review.error {
                actions.push(GameAction::Toast(e.clone()));
            }
        }
        self.review.refresh(&self.game);
        if analysis.status == Status::Running
            && let Some(e) = settings.active_engine()
        {
            self.review.note_live_speed(analysis.nps, e.threads);
        }
        self.play_tick(ui.ctx(), &mut actions);
        if input_enabled && !self.review.dialog_open && self.play_leave.is_none() {
            self.editor_keys(ui, &mut actions);
            self.keyboard(ui, &mut actions);
        }

        // The cursor can change inside any panel; re-bind the engine before each use of its lines.
        self.sync_analysis(analysis);
        let databases = settings.databases.clone();
        egui::Panel::top("game_header").show(ui, |ui| self.header(ui, &databases, &mut actions));
        // Side by side or stacked (docs/design/narrow-windows.md §3): decided by the size
        // of the window alone, for the notation tab with the evaluation bar and no graph,
        // so that switching a tab, the engine or the report never rearranges the screen.
        let width = ui.available_width();
        let height = ui.available_height();
        let graph = self.review.visible && (self.review.has_data() || self.review.running());
        let graph_h = if graph {
            (height * 0.14).clamp(80.0, 130.0) + 8.0
        } else {
            0.0
        };
        let board_side = (height - 16.0).min(width - SIDE_MIN - EVAL_BAR - 24.0);
        let board_stack = (width - 16.0 - EVAL_BAR).min(height - STACK_BOTTOM_MIN - 16.0);
        let stacked = board_stack >= 1.25 * board_side;
        let least = if self.side_tab == SideTab::Reference {
            REF_MIN
        } else {
            SIDE_MIN
        };
        let side = if stacked {
            // The notation under the board, as high as the board leaves; dragging its
            // edge up gives the notation more and the board less.
            // The engine's lines and settings come on top of the notation's least
            // part (their height as last drawn); the board gives way for them.
            let engine =
                egui::containers::panel::PanelState::load(ui.ctx(), egui::Id::new("engine_panel"))
                    .map_or(0.0, |p| (p.size().y - ENGINE_ROW).max(0.0));
            let most = (height - BOARD_MIN - graph_h - 16.0).max(STACK_BOTTOM_MIN);
            let bottom_least = (STACK_BOTTOM_MIN + engine).min(most);
            let rest = (height - board_stack - graph_h - 16.0).clamp(bottom_least, most);
            egui::Panel::bottom("game_side_stacked")
                .resizable(true)
                .default_size(rest)
                .size_range(bottom_least..=most)
                .show(ui, |ui| {
                    self.side_panel(ui, settings, analysis, reference, hub, &mut actions)
                })
        } else {
            // The board takes the full height of the window: the side panel is never
            // wider than what is left beside it (the user can make it narrower, not
            // wider), nor than leaves the board its least size. The reference tab has
            // two wide tables and its own width (egui remembers each).
            let board = height - graph_h - 16.0;
            let most = (width - BOARD_MIN - EVAL_BAR - 24.0).max(least);
            let room = (width - board - 24.0).clamp(least, most);
            let (side_id, side_width) = if self.side_tab == SideTab::Reference {
                ("game_side_ref", room.min(1100.0))
            } else {
                ("game_side", (width * 0.4).clamp(260.0, 460.0).min(room))
            };
            egui::Panel::right(side_id)
                .resizable(true)
                .default_size(side_width)
                .size_range(least..=most)
                .show(ui, |ui| {
                    self.side_panel(ui, settings, analysis, reference, hub, &mut actions)
                })
        };
        crate::widgets::check_panel("game_side", &side.response);
        self.sync_analysis(analysis);
        let hover = (self.side_tab == SideTab::Reference)
            .then(|| reference.hover())
            .flatten();
        egui::CentralPanel::default().show(ui, |ui| {
            self.board_area(ui, &*settings, analysis, hover, &mut actions)
        });
        self.sync_analysis(analysis);

        match self.review.dialog(ui.ctx(), settings) {
            Some(DialogAction::Start) => self.start_review(settings, true, ui.ctx(), &mut actions),
            Some(DialogAction::EngineSwitch) => actions.push(GameAction::EngineSwitch),
            Some(DialogAction::OpenEngines) => actions.push(GameAction::OpenEngineSettings),
            None => {}
        }

        // Leaving during a game asks first; the question itself.
        self.guard_leaving(&mut actions);
        self.leave_question(ui.ctx(), &mut actions);

        if self.show_diagnostics {
            let mut open = true;
            egui::Window::new(tr("PGN compatibility"))
                .open(&mut open)
                .default_width(520.0)
                .show(ui.ctx(), |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .show(ui, |ui| {
                            for d in &self.diagnostics {
                                let (icon, color) = match d.severity {
                                    Severity::Info => ("ℹ", ui.visuals().weak_text_color()),
                                    Severity::Warning => ("⚠", ui.visuals().warn_fg_color),
                                    Severity::Error => ("✖", ui.visuals().error_fg_color),
                                };
                                let line = d
                                    .location
                                    .as_ref()
                                    .map(|l| trf("line {n}: ", &[("n", &l.line)]))
                                    .unwrap_or_default();
                                ui.label(
                                    RichText::new(format!("{icon} {line}{}", d.message))
                                        .color(color),
                                );
                            }
                        });
                });
            self.show_diagnostics = open;
        }
        self.scroll_to_cursor = false;
        actions
    }

    fn start_review(
        &mut self,
        settings: &Settings,
        full: bool,
        ctx: &egui::Context,
        actions: &mut Vec<GameAction>,
    ) {
        if !crate::engine_offer::EngineOffer::engine_ready(settings) {
            self.review.dialog_open = false;
            actions.push(GameAction::NeedEngine);
            return;
        }
        let Some(engine) = settings.active_engine().cloned() else {
            self.review.dialog_open = true;
            return;
        };
        if self
            .review
            .start(&self.game, &engine, &settings.review, full, ctx)
        {
            actions.push(GameAction::PauseLive);
            self.side_tab = SideTab::Report;
        } else if let Some(e) = &self.review.error {
            actions.push(GameAction::Toast(e.clone()));
        } else {
            actions.push(GameAction::Toast(
                tr("Every position is already evaluated").into(),
            ));
        }
    }

    /// Toolbar button: start, progress with cancel, or show/hide the report.
    fn review_button(&mut self, ui: &mut Ui, actions: &mut Vec<GameAction>) {
        let big = |s: &str| RichText::new(s).size(18.0);
        if let Some((done, total)) = self.review.progress() {
            ui.add(
                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                    .desired_width(70.0)
                    .text(format!("{done}/{total}")),
            )
            .on_hover_text(tr("Game analysis"));
            if crate::widgets::small_close_button(ui)
                .on_hover_text(tr("Stop the game analysis"))
                .clicked()
            {
                self.review.cancel();
                actions.push(GameAction::ResumeLive);
            }
        } else if self.review.has_data() {
            ui.toggle_value(&mut self.review.visible, big("📈"))
                .on_hover_text(tr(
                    "Report: show or hide the evaluation graph and the report (Shift+A)",
                ));
        } else if ui
            .button(big("📈"))
            .on_hover_text(tr(
                "Analyse game: accuracy, inaccuracies, mistakes, blunders (Shift+A)",
            ))
            .clicked()
        {
            self.review.dialog_open = true;
        }
    }

    /// Keeps the engine bound to the game cursor (never to the preview). Cheap when unchanged.
    fn sync_analysis(&self, analysis: &mut Analysis) {
        if !analysis.is_on() {
            return;
        }
        if let (Some(h), Some(pos)) = (
            self.game.engine_history(self.cursor),
            self.game.position(self.cursor),
        ) {
            let ep = match h.start_fen {
                None => uci::EnginePosition::startpos(h.moves),
                Some(fen) => uci::EnginePosition::fen(fen, h.moves),
            };
            analysis.set_position(pos, ep);
        }
    }

    fn header(
        &mut self,
        ui: &mut Ui,
        databases: &[std::path::PathBuf],
        actions: &mut Vec<GameAction>,
    ) {
        ui.add_space(4.0);
        let warnings = self
            .diagnostics
            .iter()
            .filter(|d| d.severity >= Severity::Warning)
            .count();
        let notes = (!self.diagnostics.is_empty()).then(|| {
            if warnings > 0 {
                RichText::new(format!("⚠ {warnings}")).color(ui.visuals().warn_fg_color)
            } else {
                RichText::new(format!("ℹ {}", self.diagnostics.len())).weak()
            }
        });
        let back_to = self
            .previous
            .as_ref()
            .map(|t| trf("⬅ Back to {game}", &[("game", t)]));
        // The buttons are measured first: the title gets what they leave (shortened with
        // "…"; the result always shows). When it would get under 160 pt, the buttons
        // around it give up their words for their icons (the words go to the tooltips).
        let gap = ui.spacing().item_spacing.x;
        let button = |ui: &Ui, text: &str| {
            text_width(ui, text, egui::TextStyle::Button)
                + 2.0 * ui.spacing().button_padding.x
                + gap
        };
        let left = |compact: bool| {
            button(ui, tr("⬅ Library"))
                + back_to
                    .as_deref()
                    .map_or(0.0, |b| button(ui, if compact { "⬅" } else { b }))
                + gap
                + 2.0 * gap
        };
        let right = |compact: bool| {
            button(ui, if compact { "⚙" } else { tr("⚙ Settings") })
                + self.update.as_deref().map_or(0.0, |v| button(ui, v))
                + button(ui, "⏷")
                + button(
                    ui,
                    if compact {
                        "🔍"
                    } else {
                        tr("🔍 Find in database")
                    },
                )
                + notes.as_ref().map_or(0.0, |n| button(ui, n.text()))
        };
        let result = result_label(self.game.result());
        let result_w = result.map_or(0.0, |r| {
            ui.painter()
                .layout_no_wrap(
                    format!(" {r} "),
                    egui::FontId::proportional(15.0),
                    Color32::WHITE,
                )
                .size()
                .x
                + gap
        });
        let row = ui.available_width();
        let compact = row - left(false) - right(false) - result_w < 160.0;
        let title_room = (row - left(compact) - right(compact)).max(0.0);
        ui.horizontal(|ui| {
            if ui
                .button(if compact { "⬅" } else { tr("⬅ Library") })
                .on_hover_text(if self.previous.is_some() {
                    tr("Back to the game list (Ctrl+W)")
                } else {
                    tr("Back to the game list (Esc, Ctrl+W)")
                })
                .clicked()
            {
                actions.push(GameAction::Back);
            }
            if let Some(b) = &back_to
                && ui
                    .button(if compact { "⬅" } else { b.as_str() })
                    .on_hover_text(if compact {
                        format!(
                            "{b}\n{}",
                            tr("The game this one was opened from, as you left it (Esc)")
                        )
                    } else {
                        tr("The game this one was opened from, as you left it (Esc)").to_owned()
                    })
                    .clicked()
            {
                actions.push(GameAction::ReturnToPrevious);
            }
            ui.separator();
            let height = ui.available_height();
            ui.allocate_ui_with_layout(
                vec2(title_room, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(title_room);
                    ui.horizontal(|ui| {
                        ui.set_max_width(title_room);
                        // The unsaved mark first: it must not be truncated away.
                        let mut mark_w = 0.0;
                        if self.game.is_modified() && self.play.is_none() {
                            let mark = ui
                                .label(RichText::new("•").strong().size(16.0))
                                .on_hover_text(tr("Unsaved changes"));
                            mark_w = mark.rect.width() + ui.spacing().item_spacing.x;
                        }
                        let title = self.title();
                        ui.allocate_ui(vec2((title_room - result_w - mark_w).max(0.0), height), |ui| {
                            // The title opens the game's data (players, event, date…).
                            let label = egui::Label::new(RichText::new(&title).strong().size(16.0))
                                .truncate();
                            if self.play.is_some() {
                                ui.add(label);
                            } else if ui
                                .add(label.sense(Sense::click()))
                                .on_hover_text(tr("Game data (F2)"))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                actions.push(GameAction::EditGameData);
                            }
                        });
                        if let Some(result) = result {
                            ui.label(
                                RichText::new(format!(" {result} "))
                                    .strong()
                                    .size(15.0)
                                    .background_color(ui.visuals().widgets.inactive.weak_bg_fill),
                            )
                            .on_hover_text(result_hint(self.game.result()));
                        }
                    });
                    let sub: Vec<String> = self
                        .source_line()
                        .into_iter()
                        .chain(Some(self.subtitle()).filter(|s| !s.is_empty()))
                        .collect();
                    if !sub.is_empty() {
                        ui.add(egui::Label::new(RichText::new(sub.join(" · ")).weak()).truncate());
                    }
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if compact { "⚙" } else { tr("⚙ Settings") })
                    .on_hover_text(if compact { format!("{} (Ctrl+,)", tr("Settings")) } else { "Ctrl+,".to_owned() })
                    .clicked()
                {
                    actions.push(GameAction::OpenSettings);
                }
                if let Some(v) = &self.update
                    && crate::widgets::update_button(ui, v).clicked()
                {
                    actions.push(GameAction::OpenUpdate);
                }
                // Find the position in a database: the last chosen one, or another.
                let searchable = self.search_line().is_some();
                let why = tr("After a null move (--) this is a teaching scheme, not a position of a game: it is not searched");
                // Not during a game against Maia: it would leave the game.
                let playing = self.play.is_some();
                if !playing { ui.add_enabled_ui(searchable && !databases.is_empty(), |ui| {
                    ui.menu_button("⏷", |ui| {
                        for db in databases {
                            let name = db
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            if ui.button(name).clicked() {
                                actions.push(GameAction::FindPosition(Some(db.clone())));
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("Find in another database"));
                    if ui
                        .button(if compact { "🔍" } else { tr("🔍 Find in database") })
                        .on_hover_text(tr("Find this position in a database (Ctrl+Shift+F)"))
                        .on_disabled_hover_text(if searchable { tr("Add a database first") } else { why })
                        .clicked()
                    {
                        actions.push(GameAction::FindPosition(None));
                    }
                }); }
                if let Some(text) = notes
                    && ui
                        .button(text)
                        .on_hover_text(tr("PGN compatibility notes"))
                        .clicked()
                {
                    self.show_diagnostics = true;
                }
            });
        });
        ui.add_space(4.0);
    }

    fn board_area(
        &mut self,
        ui: &mut Ui,
        settings: &Settings,
        analysis: &Analysis,
        reference_hover: Option<shakmaty::uci::UciMove>,
        actions: &mut Vec<GameAction>,
    ) {
        let preview_pos = self.preview.as_ref().map(|p| p.position());
        let Some(game_pos) = self.game.position(self.cursor) else {
            return;
        };
        let pos: Chess = preview_pos.unwrap_or_else(|| game_pos.clone());
        let over = game_over(game_pos);

        let mut arrows = Vec::new();
        let mut circles = Vec::new();
        let last_move;
        if let Some(p) = &self.preview {
            last_move = p
                .shown
                .checked_sub(1)
                .and_then(|i| p.moves.get(i))
                .and_then(move_squares);
            if let Some(next) = p.moves.get(p.shown).and_then(move_squares) {
                arrows.push(Arrow {
                    from: next.0,
                    to: next.1,
                    color: Color32::from_rgba_unmultiplied(0, 48, 136, 160),
                    weight: 1.0,
                });
            }
        } else {
            last_move = match self.game.game_move(self.cursor).map(|m| m.kind) {
                Some(MoveKind::Normal(m)) => move_squares(&m),
                _ => None,
            };
            if let Some(node) = self.game.node(self.cursor) {
                for c in node.comments_after() {
                    for part in parse_comment(c) {
                        match part {
                            CommentPart::Annotation(Annotation::Arrows(list)) => {
                                for (color, from, to) in list {
                                    arrows.push(Arrow {
                                        from,
                                        to,
                                        color: shape_color(color),
                                        weight: 0.9,
                                    });
                                }
                            }
                            CommentPart::Annotation(Annotation::Squares(list)) => {
                                circles
                                    .extend(list.into_iter().map(|(c, sq)| (sq, shape_color(c))));
                            }
                            _ => {}
                        }
                    }
                }
            }
            // With the live engine on, only its arrows are drawn: two sources of engine
            // arrows on one board would overlap and compete.
            if self.review.visible && !analysis.is_on() {
                // The opponent's best reply to a marked move (strong) and the best answer to it.
                let line = self.review.refutation(self.cursor, &self.game);
                for (k, m) in line.iter().enumerate() {
                    if let Some(sq) = move_squares(m) {
                        engine_arrow(
                            &mut arrows,
                            sq,
                            if k == 0 { 200 } else { 100 },
                            if k == 0 { 1.0 } else { 0.75 },
                        );
                    }
                }
            }
            if settings.show_best_arrow && analysis.best().is_some() {
                for (i, line) in analysis.lines.iter().take(3).enumerate() {
                    if let Some(sq) = line.moves.first().and_then(move_squares) {
                        let alpha = if i == 0 { 200 } else { 100 };
                        engine_arrow(&mut arrows, sq, alpha, if i == 0 { 1.0 } else { 0.7 });
                    }
                }
            }
        }

        // A continuation under the pointer in the reference tab (blue, like the library's).
        if self.preview.is_none()
            && let Some(m) = reference_hover.and_then(|u| u.to_move(&pos).ok())
            && let Some((from, to)) = move_squares(&m)
        {
            arrows.push(Arrow {
                from,
                to,
                color: crate::board_search::PREVIEW_ARROW,
                weight: 1.0,
            });
        }

        // Center the eval bar + board horizontally between the left edge and the side panel.
        let area = ui.available_rect_before_wrap();
        let show_bar = analysis.is_on();
        let bar_w = if show_bar { 18.0 } else { 0.0 };
        let gap = if show_bar { 8.0 } else { 0.0 };
        let show_graph = self.review.visible && (self.review.has_data() || self.review.running());
        let graph_h = if show_graph {
            (area.height() * 0.14).clamp(80.0, 130.0)
        } else {
            0.0
        };
        let graph_gap = if show_graph { 8.0 } else { 0.0 };
        let side = (area.width() - bar_w - gap)
            .min(area.height() - graph_h - graph_gap)
            .max(BOARD_MIN);
        let total = bar_w + gap + side;
        let left = area.left() + ((area.width() - total) / 2.0).max(0.0).floor();
        let group = egui::Rect::from_min_size(egui::pos2(left, area.top()), vec2(total, side));
        let builder = egui::UiBuilder::new()
            .max_rect(group)
            .layout(egui::Layout::left_to_right(egui::Align::Min));
        ui.scope_builder(builder, |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if show_bar {
                let (rect, _) = ui.allocate_exact_size(vec2(bar_w, side), Sense::hover());
                let frac = match over {
                    Some(GameOver::Mated(loser)) => {
                        if loser == Color::White {
                            0.0
                        } else {
                            1.0
                        }
                    }
                    Some(GameOver::Draw(_)) => 0.5,
                    None => analysis
                        .best()
                        .map(|l| analysis::bar_fraction(&l.score))
                        .unwrap_or(0.5),
                };
                let frac = if self.flipped { 1.0 - frac } else { frac };
                let painter = ui.painter();
                let (top_c, bottom_c) = if self.flipped {
                    (Color32::from_gray(235), Color32::from_gray(40))
                } else {
                    (Color32::from_gray(40), Color32::from_gray(235))
                };
                painter.rect_filled(rect, 2.0, top_c);
                let split = rect.bottom() - rect.height() * frac;
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(rect.left(), split), rect.max),
                    2.0,
                    bottom_c,
                );
                painter.line_segment(
                    [
                        egui::pos2(rect.left(), rect.center().y),
                        egui::pos2(rect.right(), rect.center().y),
                    ],
                    egui::Stroke::new(1.0, Color32::from_rgb(200, 60, 60)),
                );
            }
            ui.allocate_ui(vec2(side, side), |ui| {
                let view = BoardView {
                    pos: &pos,
                    orientation: if self.flipped {
                        Color::Black
                    } else {
                        Color::White
                    },
                    last_move,
                    arrows: &arrows,
                    circles: &circles,
                    badge: if self.preview.is_none() {
                        self.badge()
                    } else {
                        None
                    },
                    interactive: self.preview.is_none() && self.user_may_move(),
                    show_coords: settings.show_coords,
                    theme: settings.board_theme.theme(),
                    pieces: settings.piece_set,
                    animation: settings.animation.seconds(),
                };
                if let Some(m) = view.show(ui, &mut self.board) {
                    self.play_move(m, actions);
                }
                if let Some(drawn) = self.board.take_shape()
                    && self.preview.is_none()
                    && self.play.is_none()
                {
                    self.apply_shape(drawn);
                }
            });
        });
        if show_graph {
            let graph_rect = egui::Rect::from_min_size(
                egui::pos2(left, area.top() + side + graph_gap),
                vec2(total, graph_h),
            );
            let target = ui
                .scope_builder(egui::UiBuilder::new().max_rect(graph_rect), |ui| {
                    self.review
                        .graph(ui, graph_rect.size(), self.cursor, &self.game)
                })
                .inner;
            if let Some(id) = target {
                self.goto(id);
            }
        }
    }

    fn side_panel(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        analysis: &mut Analysis,
        reference: &mut crate::reference::Reference,
        hub: &mut crate::translate_ui::Hub,
        actions: &mut Vec<GameAction>,
    ) {
        if self.play.is_some() {
            self.play_side(ui, settings, analysis, reference, hub, actions);
            return;
        }
        if self.play_end.is_some() {
            egui::Panel::top("play_result").show(ui, |ui| self.result_strip(ui, actions));
        }
        egui::Panel::bottom("game_toolbar").show(ui, |ui| self.toolbar(ui, actions));
        egui::Panel::top("engine_panel")
            .show(ui, |ui| self.engine_panel(ui, settings, analysis, actions));
        let report = self.review.visible && (self.review.has_data() || self.review.running());
        if !report && self.side_tab == SideTab::Report {
            self.side_tab = SideTab::Notation;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.side_tab, SideTab::Notation, tr("Notation"));
                ui.selectable_value(&mut self.side_tab, SideTab::Reference, tr("Reference"))
                    .on_hover_text(tr("What was played in this position in a database"));
                if report {
                    ui.selectable_value(&mut self.side_tab, SideTab::Report, tr("Report"));
                }
            });
            ui.separator();
            if self.side_tab == SideTab::Reference {
                let source = self.source.as_ref().map(|s| (s.path.as_path(), s.index));
                let action = egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        reference.ui(
                            ui,
                            &self.game,
                            self.cursor,
                            settings.reference_db.as_deref(),
                            source,
                        )
                    })
                    .inner;
                match action {
                    Some(RefAction::Play(m)) => self.play_move(m, actions),
                    Some(RefAction::Goto(id)) => self.goto(id),
                    Some(RefAction::Choose(p)) => actions.push(GameAction::ReferenceChoose(p)),
                    Some(RefAction::Load(p)) => actions.push(GameAction::ReferenceLoad(p)),
                    Some(RefAction::OpenGame(p, i, at)) => {
                        actions.push(GameAction::OpenGame(p, i, at))
                    }
                    Some(RefAction::FindInLibrary(p)) => {
                        actions.push(GameAction::FindPosition(Some(p)))
                    }
                    None => {}
                }
                return;
            }
            if self.side_tab == SideTab::Report {
                let action = egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.review.report_ui(ui, &self.game))
                    .inner;
                match action {
                    Some(ReportAction::Goto(id)) => self.goto(id),
                    Some(ReportAction::Write) => {
                        let n = self.review.write_to_game(&mut self.game);
                        actions.push(GameAction::Toast(trf(
                            "Analysis written to the game: {n} marked moves (Ctrl+Z to undo)",
                            &[("n", &n)],
                        )));
                    }
                    Some(ReportAction::Update) => {
                        self.start_review(settings, false, ui.ctx(), actions)
                    }
                    Some(ReportAction::Reanalyse) => self.review.dialog_open = true,
                    None => {}
                }
                return;
            }
            match self.translation.strip(ui, settings, hub) {
                Some(crate::translate_ui::StripAction::OpenSettings) => {
                    actions.push(GameAction::OpenTranslationSettings)
                }
                Some(crate::translate_ui::StripAction::Toast(t)) => {
                    actions.push(GameAction::Toast(t))
                }
                None => {}
            }
            if std::mem::take(&mut self.translate_now) {
                self.translation.translate(ui.ctx(), settings, hub);
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let resp = Notation {
                        game: &self.game,
                        cursor: self.cursor,
                        scroll_to_cursor: self.scroll_to_cursor,
                        marks: self.review.visible.then_some(&self.review.analysis.marks),
                        preview: self
                            .preview
                            .as_ref()
                            .map(|p| (p.base, p.san.as_slice(), p.shown)),
                        translations: self.translation.shown(),
                        editor: self.editor.as_mut(),
                        read_only: self.play.is_some(),
                    }
                    .show(ui);
                    if let Some(id) = resp.clicked {
                        self.goto(id);
                    }
                    match resp.editor {
                        Some(EditorOutcome::Done) => self.commit_editor(),
                        Some(EditorOutcome::Cancel) => self.cancel_editor(),
                        None => {}
                    }
                    if resp.insert_preview {
                        self.insert_preview(actions);
                    }
                    if let Some(slot) = resp.edit {
                        self.open_editor(slot);
                    }
                    if let Some(slot) = resp.delete {
                        self.delete_comment(slot, actions);
                    }
                    if let Some((id, nag)) = resp.nag {
                        self.toggle_nag_on(id, nag);
                    }
                    if resp.best.is_some() {
                        self.commit_editor();
                    }
                    if let Some((id, shown)) = resp.best
                        && let Some(mark) = self.review.analysis.marks.get(&id)
                        && let Some(root) = self.game.position(mark.base)
                        && !mark.best.is_empty()
                    {
                        self.cursor = mark.base;
                        self.scroll_to_cursor = false;
                        self.board.clear();
                        self.preview = Some(Preview {
                            base: mark.base,
                            root: root.clone(),
                            moves: mark.best.clone(),
                            san: mark.best_san.clone(),
                            shown: shown.min(mark.best.len()),
                        });
                    }
                    if let Some((id, cmd)) = resp.context {
                        let result = match cmd {
                            NotationCommand::Promote => {
                                self.game.promote_variation(id).map(|_| self.cursor)
                            }
                            NotationCommand::Delete => self.game.delete_variation(id),
                        };
                        match result {
                            Ok(cursor) => {
                                // Deleting may detach the cursor; fall back to the returned parent.
                                let c = if self.game.is_attached(self.cursor) {
                                    self.cursor
                                } else {
                                    cursor
                                };
                                self.goto(c);
                            }
                            Err(e) => actions.push(GameAction::Toast(e.to_string())),
                        }
                    }
                });
        });
    }

    fn engine_panel(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        analysis: &mut Analysis,
        actions: &mut Vec<GameAction>,
    ) {
        ui.add_space(4.0);
        let mut below = None;
        ui.horizontal(|ui| {
            let mut on = analysis.is_on();
            let label = if on {
                tr("Analysis: on")
            } else {
                tr("Analysis: off")
            };
            if ui
                .toggle_value(&mut on, label)
                .on_hover_text(tr("Turn the engine on/off (L)"))
                .changed()
            {
                actions.push(GameAction::ToggleEngine);
            }
            match self.game.position(self.cursor).and_then(game_over) {
                Some(over) if analysis.is_on() => {
                    ui.label(RichText::new(over.label()).strong().size(18.0));
                }
                _ => {
                    if let Some(best) = analysis.best() {
                        ui.label(
                            RichText::new(analysis::format_eval(&best.score))
                                .strong()
                                .size(20.0),
                        );
                    }
                }
            }
            // The engine's name and how it goes: beside the evaluation (two small lines,
            // or one centred line when there is no status); in a narrow panel, on a line
            // of their own under the row, shortened.
            let name = if analysis.is_on() {
                analysis.engine_name.clone()
            } else {
                settings
                    .active_engine()
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| tr("No engine selected").into())
            };
            let status = match &analysis.status {
                Status::Starting => Some(RichText::new(tr("starting…")).small().weak()),
                Status::Running => {
                    let depth = analysis.best().map(|l| l.depth).unwrap_or(0);
                    let tail = if analysis.finished {
                        tr(" · done").to_string()
                    } else {
                        trf(
                            " · {n} Mn/s",
                            &[("n", &format!("{:.1}", analysis.nps as f64 / 1e6))],
                        )
                    };
                    Some(
                        RichText::new(format!("{}{tail}", trf("Depth {n}", &[("n", &depth)])))
                            .small()
                            .weak(),
                    )
                }
                Status::Failed(e) => {
                    Some(RichText::new(e).small().color(ui.visuals().error_fg_color))
                }
                Status::Off => self.review.running().then(|| {
                    RichText::new(tr("paused: the game is being analysed"))
                        .small()
                        .weak()
                }),
            };
            let small = |t: &str| text_width(ui, t, egui::TextStyle::Small);
            let block = small(&name).max(status.as_ref().map_or(0.0, |s| small(s.text())));
            let gear = ui
                .painter()
                .layout_no_wrap("⚙".into(), egui::FontId::proportional(18.0), Color32::WHITE)
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
                + ui.spacing().item_spacing.x;
            let beside = block + ui.spacing().item_spacing.x <= ui.available_width() - gear;
            if beside {
                match &status {
                    Some(st) => {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&name).small());
                            ui.label(st.clone());
                        });
                    }
                    None => {
                        ui.label(RichText::new(&name).small());
                    }
                }
            } else {
                below = Some((name, status));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(
                    &mut self.engine_settings_open,
                    RichText::new("⚙").size(18.0),
                )
                .on_hover_text(tr("Engine settings"));
            });
        });
        if let Some((name, status)) = below {
            ui.horizontal(|ui| {
                let name = RichText::new(name).small();
                match status {
                    Some(st) => {
                        ui.label(name);
                        ui.add(egui::Label::new(st).truncate());
                    }
                    None => {
                        ui.add(egui::Label::new(name).truncate());
                    }
                }
            });
        }
        if self.engine_settings_open {
            engine_settings(ui, settings, actions);
        }
        if analysis.is_on() {
            let base = self.cursor;
            let root = self.game.position(base).cloned();
            let lines: Vec<PvLine> = analysis.lines.clone();
            let mut insert = false;
            for line in &lines {
                let previewed = self
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.base == base && p.san == line.san);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    // The line being stepped through on the board: added from here.
                    if previewed
                        && ui
                            .small_button(tr("+ Add as variation"))
                            .on_hover_text(tr(
                                "Puts the line into the game as a variation (Ctrl+Z to undo)",
                            ))
                            .clicked()
                    {
                        insert = true;
                    }
                    ui.label(
                        RichText::new(analysis::format_eval(&line.score))
                            .strong()
                            .monospace(),
                    );
                    let start_num = root.as_ref().map(|p| (p.fullmoves().get(), p.turn()));
                    // No bar popping up over the next line: a long line is cut at the
                    // edge (it still scrolls with Shift+wheel or by dragging).
                    egui::ScrollArea::horizontal()
                        .id_salt(("pv", line.multipv))
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (i, san) in line.san.iter().enumerate().take(14) {
                                    let label = match start_num {
                                        Some((n, turn)) => {
                                            let ply = i as u32 + u32::from(turn == Color::Black);
                                            let num = n + ply / 2;
                                            if ply.is_multiple_of(2) {
                                                format!("{num}.{san}")
                                            } else if i == 0 {
                                                format!("{num}…{san}")
                                            } else {
                                                san.clone()
                                            }
                                        }
                                        None => san.clone(),
                                    };
                                    let label = crate::san::show(&label).into_owned();
                                    let selected = self.preview.as_ref().is_some_and(|p| {
                                        p.base == base && p.shown == i + 1 && p.san == line.san
                                    });
                                    let resp = ui.add(
                                        egui::Button::selectable(selected, label)
                                            .frame_when_inactive(false),
                                    );
                                    if resp.clicked()
                                        && let Some(root) = &root
                                    {
                                        self.commit_editor();
                                        self.preview = Some(Preview {
                                            base,
                                            root: root.clone(),
                                            moves: line.moves.clone(),
                                            san: line.san.clone(),
                                            shown: i + 1,
                                        });
                                        self.board.clear();
                                    }
                                }
                            });
                        });
                });
            }
            if insert {
                self.insert_preview(actions);
            }
        }
        ui.add_space(4.0);
    }

    fn insert_preview(&mut self, actions: &mut Vec<GameAction>) {
        let Some(p) = self.preview.take() else { return };
        match self.game.add_line(p.base, &p.moves) {
            Ok(_) => {
                let mut cur = p.base;
                for m in &p.moves[..p.shown] {
                    match self.game.find_child(cur, *m) {
                        Some(c) => cur = c,
                        None => break,
                    }
                }
                self.goto(cur);
                actions.push(GameAction::Toast(
                    tr("Line added as a variation (Ctrl+Z to undo)").into(),
                ));
            }
            Err(e) => actions.push(GameAction::Toast(trf(
                "Could not add the line: {e}",
                &[("e", &e)],
            ))),
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, actions: &mut Vec<GameAction>) {
        ui.add_space(4.0);
        // Wraps onto a second row when the side panel is narrow, a group at a time
        // (moves, board and edits, copying, the game analysis): a group that does not
        // fit the rest of the row starts the next one, without a separator.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let big = |s: &str| RichText::new(s).size(18.0);
            let next_group = |ui: &mut Ui, icons: &[&str], extra: f32| {
                let font = egui::FontId::proportional(18.0);
                let gap = ui.spacing().item_spacing.x;
                let width: f32 = icons
                    .iter()
                    .map(|i| {
                        ui.painter()
                            .layout_no_wrap((*i).to_owned(), font.clone(), Color32::WHITE)
                            .size()
                            .x
                            + 2.0 * ui.spacing().button_padding.x
                            + gap
                    })
                    .sum::<f32>()
                    + extra;
                if ui.available_size_before_wrap().x < width + 2.0 * gap + 1.0 {
                    ui.end_row();
                } else {
                    ui.separator();
                }
            };
            if ui
                .button(big("⏮"))
                .on_hover_text(tr("To start (Home)"))
                .clicked()
            {
                self.goto(NodeId::ROOT);
            }
            if ui.button(big("◀")).on_hover_text(tr("Back (←)")).clicked() {
                self.step_back();
            }
            if ui
                .button(big("▶"))
                .on_hover_text(tr("Forward (→)"))
                .clicked()
            {
                self.step_forward(actions);
            }
            if ui
                .button(big("⏭"))
                .on_hover_text(tr("To the end of the line (End)"))
                .clicked()
            {
                let end = self.game.line_end(self.cursor);
                self.goto(end);
            }
            next_group(ui, &["🔃", "↩", "↪"], 0.0);
            if ui
                .button(big("🔃"))
                .on_hover_text(tr("Flip board (F)"))
                .clicked()
            {
                self.flipped = !self.flipped;
            }
            // Not while a comment is being written: it would undo what stands under it.
            let no_editor = self.editor.is_none();
            if ui
                .add_enabled(
                    self.game.can_undo() && no_editor,
                    egui::Button::new(big("↩")),
                )
                .on_hover_text(tr("Undo move/edit (Ctrl+Z)"))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(
                    self.game.can_redo() && no_editor,
                    egui::Button::new(big("↪")),
                )
                .on_hover_text(tr("Redo move/edit (Ctrl+Y)"))
                .clicked()
            {
                self.redo();
            }
            next_group(ui, &["☰", "💾", "📋"], 0.0);
            ui.menu_button(big("☰"), |ui| {
                if ui.button(tr("Game data… (F2)")).clicked() {
                    actions.push(GameAction::EditGameData);
                    ui.close();
                }
                let comment = if self.cursor == NodeId::ROOT {
                    tr("Comment at the start of the game… (Enter)")
                } else {
                    tr("Comment on the move… (Enter)")
                };
                if ui.button(comment).clicked() {
                    self.open_editor(Slot::After(self.cursor));
                    ui.close();
                }
                let writable_source = self.source.as_ref().is_some_and(|s| s.block.is_none());
                if (writable_source || (self.source.is_none() && self.target.is_some()))
                    && ui.button(self.save_hint()).clicked()
                {
                    actions.push(GameAction::Save);
                    ui.close();
                }
                if ui.button(tr("Add to database… (Ctrl+Shift+S)")).clicked() {
                    actions.push(GameAction::AddToDatabase);
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("Copy PGN")).clicked() {
                    actions.push(self.copy_pgn());
                    ui.close();
                }
                if ui.button(tr("Copy FEN")).clicked() {
                    if let Some(fen) = self.game.fen(self.cursor) {
                        actions.push(GameAction::CopyText(fen, tr("FEN copied")));
                    }
                    ui.close();
                }
                if !self.diagnostics.is_empty() && ui.button(tr("Compatibility notes")).clicked() {
                    self.show_diagnostics = true;
                    ui.close();
                }
                ui.separator();
                if self.translation.can_translate() && ui.button(tr("Translate comments")).clicked()
                {
                    self.translate_now = true;
                    self.side_tab = SideTab::Notation;
                    ui.close();
                }
                if self.translation.has_translation() {
                    let label = if self.translation.original {
                        tr("Show the translation (Ctrl+Shift+T)")
                    } else {
                        tr("Show the original (Ctrl+Shift+T)")
                    };
                    if ui.button(label).clicked() {
                        self.translation.toggle();
                        ui.close();
                    }
                }
                if ui.button(tr("Translation settings…")).clicked() {
                    actions.push(GameAction::OpenTranslationSettings);
                    ui.close();
                }
            });
            if ui
                .button(big("💾"))
                .on_hover_text(self.save_hint())
                .clicked()
            {
                actions.push(GameAction::Save);
            }
            if ui
                .button(big("📋"))
                .on_hover_text(tr("Copy PGN (Ctrl+Shift+C)"))
                .clicked()
            {
                actions.push(self.copy_pgn());
            }
            // A running analysis shows its progress (70) and its × (22) instead.
            if self.review.progress().is_some() {
                next_group(ui, &[], 70.0 + 22.0 + 2.0 * 5.0);
            } else {
                next_group(ui, &["📈"], 0.0);
            }
            self.review_button(ui, actions);
        });
        ui.add_space(4.0);
    }

    /// Moves the cursor `plies` steps along the main line (dev startup automation).
    pub fn debug_open_engine_settings(&mut self) {
        self.engine_settings_open = true;
    }

    /// Shows the Report tab (with the graph) when an analysis is available.
    pub fn debug_show_report(&mut self) {
        if self.review.has_data() {
            self.review.visible = true;
            self.side_tab = SideTab::Report;
        }
    }

    /// Opens the better line of the first engine-marked move up to its `shown`-th move.
    pub fn debug_best_line(&mut self, shown: usize) {
        let mark = self
            .game
            .mainline()
            .into_iter()
            .find_map(|id| self.review.analysis.marks.get(&id).cloned());
        self.commit_editor();
        if let Some(mark) = mark
            && let Some(root) = self.game.position(mark.base)
            && !mark.best.is_empty()
        {
            self.cursor = mark.base;
            self.preview = Some(Preview {
                base: mark.base,
                root: root.clone(),
                shown: shown.clamp(1, mark.best.len()),
                moves: mark.best,
                san: mark.best_san,
            });
        }
    }

    pub fn debug_review(&mut self, settings: &Settings, ctx: &egui::Context, start: bool) {
        if start {
            self.start_review(settings, false, ctx, &mut Vec::new());
        } else {
            self.review.dialog_open = true;
        }
    }

    /// Starts the game analysis right away (after an engine was installed for it).
    pub fn start_review_now(
        &mut self,
        settings: &Settings,
        ctx: &egui::Context,
    ) -> Vec<GameAction> {
        let mut actions = Vec::new();
        self.start_review(settings, true, ctx, &mut actions);
        actions
    }

    /// Moves the cursor to the end of the main line.
    pub fn goto_main_line_end(&mut self) {
        let end = self.game.line_end(self.game.root());
        self.goto(end);
    }

    /// Moves the cursor to the first node showing `target`: the main line first, then
    /// the variations. Returns false if the game never reaches the position.
    pub fn goto_position(&mut self, target: &Chess) -> bool {
        let root = self.game.root();
        let found = std::iter::once(root)
            .chain(self.game.mainline())
            .chain(self.game.attached_nodes())
            .find(|&id| {
                self.game
                    .position(id)
                    .is_some_and(|p| chess_import::position::same_position(p, target))
            });
        match found {
            Some(id) => {
                self.goto(id);
                true
            }
            None => false,
        }
    }

    /// The cursor's position as a line of moves from the game's start, for searching
    /// it in a database; `None` after a null move or in a game that cannot be searched.
    /// The reference tab is shown (kept for the next game opened from it).
    pub fn reference_tab(&self) -> bool {
        self.side_tab == SideTab::Reference
    }

    pub fn show_reference_tab(&mut self) {
        self.side_tab = SideTab::Reference;
    }

    pub fn search_line(&self) -> Option<(Chess, Vec<shakmaty::Move>, bool)> {
        let (start, line) = crate::board_search::line_to(&self.game, self.cursor)?;
        Some((start, line, self.flipped))
    }

    pub fn debug_advance(&mut self, plies: usize) {
        for _ in 0..plies {
            match self.game.main_child(self.cursor) {
                Some(n) => self.cursor = n,
                None => break,
            }
        }
        if std::env::var("CP_VAR").is_ok()
            && let Some(v) = self.game.next_sibling(self.cursor)
        {
            self.cursor = self.game.main_child(v).unwrap_or(v);
        }
    }

    // ----- playing against Maia (docs/design/play-mode.md) -----------------------------

    /// Starts a game against Maia in this (new, empty) game.
    pub fn start_play(
        &mut self,
        start: crate::play::Start,
        weights: std::path::PathBuf,
        ctx: &egui::Context,
    ) {
        use crate::play::{Clock, Opponent, Session};
        let maia = format!("Maia {}", start.strength);
        let you = if start.user_name.is_empty() {
            "?".to_owned()
        } else {
            start.user_name.clone()
        };
        let (white, black) = match start.user {
            Color::White => (you, maia),
            Color::Black => (maia, you),
        };
        let event = match start.clock {
            Some(tc) => trf(
                "Game vs computer, {speed}",
                &[("speed", &tc.speed().name().to_lowercase())],
            ),
            None => tr("Game vs computer").to_owned(),
        };
        let g = &mut self.game;
        g.set_tag("Event", &event);
        g.set_tag("Site", "ChessPenguin");
        g.set_tag("White", &white);
        g.set_tag("Black", &black);
        let (we, be) = match start.user {
            Color::White => (start.user_elo, Some(start.strength)),
            Color::Black => (Some(start.strength), start.user_elo),
        };
        if let Some(e) = we {
            g.set_tag("WhiteElo", &e.to_string());
        }
        if let Some(e) = be {
            g.set_tag("BlackElo", &e.to_string());
        }
        let (wt, bt) = match start.user {
            Color::White => ("human", "program"),
            Color::Black => ("program", "human"),
        };
        g.set_tag("WhiteType", wt);
        g.set_tag("BlackType", bt);
        g.set_tag(
            "TimeControl",
            &start.clock.map_or_else(|| "-".to_owned(), |tc| tc.pgn()),
        );
        g.mark_saved();
        self.flipped = start.user == Color::Black;
        self.play_end = None;
        self.play = Some(Session {
            user: start.user,
            strength: start.strength,
            user_elo: start.user_elo,
            clock: start.clock.map(Clock::new),
            opponent: Opponent::start(weights, ctx),
            resign_asked: false,
        });
        self.play_start = Some(start);
        self.ask_maia();
    }

    /// The positions of the main line, from the start to its end.
    fn mainline_positions(&self) -> Vec<Chess> {
        let mut out = Vec::new();
        if let Some(p) = self.game.position(NodeId::ROOT) {
            out.push(p.clone());
        }
        for id in self.game.mainline() {
            if let Some(p) = self.game.position(id) {
                out.push(p.clone());
            }
        }
        out
    }

    fn play_end_node(&self) -> NodeId {
        self.game.line_end(NodeId::ROOT)
    }

    /// The side to move at the end of the main line.
    fn play_turn(&self) -> Color {
        self.game
            .position(self.play_end_node())
            .map_or(Color::White, |p| p.turn())
    }

    /// The user may move: their turn, at the current position, Maia not thinking.
    fn user_may_move(&self) -> bool {
        match &self.play {
            None => true,
            Some(s) => {
                self.play_leave.is_none()
                    && self.play_turn() == s.user
                    && self.cursor == self.play_end_node()
                    && !s.opponent.thinking
            }
        }
    }

    /// Asks Maia for a move when it is her turn (dev hook `CP_PLAY_AUTO`: for the user
    /// too, quickly — a whole game without a mouse).
    fn ask_maia(&mut self) {
        let turn = self.play_turn();
        let positions = self.mainline_positions();
        let ply = positions.len() - 1;
        let only = positions.last().is_some_and(|p| p.legal_moves().len() == 1);
        let auto = std::env::var_os("CP_PLAY_AUTO").is_some();
        let Some(s) = self.play.as_mut() else { return };
        if turn == s.user {
            if auto {
                let elo = s.user_elo.unwrap_or(s.strength);
                s.opponent.ask(
                    positions,
                    elo,
                    s.strength,
                    std::time::Duration::from_millis(30),
                );
            }
            return;
        }
        let now = std::time::Instant::now();
        let clock = s
            .clock
            .as_ref()
            .map(|c| (c.left(turn, now), c.control.increment));
        let pause = if auto {
            std::time::Duration::from_millis(30)
        } else {
            crate::play::pause(clock, ply, only)
        };
        let oppo = s.user_elo.unwrap_or(s.strength);
        s.opponent.ask(positions, s.strength, oppo, pause);
    }

    /// A move at the end of the main line by the side to move: the clock, `[%clk]`, the
    /// sound, the end of the game, Maia's turn.
    fn play_add(
        &mut self,
        m: Move,
        spent: Option<std::time::Duration>,
        actions: &mut Vec<GameAction>,
    ) {
        let end = self.play_end_node();
        let mover = self.play_turn();
        let Ok(id) = self.game.add_move(end, m) else {
            return;
        };
        let now = std::time::Instant::now();
        if let Some(s) = self.play.as_mut() {
            s.resign_asked = false;
            if let Some(c) = s.clock.as_mut() {
                c.moved(mover, now, spent);
                let left = c.left(mover, now);
                let _ = self
                    .game
                    .annotate(id, Vec::new(), vec![crate::play::clk_command(left)]);
            }
        }
        self.goto(id);
        if let Some(snd) = self.sound_for(id) {
            actions.push(GameAction::Sound(snd));
        }
        if let Some(e) = crate::play::ending(&self.mainline_positions()) {
            self.finish_play(e, actions);
        } else {
            self.ask_maia();
        }
    }

    fn user_move(&mut self, m: Move, actions: &mut Vec<GameAction>) {
        if !self.user_may_move() {
            // A move in a past position: back to the game.
            let end = self.play_end_node();
            self.goto(end);
            return;
        }
        self.play_add(m, None, actions);
    }

    /// The clock and Maia's answers, each frame.
    fn play_tick(&mut self, ctx: &egui::Context, actions: &mut Vec<GameAction>) {
        if self.play_leave.is_some() {
            return;
        }
        let Some(s) = self.play.as_mut() else { return };
        if let Some(err) = s.opponent.failed() {
            crate::maia_download::forget_broken();
            actions.push(GameAction::Toast(trf(
                "Maia did not start: {e}",
                &[("e", &err)],
            )));
            self.play = None;
            self.game.mark_saved();
            actions.push(GameAction::Back);
            return;
        }
        if let Some((m, pause)) = s.opponent.answer() {
            // Maia's clock takes her pause, not the program's computing.
            self.play_add(m, Some(pause), actions);
        }
        let now = std::time::Instant::now();
        let end_pos = self.game.position(self.play_end_node()).cloned();
        let Some(s) = self.play.as_mut() else { return };
        if let Some(c) = &s.clock
            && let Some(side) = c.running()
        {
            let left = c.left(side, now);
            if left.is_zero() {
                // Time is up; a draw when the other side cannot mate (FIDE 6.9).
                let draw = end_pos.is_some_and(|p| p.has_insufficient_material(!side));
                self.finish_play(crate::play::Ending::Time { loser: side, draw }, actions);
                return;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The game is over: the result, the tags, the strip; the screen becomes an ordinary
    /// new game.
    fn finish_play(&mut self, ending: crate::play::Ending, actions: &mut Vec<GameAction>) {
        let Some(mut s) = self.play.take() else {
            return;
        };
        s.opponent.forget();
        let now = std::time::Instant::now();
        let left = s.clock.as_mut().map(|c| {
            c.stop(now);
            (c.left(s.user, now), c.left(!s.user, now))
        });
        self.game.set_result(ending.result());
        self.game.set_tag("Termination", ending.termination());
        self.game.forget_history();
        let end = self.play_end_node();
        self.goto(end);
        if matches!(ending, crate::play::Ending::Checkmate { .. }) {
            actions.push(GameAction::Sound(Sound::Checkmate));
        }
        if let Some(start) = self.play_start.clone() {
            self.play_end = Some(crate::play::End {
                ending,
                user: s.user,
                start,
                left,
            });
        }
    }

    /// Before both sides have moved there is nothing to lose: the game is dropped.
    fn play_started(&self) -> bool {
        self.game.mainline().len() >= 2
    }

    /// Resigns the game in progress (leaving it, closing the program); before the first
    /// moves it is dropped instead. The game stays as played.
    pub fn resign_if_playing(&mut self) {
        let Some(s) = &self.play else { return };
        if self.play_started() {
            let user = s.user;
            let mut sink = Vec::new();
            self.finish_play(crate::play::Ending::Resigned { loser: user }, &mut sink);
        } else {
            self.play = None;
            self.game.mark_saved();
        }
    }

    /// Takes back the user's last move and Maia's answer to it (no variation is left).
    fn take_back(&mut self) {
        let Some(s) = self.play.as_mut() else { return };
        s.opponent.forget();
        let user = s.user;
        // The last move of the user on the main line.
        let line = self.game.mainline();
        let last = line.iter().rev().find(|&&id| {
            self.game
                .parent(id)
                .and_then(|p| self.game.position(p))
                .is_some_and(|p| p.turn() == user)
        });
        if let Some(&id) = last
            && let Ok(parent) = self.game.delete_variation(id)
        {
            self.goto(parent);
            let moved = self.game.mainline().iter().any(|&n| {
                self.game
                    .parent(n)
                    .and_then(|p| self.game.position(p))
                    .is_some_and(|p| p.turn() == user)
            });
            if let Some(c) = self.play.as_mut().and_then(|s| s.clock.as_mut()) {
                c.take_back(user, moved, std::time::Instant::now());
            }
        }
        self.ask_maia();
    }

    /// Keys during a game: looking through it, flipping, the take-back; Esc asks.
    fn play_keys(&mut self, ui: &Ui, actions: &mut Vec<GameAction>) {
        let ctrl = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let mut cmds = Vec::new();
        ui.input_mut(|i| {
            if i.consume_shortcut(&ctrl(Key::Z)) {
                cmds.push("takeback");
            }
            if i.consume_shortcut(&ctrl(Key::W)) {
                cmds.push("leave");
            }
            if i.modifiers.is_none() {
                for (key, cmd) in [
                    (Key::ArrowLeft, "back1"),
                    (Key::ArrowRight, "fwd1"),
                    (Key::ArrowUp, "end"),
                    (Key::ArrowDown, "home"),
                    (Key::Home, "home"),
                    (Key::End, "end"),
                    (Key::F, "flip"),
                    (Key::L, "engine"),
                    (Key::Escape, "leave"),
                ] {
                    if i.key_pressed(key) {
                        cmds.push(cmd);
                    }
                }
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::A)) {
                cmds.push("engine");
            }
        });
        for cmd in cmds {
            match cmd {
                "takeback" => self.take_back(),
                "back1" => self.step_back(),
                "fwd1" => self.step_forward(actions),
                "home" => self.goto(NodeId::ROOT),
                "end" => {
                    let end = self.play_end_node();
                    self.goto(end);
                }
                "flip" => self.flipped = !self.flipped,
                "engine" => actions.push(GameAction::Toast(
                    tr("The engine is off during the game — analyse after it ends").to_owned(),
                )),
                "leave" => {
                    if let Some(s) = self.play.as_mut()
                        && s.resign_asked
                    {
                        s.resign_asked = false;
                    } else {
                        ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
                        actions.push(GameAction::Back);
                    }
                }
                _ => {}
            }
        }
    }

    /// Leaving during a game asks first (play-mode.md §6); before the first moves of
    /// both sides the game is just dropped.
    fn guard_leaving(&mut self, actions: &mut Vec<GameAction>) {
        if self.play.is_none() {
            return;
        }
        let leaving = actions
            .iter()
            .position(|a| matches!(a, GameAction::Back | GameAction::ReturnToPrevious));
        if let Some(k) = leaving {
            let a = actions.remove(k);
            if self.play_started() {
                let intent = if matches!(a, GameAction::ReturnToPrevious) {
                    crate::app::CloseIntent::ReturnToPrevious
                } else {
                    crate::app::CloseIntent::BackToLibrary
                };
                self.ask_leave(intent);
            } else {
                if let Some(s) = self.play.as_mut() {
                    s.opponent.forget();
                }
                self.play = None;
                self.game.mark_saved();
                actions.push(a);
            }
        }
    }

    /// Leaving a game in progress (any way: back, the window closed, …) asks first;
    /// the clocks wait while the question is open.
    pub fn ask_leave(&mut self, intent: crate::app::CloseIntent) {
        if self.play_leave.is_none() {
            self.play_leave = Some((intent, std::time::Instant::now()));
        }
    }

    /// A game is in progress and its first moves are made (leaving it costs a result).
    pub fn playing_started(&self) -> bool {
        self.play.is_some() && self.play_started()
    }

    /// «The game is on»: resign and add, resign and leave, keep playing.
    fn leave_question(&mut self, ctx: &egui::Context, actions: &mut Vec<GameAction>) {
        let Some((intent, since)) = self.play_leave else {
            return;
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("play_leave")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("The game is on"));
            ui.label(tr("If you leave, the game ends with your resignation."));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("Resign and add to a database…")).clicked() {
                    choice = Some(0);
                }
                if ui.button(tr("Resign and leave")).clicked() {
                    choice = Some(1);
                }
                if crate::widgets::primary(ui, tr("Keep playing")).clicked() {
                    choice = Some(2);
                }
            });
        });
        if choice.is_none() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            choice = Some(2);
        }
        let Some(choice) = choice else { return };
        self.play_leave = None;
        // The clocks waited while the question was open.
        let waited = since.elapsed();
        if let Some(c) = self.play.as_mut().and_then(|s| s.clock.as_mut()) {
            c.pause(waited);
        }
        match choice {
            // Then the game goes on where the user was going once it is saved.
            0 => {
                self.resign_if_playing();
                actions.push(GameAction::SaveAndLeave(intent));
            }
            // Leaving was the answer: no second question about saving.
            1 => {
                self.resign_if_playing();
                self.game.mark_saved();
                actions.push(GameAction::Leave(intent));
            }
            _ => {}
        }
    }

    /// The side panel during a game: the opponent's clock, the notation, the user's
    /// clock, the game's buttons.
    fn play_side(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        analysis: &mut Analysis,
        reference: &mut crate::reference::Reference,
        hub: &mut crate::translate_ui::Hub,
        actions: &mut Vec<GameAction>,
    ) {
        let _ = (settings, analysis, reference, hub);
        let Some(s) = &self.play else { return };
        let user = s.user;
        // The side whose pieces are at the top of the board is at the top.
        let top = if self.flipped {
            Color::White
        } else {
            Color::Black
        };
        egui::Panel::bottom("play_toolbar").show(ui, |ui| self.play_toolbar(ui, actions));
        egui::Panel::bottom("play_card_bottom")
            .show_separator_line(false)
            .show(ui, |ui| self.clock_card(ui, !top, user));
        egui::Panel::top("play_card_top")
            .show_separator_line(false)
            .show(ui, |ui| self.clock_card(ui, top, user));
        if self.cursor != self.play_end_node() {
            egui::Panel::bottom("play_viewing")
                .show_separator_line(false)
                .show(ui, |ui| {
                    if ui
                        .link(
                            RichText::new(tr(
                                "Viewing an earlier position · back to the game — End",
                            ))
                            .weak(),
                        )
                        .clicked()
                    {
                        let end = self.play_end_node();
                        self.goto(end);
                    }
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let resp = Notation {
                        game: &self.game,
                        cursor: self.cursor,
                        scroll_to_cursor: self.scroll_to_cursor,
                        marks: None,
                        preview: None,
                        translations: None,
                        editor: None,
                        read_only: true,
                    }
                    .show(ui);
                    if let Some(id) = resp.clicked {
                        self.goto(id);
                    }
                    self.scroll_to_cursor = false;
                });
        });
    }

    /// A player's card: name, rating, the clock and what is going on.
    fn clock_card(&self, ui: &mut Ui, side: Color, user: Color) {
        let Some(s) = &self.play else { return };
        // While «The game is on» is open, the clocks stand at the moment it opened.
        let now = self
            .play_leave
            .map_or_else(std::time::Instant::now, |(_, since)| since);
        let turn = self.play_turn();
        let active = turn == side;
        let left = s.clock.as_ref().map(|c| c.left(side, now));
        let low = s
            .clock
            .as_ref()
            .zip(left)
            .is_some_and(|(c, l)| c.running() == Some(side) && l <= c.low());
        let sel = ui.visuals().selection;
        let mut frame = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 6))
            .corner_radius(4)
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ));
        if active {
            frame = frame.fill(sel.bg_fill.gamma_multiply(0.35));
        }
        if low {
            frame = frame.stroke(egui::Stroke::new(1.5, ui.visuals().error_fg_color));
        }
        ui.add_space(4.0);
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    let king = if side == Color::White { "♔" } else { "♚" };
                    let name = if side == user {
                        match s.user_elo {
                            Some(r) => format!("{king} {} · {r}", tr("You")),
                            None => format!("{king} {}", tr("You")),
                        }
                    } else {
                        format!("{king} Maia {}", s.strength)
                    };
                    ui.label(RichText::new(name).size(15.0));
                    let state = if side != user && s.opponent.thinking {
                        tr("thinking…")
                    } else if side == user && active {
                        tr("your move")
                    } else if side != user {
                        tr("waiting")
                    } else {
                        ""
                    };
                    ui.label(RichText::new(state).weak().size(13.0));
                });
                if let Some(left) = left {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut t = RichText::new(crate::play::clock_text(left))
                            .monospace()
                            .size(28.0);
                        if low {
                            t = t.color(ui.visuals().error_fg_color);
                        } else if active {
                            t = t.strong();
                        } else {
                            t = t.weak();
                        }
                        ui.label(t);
                    });
                }
            });
        });
        ui.add_space(4.0);
    }

    /// ⏮ ◀ ▶ ⏭ · 🔃 · ↶ Take back · 🏳 Resign (✕ Abort before the first moves).
    fn play_toolbar(&mut self, ui: &mut Ui, actions: &mut Vec<GameAction>) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let big = |s: &str| RichText::new(s).size(18.0);
            // The words as tall as the icon buttons beside them.
            let word =
                |s: &str| egui::Button::new(RichText::new(s).size(14.0)).min_size(vec2(0.0, 28.0));
            if ui.button(big("⏮")).clicked() {
                self.goto(NodeId::ROOT);
            }
            if ui.button(big("◀")).clicked() {
                self.step_back();
            }
            if ui.button(big("▶")).clicked() {
                self.step_forward(actions);
            }
            if ui.button(big("⏭")).clicked() {
                let end = self.play_end_node();
                self.goto(end);
            }
            ui.separator();
            if ui
                .button(big("🔃"))
                .on_hover_text(tr("Flip board (F)"))
                .clicked()
            {
                self.flipped = !self.flipped;
            }
            ui.separator();
            let user = self.play.as_ref().map_or(Color::White, |s| s.user);
            let user_moved = self.game.mainline().iter().any(|&id| {
                self.game
                    .parent(id)
                    .and_then(|p| self.game.position(p))
                    .is_some_and(|p| p.turn() == user)
            });
            if ui
                .add_enabled(user_moved, word(tr("↶ Take back")))
                .on_hover_text(tr("Take back a move (Ctrl+Z)"))
                .clicked()
            {
                self.take_back();
            }
            if !self.play_started() {
                if ui
                    .add(word(tr("× Abort")))
                    .on_hover_text(tr("Leave without a result: no moves made yet"))
                    .clicked()
                {
                    actions.push(GameAction::Back);
                }
            } else if self.play.as_ref().is_some_and(|s| s.resign_asked) {
                ui.label(RichText::new(tr("Resign?")).size(14.0));
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(tr("Yes")).size(14.0).color(Color32::WHITE),
                        )
                        .fill(ui.visuals().error_fg_color)
                        .min_size(vec2(0.0, 28.0)),
                    )
                    .clicked()
                {
                    let user = self.play.as_ref().map_or(Color::White, |s| s.user);
                    self.finish_play(crate::play::Ending::Resigned { loser: user }, actions);
                } else if ui.add(word(tr("No"))).clicked()
                    && let Some(s) = self.play.as_mut()
                {
                    s.resign_asked = false;
                }
            } else if ui.add(word(tr("⚑ Resign"))).clicked()
                && let Some(s) = self.play.as_mut()
            {
                s.resign_asked = true;
            }
        });
        ui.add_space(4.0);
    }

    /// The strip above the notation after a game: the result and what to do next.
    fn result_strip(&mut self, ui: &mut Ui, actions: &mut Vec<GameAction>) {
        let Some(end) = self.play_end.clone() else {
            return;
        };
        let mut close = false;
        ui.add_space(4.0);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 8))
            .corner_radius(4)
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(" {} ", end.ending.result().as_str()))
                            .strong()
                            .size(15.0)
                            .background_color(ui.visuals().widgets.inactive.weak_bg_fill),
                    );
                    ui.label(RichText::new(end.heading()).strong().size(17.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::widgets::small_close_button(ui)
                            .on_hover_text(tr("Close"))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                if !end.detail().is_empty() {
                    ui.label(end.detail());
                }
                if let Some((you, maia)) = end.left {
                    ui.label(
                        RichText::new(trf(
                            "Left: you {you} · Maia {maia}",
                            &[
                                ("you", &crate::play::clock_text(you)),
                                ("maia", &crate::play::clock_text(maia)),
                            ],
                        ))
                        .weak(),
                    );
                }
                ui.add_space(4.0);
                // All four in a row when they fit; else Analyse, then Rematch and New
                // game side by side, then Save — each as wide as the strip.
                let labels = [
                    tr("📈 Analyse"),
                    tr("↻ Rematch"),
                    tr("♞ New game"),
                    tr("💾 Save to a database…"),
                ];
                let font = egui::FontId::proportional(14.0);
                let pad = 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x;
                let one_row: f32 = labels
                    .iter()
                    .map(|l| {
                        ui.painter()
                            .layout_no_wrap((*l).to_owned(), font.clone(), Color32::WHITE)
                            .size()
                            .x
                            + pad
                    })
                    .sum();
                let fits = one_row <= ui.available_width();
                let mut clicked = [false; 4];
                let mut button = |ui: &mut Ui, k: usize, w: f32| {
                    let mut text = RichText::new(labels[k]).size(14.0);
                    let mut b = egui::Button::new(text.clone());
                    if k == 0 {
                        text = text.strong();
                        b = egui::Button::new(text).fill(ui.visuals().selection.bg_fill);
                    }
                    let mut r = ui.add(b.min_size(vec2(w, 30.0)));
                    if k == 1 {
                        r = r.on_hover_text(tr("Same settings, colours swapped"));
                    }
                    if k == 3 {
                        r = r.on_hover_text("Ctrl+S");
                    }
                    clicked[k] = r.clicked();
                };
                if fits {
                    ui.horizontal(|ui| {
                        for k in 0..4 {
                            button(ui, k, 0.0);
                        }
                    });
                } else {
                    let full = ui.available_width();
                    button(ui, 0, full);
                    ui.horizontal(|ui| {
                        let half = (full - ui.spacing().item_spacing.x) / 2.0;
                        button(ui, 1, half);
                        button(ui, 2, half);
                    });
                    button(ui, 3, full);
                }
                if clicked[0] {
                    self.review.dialog_open = true;
                }
                if clicked[1] {
                    actions.push(GameAction::Rematch);
                }
                if clicked[2] {
                    actions.push(GameAction::PlayAgain);
                }
                if clicked[3] {
                    actions.push(GameAction::Save);
                }
            });
        if close {
            self.play_end = None;
        }
    }

    /// Dev hook (`CP_SHAPE=Ge2e4,Rd5`): shapes drawn on the board at the current move.
    pub fn debug_shapes(&mut self, spec: &str) {
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let parsed = chess_core::parse_comment(&if item.len() > 3 {
                format!("[%cal {item}]")
            } else {
                format!("[%csl {item}]")
            });
            for p in parsed {
                match p {
                    CommentPart::Annotation(Annotation::Arrows(list)) => {
                        for (c, a, b) in list {
                            self.apply_shape(board::Drawn::Arrow(c, a, b));
                        }
                    }
                    CommentPart::Annotation(Annotation::Squares(list)) => {
                        for (c, sq) in list {
                            self.apply_shape(board::Drawn::Circle(c, sq));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Dev hook (`CP_COMMENT_EDIT`): the editor on the current move.
    pub fn debug_comment_edit(&mut self) {
        self.open_editor(Slot::After(self.cursor));
    }

    /// Dev hook (`CP_COMMENT`): a comment after the current move.
    pub fn debug_comment(&mut self, text: &str) {
        let nags = self
            .game
            .node(self.cursor)
            .map(|n| n.nags().to_vec())
            .unwrap_or_default();
        let _ = self.game.annotate(self.cursor, nags, vec![text.to_owned()]);
    }

    fn step_back(&mut self) {
        if let Some(p) = &mut self.preview {
            self.scroll_to_cursor = true;
            p.shown = p.shown.saturating_sub(1);
            if p.shown == 0 {
                self.preview = None;
            }
            return;
        }
        if let Some(parent) = self.game.parent(self.cursor) {
            self.goto(parent);
        }
    }

    fn step_forward(&mut self, actions: &mut Vec<GameAction>) {
        if let Some(p) = &mut self.preview {
            if p.shown < p.moves.len() {
                self.scroll_to_cursor = true;
                p.shown += 1;
                actions.push(GameAction::Sound(Sound::Move));
            }
            return;
        }
        if let Some(next) = self.game.main_child(self.cursor) {
            self.goto(next);
            if let Some(s) = self.sound_for(next) {
                actions.push(GameAction::Sound(s));
            }
        }
    }

    fn undo(&mut self) {
        if let Some(o) = self.game.undo() {
            self.goto(o.cursor);
        }
    }

    fn redo(&mut self) {
        if let Some(o) = self.game.redo() {
            self.goto(o.cursor);
        }
    }

    fn keyboard(&mut self, ui: &Ui, actions: &mut Vec<GameAction>) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if self.play.is_some() {
            self.play_keys(ui, actions);
            return;
        }
        let ctrl = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let ctrl_shift = |k| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
        let overlay = crate::app::overlay_open(ui.ctx());
        let mut cmds = Vec::new();
        ui.input_mut(|i| {
            if i.consume_shortcut(&ctrl_shift(Key::Z)) || i.consume_shortcut(&ctrl(Key::Y)) {
                cmds.push("redo");
            }
            if i.consume_shortcut(&ctrl(Key::Z)) {
                cmds.push("undo");
            }
            if i.consume_shortcut(&ctrl_shift(Key::C)) {
                cmds.push("copy");
            }
            if i.consume_shortcut(&ctrl_shift(Key::S)) {
                cmds.push("add_to_db");
            }
            if i.consume_shortcut(&ctrl(Key::S)) {
                cmds.push("save");
            }
            // F2 as in ChessBase; on a Mac F2 is the brightness key, ⌘I too.
            if i.consume_key(Modifiers::NONE, Key::F2)
                || (cfg!(target_os = "macos") && i.consume_shortcut(&ctrl(Key::I)))
            {
                cmds.push("game_data");
            }
            if i.consume_shortcut(&ctrl(Key::W)) {
                cmds.push("back");
            }
            if i.consume_shortcut(&ctrl_shift(Key::F)) {
                cmds.push("find");
            }
            if i.consume_shortcut(&ctrl_shift(Key::T)) {
                cmds.push("translate");
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::A)) {
                cmds.push("review");
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::ArrowUp)) {
                cmds.push("prev_var");
            }
            for (key, cmd) in [
                (Key::Num1, "nag1"),
                (Key::Num2, "nag2"),
                (Key::Num3, "nag3"),
                (Key::Num4, "nag4"),
                (Key::Num5, "nag5"),
                (Key::Num6, "nag6"),
            ] {
                if i.consume_shortcut(&ctrl(key)) {
                    cmds.push(cmd);
                }
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::ArrowDown)) {
                cmds.push("next_var");
            }
            if i.modifiers.is_none() {
                for (key, cmd) in [
                    (Key::ArrowLeft, "back1"),
                    (Key::ArrowRight, "fwd1"),
                    (Key::ArrowUp, "end"),
                    (Key::ArrowDown, "home"),
                    (Key::Home, "home"),
                    (Key::End, "end"),
                    (Key::F, "flip"),
                    (Key::L, "engine"),
                    (Key::Escape, "esc"),
                ] {
                    if i.key_pressed(key) {
                        cmds.push(cmd);
                    }
                }
                // Taken: the Enter that closed a dialog this frame must not open the editor.
                if i.key_pressed(Key::Enter) && !overlay && i.modifiers.is_none() {
                    i.consume_key(Modifiers::NONE, Key::Enter);
                    cmds.push("comment");
                }
            }
        });
        for cmd in cmds {
            match cmd {
                "undo" if self.editor.is_none() => self.undo(),
                "redo" if self.editor.is_none() => self.redo(),
                "comment" => {
                    if self.preview.is_none() {
                        self.open_editor(Slot::After(self.cursor));
                    }
                }
                "nag1" | "nag2" | "nag3" | "nag4" | "nag5" | "nag6" => {
                    if self.preview.is_none() {
                        let k: u8 = cmd[3..].parse().unwrap_or(1);
                        // With the editor open the symbol is part of its draft.
                        match self.editor.as_mut() {
                            Some(ed) if ed.slot == Slot::After(self.cursor) => {
                                ed.nags = toggle_nag(&ed.nags, Nag(k));
                            }
                            _ => self.toggle_nag_on(self.cursor, Nag(k)),
                        }
                    }
                }
                "copy" => actions.push(self.copy_pgn()),
                "save" => actions.push(GameAction::Save),
                "add_to_db" => actions.push(GameAction::AddToDatabase),
                "game_data" => actions.push(GameAction::EditGameData),
                "back" => actions.push(GameAction::Back),
                "find" if self.search_line().is_some() => {
                    actions.push(GameAction::FindPosition(None))
                }
                "back1" => self.step_back(),
                "fwd1" => self.step_forward(actions),
                "prev_var" => {
                    if self.preview.is_none()
                        && let Some(s) = self.game.prev_sibling(self.cursor)
                    {
                        self.goto(s);
                    }
                }
                "next_var" => {
                    if self.preview.is_none()
                        && let Some(s) = self.game.next_sibling(self.cursor)
                    {
                        self.goto(s);
                    }
                }
                "home" => self.goto(NodeId::ROOT),
                "end" => {
                    let end = self.game.line_end(self.cursor);
                    self.goto(end);
                }
                "flip" => self.flipped = !self.flipped,
                "translate" => {
                    if self.translation.has_translation() {
                        self.translation.toggle();
                    } else if self.translation.can_translate() {
                        self.translate_now = true;
                        self.side_tab = SideTab::Notation;
                    }
                }
                "engine" => actions.push(GameAction::ToggleEngine),
                "review" => {
                    if self.review.has_data() || self.review.running() {
                        self.review.visible = !self.review.visible;
                    } else {
                        self.review.dialog_open = true;
                    }
                }
                // Escape cancels what is going on first (a preview of an engine line, a
                // piece picked up); then it leaves the game: to the game this one was
                // opened from, or to the library. A window, menu or dialog open over the
                // game takes the key itself.
                "esc" => {
                    if self.preview.is_some() {
                        self.preview = None;
                    } else if self.board.is_busy() {
                        self.board.clear();
                    } else if !crate::app::overlay_open(ui.ctx()) {
                        // Taken: the question about unsaved edits that leaving may open in
                        // this frame must not read the same key as its own Escape.
                        ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
                        actions.push(if self.previous.is_some() {
                            GameAction::ReturnToPrevious
                        } else {
                            GameAction::Back
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

/// The least widths and heights of the game screen (docs/design/narrow-windows.md §1):
/// the side panel (notation, report), its reference tab, the board, the notation part
/// under a stacked board, the evaluation bar with its gap.
const SIDE_MIN: f32 = 240.0;
const REF_MIN: f32 = 320.0;
const BOARD_MIN: f32 = 220.0;
const STACK_BOTTOM_MIN: f32 = 300.0;
const EVAL_BAR: f32 = 26.0;
/// The engine row alone (the "Analysis" toggle), as counted in `STACK_BOTTOM_MIN`.
const ENGINE_ROW: f32 = 44.0;

const SEARCH_TIMES: [Option<u32>; 11] = [
    Some(2),
    Some(4),
    Some(6),
    Some(8),
    Some(10),
    Some(15),
    Some(20),
    Some(30),
    Some(60),
    Some(90),
    None,
];

/// Lichess-style inline engine settings: engine choice, search time, lines, threads, memory.
fn engine_settings(ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<GameAction>) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(4)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(tr("Engine"));
                let current = settings
                    .active_engine()
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| tr("not selected").into());
                egui::ComboBox::from_id_salt("engine_pick")
                    .selected_text(current)
                    .width(ui.available_width() - 40.0)
                    .show_ui(ui, |ui| {
                        for i in 0..settings.engines.len() {
                            let selected = settings.active_engine == Some(i);
                            if ui
                                .selectable_label(selected, &settings.engines[i].name)
                                .clicked()
                                && !selected
                            {
                                settings.active_engine = Some(i);
                                actions.push(GameAction::EngineSwitch);
                            }
                        }
                        ui.separator();
                        if ui.button(tr("Add or edit engines…")).clicked() {
                            actions.push(GameAction::OpenEngineSettings);
                        }
                    });
            });
            let Some(i) = settings
                .active_engine
                .filter(|i| *i < settings.engines.len())
            else {
                if ui.button(tr("Add engine…")).clicked() {
                    actions.push(GameAction::OpenEngineSettings);
                }
                return;
            };
            let e = &mut settings.engines[i];
            let cores = std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1);
            let hash_steps = hash_steps();
            let mut settled = false;
            // Each setting: its name, the slider, the value. The slider gets what the
            // measured names and values leave; in a narrow panel (under 100 pt for it)
            // the name and the value share a line above a slider across the panel.
            let mut t = SEARCH_TIMES
                .iter()
                .position(|s| *s == e.search_secs)
                .unwrap_or(SEARCH_TIMES.len() - 1);
            let mut k = e.multipv.clamp(1, 5) as usize - 1;
            let mut th = e.threads.clamp(1, cores) as usize - 1;
            let mut h = hash_steps
                .iter()
                .position(|m| *m >= e.hash_mb)
                .unwrap_or(hash_steps.len() - 1);
            let search_value = |t: usize| {
                SEARCH_TIMES[t]
                    .map(|s| trf("{n} s", &[("n", &s)]))
                    .unwrap_or_else(|| "∞".into())
            };
            let names = [tr("Search time"), tr("Lines"), tr("Threads"), tr("Memory")];
            // The widest value each setting can show, so the slider does not jump.
            let widest = [
                (0..SEARCH_TIMES.len())
                    .map(search_value)
                    .collect::<Vec<_>>(),
                vec!["5 / 5".to_owned()],
                vec![format!("{cores} / {cores}")],
                hash_steps.iter().map(|m| format_mb(*m)).collect(),
            ];
            let font = egui::TextStyle::Body.resolve(ui.style());
            let text_w = |t: &str| {
                ui.painter()
                    .layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            };
            let name_w = names.iter().map(|n| text_w(n)).fold(0.0, f32::max);
            let value_w = widest
                .iter()
                .flatten()
                .map(|v| text_w(v))
                .fold(0.0, f32::max);
            let gap = 12.0;
            let slider = (ui.available_width() - name_w - value_w - 2.0 * gap).floor();
            let mut rows: [(&str, &mut usize, usize); 4] = [
                (names[0], &mut t, SEARCH_TIMES.len()),
                (names[1], &mut k, 5),
                (names[2], &mut th, cores as usize),
                (names[3], &mut h, hash_steps.len()),
            ];
            let value = |i: usize, idx: usize| match i {
                0 => search_value(idx),
                1 => format!("{} / 5", idx + 1),
                2 => format!("{} / {cores}", idx + 1),
                _ => format_mb(hash_steps[idx]),
            };
            if slider >= 100.0 {
                ui.spacing_mut().slider_width = slider;
                egui::Grid::new("engine_settings_grid")
                    .num_columns(3)
                    .spacing(vec2(gap, 8.0))
                    .show(ui, |ui| {
                        for (i, (name, idx, len)) in rows.iter_mut().enumerate() {
                            ui.label(*name);
                            settled |= index_slider(ui, idx, *len);
                            ui.label(value(i, **idx));
                            ui.end_row();
                        }
                    });
            } else {
                ui.spacing_mut().slider_width = ui.available_width().floor();
                for (i, (name, idx, len)) in rows.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(*name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(value(i, **idx));
                        });
                    });
                    settled |= index_slider(ui, idx, *len);
                    ui.add_space(4.0);
                }
            }
            e.search_secs = SEARCH_TIMES[t];
            e.multipv = k as u32 + 1;
            e.threads = th as u32 + 1;
            e.hash_mb = hash_steps[h];
            if settled {
                actions.push(GameAction::EngineReconfigure);
            }
        });
}

/// Slider over `0..len`; returns true when the user finished changing it.
pub(crate) fn index_slider(ui: &mut Ui, idx: &mut usize, len: usize) -> bool {
    let before = *idx;
    let r = ui.add(egui::Slider::new(idx, 0..=len.saturating_sub(1)).show_value(false));
    r.drag_stopped() || (*idx != before && !r.dragged())
}

pub(crate) fn format_mb(mb: u32) -> String {
    if mb >= 1024 {
        trf("{n} GB", &[("n", &(mb / 1024))])
    } else {
        trf("{n} MB", &[("n", &mb)])
    }
}

/// Hash sizes from 16 MB up to half of physical memory (powers of two).
pub(crate) fn hash_steps() -> Vec<u32> {
    let total_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(4096);
    let max = (total_mb / 2).clamp(16, 65536) as u32;
    let mut v = vec![16];
    while v.last().is_some_and(|m| m * 2 <= max) {
        let next = v.last().copied().unwrap_or(16) * 2;
        v.push(next);
    }
    v
}

#[derive(Clone, Copy)]
enum GameOver {
    Mated(Color),
    Draw(&'static str),
}

impl GameOver {
    fn label(self) -> &'static str {
        match self {
            GameOver::Mated(Color::White) => tr("Checkmate · 0-1"),
            GameOver::Mated(Color::Black) => tr("Checkmate · 1-0"),
            GameOver::Draw(s) => s,
        }
    }
}

/// Terminal positions: engines send no PV there, so the UI states the outcome itself.
fn game_over(pos: &Chess) -> Option<GameOver> {
    if pos.is_checkmate() {
        Some(GameOver::Mated(pos.turn()))
    } else if pos.is_stalemate() {
        Some(GameOver::Draw(tr("Stalemate · ½-½")))
    } else if pos.is_insufficient_material() {
        Some(GameOver::Draw(tr("Insufficient material")))
    } else {
        None
    }
}

/// Engine arrows (live analysis and game analysis) are all blue; a move both suggest
/// is drawn once, with the stronger of the two looks.
fn engine_arrow(arrows: &mut Vec<Arrow>, (from, to): (Square, Square), alpha: u8, weight: f32) {
    let color = Color32::from_rgba_unmultiplied(0, 48, 136, alpha);
    if let Some(a) = arrows.iter_mut().find(|a| a.from == from && a.to == to) {
        if a.color.a() < color.a() {
            a.color = color;
        }
        a.weight = a.weight.max(weight);
        return;
    }
    arrows.push(Arrow {
        from,
        to,
        color,
        weight,
    });
}

/// "1-0", "0-1", "½-½"; `None` for an unfinished game.
fn result_label(r: GameResult) -> Option<&'static str> {
    match r {
        GameResult::WhiteWins => Some("1-0"),
        GameResult::BlackWins => Some("0-1"),
        GameResult::Draw => Some("½-½"),
        GameResult::Unknown => None,
    }
}

fn result_hint(r: GameResult) -> &'static str {
    match r {
        GameResult::WhiteWins => tr("White won"),
        GameResult::BlackWins => tr("Black won"),
        _ => tr("Draw"),
    }
}

fn move_squares(m: &Move) -> Option<(Square, Square)> {
    Some((m.from()?, board::ui_target(m)))
}

fn shape_color(c: ShapeColor) -> Color32 {
    match c {
        ShapeColor::Red => Color32::from_rgba_unmultiplied(136, 32, 32, 170),
        ShapeColor::Green => Color32::from_rgba_unmultiplied(21, 120, 27, 170),
        ShapeColor::Blue => Color32::from_rgba_unmultiplied(0, 48, 136, 170),
        ShapeColor::Yellow => Color32::from_rgba_unmultiplied(230, 143, 0, 170),
        ShapeColor::Other(_) => Color32::from_rgba_unmultiplied(120, 120, 120, 170),
    }
}

/// The width of `text` in `style` (as a label or a button would lay it out on one line).
pub(crate) fn text_width(ui: &Ui, text: &str, style: egui::TextStyle) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), style.resolve(ui.style()), Color32::WHITE)
        .size()
        .x
}
