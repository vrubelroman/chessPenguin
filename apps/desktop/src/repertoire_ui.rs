use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};

use chess_core::shakmaty::{Position, uci::UciMove};
use chess_core::{Game, NodeId, PgnDatabase, Severity};
use egui::{Key, Modifiers, RichText, Ui};
use trainer::repertoire::{
    LearningAggregate, LearningItemKey, LearningPriority, LearningSessionPhase,
    LearningSessionSnapshot, LearningSide, LearningStore, LearningStoreData, Library, LibraryStore,
    MergeReport, QueueMode, RepertoireId, RepertoireTree, ResetImpactProjection,
    ScopedLearningAggregate, SrsSettings, StatsScope, TrainingMode, TrainingScope,
    active_learning_keys, aggregate_scope, fen4, reset_impact,
};

use crate::board::{BoardState, BoardView};
use crate::i18n::tr;
use crate::learning_dashboard::{LearningDashboardAction, LearningDashboardUi};
use crate::learning_training::{LearningTrainingAction as TrainingAction, LearningTrainingUi};
use crate::repertoire_editor::{RepertoireEditorAction, RepertoireEditorUi};
use crate::settings::Settings;

pub enum RepertoireAction {
    Close,
    OpenSettings,
    Sound(crate::assets::Sound),
    Toast(String),
    SettingsChanged,
    Added {
        id: RepertoireId,
        name: String,
        report: MergeReport,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OpeningLabel {
    eco: String,
    name: String,
}

struct LibraryRow {
    id: RepertoireId,
    name: String,
    openings: Vec<OpeningLabel>,
}

const OPENING_LINES: usize = 5;
const ACTION_BUTTON_WIDTH: f32 = 120.0;
const ROW_VERTICAL_PADDING: f32 = 8.0;
const ROW_BOTTOM_PADDING: f32 = 4.0;
const TITLE_HEIGHT: f32 = 24.0;
const TITLE_GAP: f32 = 12.0;
const OPENING_HEIGHT: f32 = 20.0;
const OPENING_GAP: f32 = 2.0;

impl LibraryRow {
    fn height() -> f32 {
        ROW_VERTICAL_PADDING
            + ROW_BOTTOM_PADDING
            + TITLE_HEIGHT
            + TITLE_GAP
            + OPENING_HEIGHT * OPENING_LINES as f32
            + OPENING_GAP * (OPENING_LINES - 1) as f32
    }

    fn visible_openings(&self) -> &[OpeningLabel] {
        &self.openings[..self.openings.len().min(OPENING_LINES)]
    }
}

struct ImportPreview {
    library: Library,
    id: RepertoireId,
    records: usize,
    report: MergeReport,
    name: String,
    target: Option<RepertoireId>,
}

struct LibraryLoadError {
    message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AggregateKey {
    id: RepertoireId,
    side: LearningSide,
    library_revision: u64,
    learning_revision: u64,
    minute: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartKind {
    Review,
    IntroduceNew,
    Unavailable,
}

struct SharedReset {
    impact: Option<ResetImpactProjection>,
    keys: BTreeSet<LearningItemKey>,
    confirmed: bool,
    busy: bool,
}

enum Event {
    Aggregated(AggregateKey, Box<Result<ScopedLearningAggregate, String>>),
    ResetImpact(
        u64,
        Box<Result<(ResetImpactProjection, BTreeSet<LearningItemKey>), String>>,
    ),
    LearningReset(u64, Box<Result<LearningStoreData, String>>),
    ReviewReset(u64, Box<Result<LearningStoreData, String>>),
    Indexed(
        u64,
        HashMap<RepertoireId, Arc<RepertoireTree>>,
        HashMap<RepertoireId, Vec<OpeningLabel>>,
        HashMap<RepertoireId, usize>,
    ),
    TrainingReady(u64, Result<Box<LearningTrainingUi>, String>),
    Imported(Result<ImportPreview, String>),
    Exported(Result<(), String>),
}

struct AddFromGame {
    source: Game,
    at: NodeId,
    selected: Option<RepertoireId>,
    name: String,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct TestLayout {
    status: Option<egui::Rect>,
    message: Option<egui::Rect>,
    retry: Option<egui::Rect>,
    cancel: Option<egui::Rect>,
    table: Option<egui::Rect>,
    reset_review: Option<egui::Rect>,
    editor: Option<egui::Rect>,
}

pub struct RepertoireUi {
    library_store: LibraryStore,
    learning_store: LearningStore,
    library: Library,
    learning: LearningStoreData,
    library_error: Option<String>,
    learning_error: Option<String>,
    error: Option<String>,
    selected: Option<RepertoireId>,
    scroll_to_selected: bool,
    tree: Option<Arc<RepertoireTree>>,
    trees: HashMap<RepertoireId, Arc<RepertoireTree>>,
    openings: HashMap<RepertoireId, Vec<OpeningLabel>>,
    variant_counts: HashMap<RepertoireId, usize>,
    aggregates: HashMap<AggregateKey, Option<Result<ScopedLearningAggregate, String>>>,
    index_generation: u64,
    indexing: bool,
    preparing: bool,
    training_generation: u64,
    board: BoardState,
    side: LearningSide,
    queue: QueueMode,
    priority: LearningPriority,
    shared_reset: Option<SharedReset>,
    reset_generation: u64,
    review_reset_pending: bool,
    training: Option<LearningTrainingUi>,
    training_visible: bool,
    training_name: String,
    editor: Option<RepertoireEditorUi>,
    editor_id: Option<RepertoireId>,
    dashboard: Option<LearningDashboardUi>,
    return_to_game: Option<String>,
    add: Option<AddFromGame>,
    edit_name: Option<(Option<RepertoireId>, String)>,
    delete: Option<RepertoireId>,
    search: String,
    import: Option<ImportPreview>,
    cancel: Option<Arc<AtomicBool>>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    debug_start: Option<String>,
    debug_pending_moves: Option<String>,
    #[cfg(test)]
    debug_layout: TestLayout,
}

impl RepertoireUi {
    pub fn new() -> Self {
        let (library_store, learning_store) = stores();
        Self::with_stores(library_store, learning_store)
    }
    fn with_stores(library_store: LibraryStore, learning_store: LearningStore) -> Self {
        let (library, library_error) = match load_library(&library_store) {
            Ok(data) => (data, None),
            Err(e) => (Library::default(), Some(e.message)),
        };
        let (learning, learning_error) = match learning_store.load() {
            Ok(data) => (data, None),
            Err(e) => (LearningStoreData::default(), Some(e.to_string())),
        };
        let selected = library.repertoires.first().map(|r| r.id.clone());
        let (tx, rx) = mpsc::channel();
        let mut this = Self {
            library_store,
            learning_store,
            library,
            learning,
            library_error,
            learning_error,
            error: None,
            selected,
            scroll_to_selected: true,
            tree: None,
            trees: HashMap::new(),
            openings: HashMap::new(),
            variant_counts: HashMap::new(),
            aggregates: HashMap::new(),
            index_generation: 0,
            indexing: false,
            preparing: false,
            training_generation: 0,
            board: BoardState::default(),
            side: LearningSide::White,
            queue: QueueMode::SpacedRepetition,
            priority: LearningPriority::default(),
            shared_reset: None,
            reset_generation: 0,
            review_reset_pending: false,
            training: None,
            training_visible: false,
            training_name: String::new(),
            editor: None,
            editor_id: None,
            dashboard: None,
            return_to_game: None,
            add: None,
            edit_name: None,
            delete: None,
            search: String::new(),
            import: None,
            cancel: None,
            tx,
            rx,
            debug_start: None,
            debug_pending_moves: None,
            #[cfg(test)]
            debug_layout: TestLayout::default(),
        };
        this.rebuild_tree();
        this
    }

    pub fn has_unsaved(&self) -> bool {
        self.training
            .as_ref()
            .is_some_and(LearningTrainingUi::has_unsaved)
            || self
                .editor
                .as_ref()
                .is_some_and(RepertoireEditorUi::has_unsaved)
    }
    pub fn resolve_unsaved(&mut self) {
        if self
            .training
            .as_ref()
            .is_some_and(LearningTrainingUi::has_unsaved)
        {
            self.training_visible = true;
        }
    }
    fn open_dashboard(&mut self) {
        if self.has_unsaved() || self.training_visible || self.preparing || self.editor.is_some() {
            return;
        }
        self.dashboard = Some(match self.selected.clone() {
            Some(id) if self.library.repertoire(&id).is_some() => {
                LearningDashboardUi::new_for_repertoire(
                    self.library.clone(),
                    self.learning_store.clone(),
                    id,
                    self.side,
                )
            }
            _ => LearningDashboardUi::new(self.library.clone(), self.learning_store.clone()),
        });
    }
    fn open_editor(&mut self) {
        if self.has_unsaved() || self.training_visible || self.preparing || self.editor.is_some() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        match RepertoireEditorUi::new(&self.library, id.clone()) {
            Ok(mut editor) => {
                editor.set_learning(&self.learning.learning);
                self.editor = Some(editor);
                self.editor_id = Some(id);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    fn close_editor(&mut self) {
        self.editor = None;
        self.editor_id = None;
    }
    fn handle_editor_action(&mut self, action: RepertoireEditorAction) {
        match action {
            RepertoireEditorAction::Close | RepertoireEditorAction::Cancel => self.close_editor(),
            RepertoireEditorAction::Applied {
                repertoire_id,
                library,
            } => {
                let current = self.editor_id.as_ref() == Some(&repertoire_id)
                    && self.selected.as_ref() == Some(&repertoire_id)
                    && self.library.revision.saturating_add(1) == library.revision;
                if current {
                    self.library = library;
                    self.error = None;
                    self.rebuild_tree();
                }
                self.close_editor();
            }
            RepertoireEditorAction::Error(error) => self.error = Some(error),
        }
    }
    pub fn open_library(&mut self) {
        self.preparing = false;
        self.scroll_to_selected = true;
        if self.training.is_some() {
            self.reload_learning();
        }
        self.training_visible = false;
    }
    pub fn open_from_toast(&mut self, id: RepertoireId, title: String, settings: &Settings) {
        self.return_to_game = Some(title);
        self.select(id, settings);
    }
    pub fn open_add_from_game(&mut self, source: Game, at: NodeId, title: String) {
        self.return_to_game = Some(title);
        let start = fen4(source.start().position());
        let selected = self
            .library
            .repertoires
            .iter()
            .find(|r| {
                self.trees
                    .get(&r.id)
                    .is_some_and(|tree| fen4(tree.game().start().position()) == start)
            })
            .map(|r| r.id.clone());
        self.add = Some(AddFromGame {
            source,
            at,
            selected,
            name: String::new(),
        });
    }
    fn select(&mut self, id: RepertoireId, settings: &Settings) {
        if self
            .editor
            .as_ref()
            .is_some_and(RepertoireEditorUi::has_unsaved)
        {
            return;
        }
        self.close_editor();
        self.preparing = false;
        self.training_visible = false;
        self.training = None;
        self.scroll_to_selected = true;
        self.selected = Some(id.clone());
        let (side, queue) = settings
            .repertoire_modes
            .get(id.as_str())
            .map(|(mode, queue)| {
                let side = match mode {
                    TrainingMode::Black => LearningSide::Black,
                    TrainingMode::White | TrainingMode::Both => LearningSide::White,
                };
                (side, *queue)
            })
            .unwrap_or((LearningSide::White, QueueMode::SpacedRepetition));
        self.side = side;
        self.queue = match queue {
            QueueMode::ReviewInOrder | QueueMode::LeastRecent => queue,
            QueueMode::Practice | QueueMode::SpacedRepetition => QueueMode::SpacedRepetition,
        };
        self.priority = settings
            .repertoire_priorities
            .get(id.as_str())
            .copied()
            .unwrap_or_default();
        self.invalidate_reset_jobs();
        self.update_selected_tree();
    }
    fn update_selected_tree(&mut self) {
        self.tree = self
            .selected
            .as_ref()
            .and_then(|id| self.trees.get(id))
            .cloned();
    }
    fn rebuild_tree(&mut self) {
        self.invalidate_reset_jobs();
        self.aggregates.clear();
        self.variant_counts.clear();
        self.index_generation += 1;
        self.indexing = true;
        self.preparing = false;
        self.trees.clear();
        self.openings.clear();
        self.update_selected_tree();
        let generation = self.index_generation;
        let repertoires = self.library.repertoires.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-index", move || {
            let mut trees = HashMap::new();
            let mut openings = HashMap::new();
            let mut variant_counts = HashMap::new();
            for r in repertoires {
                let tree = Arc::new(RepertoireTree::from_game(r.game(), u32::MAX, Vec::new()));
                openings.insert(r.id.clone(), opening_labels(&tree));
                variant_counts.insert(r.id.clone(), pgn_route_count(&tree));
                trees.insert(r.id.clone(), tree);
            }
            let _ = tx.send(Event::Indexed(generation, trees, openings, variant_counts));
        }) {
            self.indexing = false;
            self.error = Some(error.to_string());
        }
    }
    fn persist(&mut self, mut library: Library) -> bool {
        if self.library_error.is_some()
            || self
                .editor
                .as_ref()
                .is_some_and(RepertoireEditorUi::has_unsaved)
        {
            return false;
        }
        match self.library_store.save(&mut library) {
            Ok(()) => {
                self.library = library;
                self.error = None;
                self.close_editor();
                self.trees.clear();
                self.rebuild_tree();
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }
    fn reload_learning(&mut self) {
        match self.learning_store.load() {
            Ok(data) => {
                self.learning = data;
                self.learning_error = None;
                if let Some(editor) = &mut self.editor {
                    editor.set_learning(&self.learning.learning);
                }
            }
            Err(error) => self.learning_error = Some(error.to_string()),
        }
    }
    fn invalidate_reset_jobs(&mut self) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.shared_reset = None;
        self.review_reset_pending = false;
    }
    fn begin_shared_reset(&mut self, scope: StatsScope) {
        if self.has_unsaved() || self.preparing {
            return;
        }
        self.invalidate_reset_jobs();
        let generation = self.reset_generation;
        self.shared_reset = Some(SharedReset {
            impact: None,
            keys: BTreeSet::new(),
            confirmed: false,
            busy: false,
        });
        let library = self.library.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-reset-impact", move || {
            let result = reset_impact(&library, &scope)
                .map_err(|error| error.to_string())
                .and_then(|impact| {
                    active_learning_keys(&library, &scope)
                        .map(|keys| (impact, keys))
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(Event::ResetImpact(generation, Box::new(result)));
        }) {
            self.shared_reset = None;
            self.error = Some(error.to_string());
        }
    }
    fn cancel_shared_reset(&mut self) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.shared_reset = None;
    }
    fn confirm_shared_reset(&mut self) {
        if self.has_unsaved() || self.preparing {
            return;
        }
        let Some(state) = &mut self.shared_reset else {
            return;
        };
        if !state.confirmed || state.busy || state.impact.is_none() || state.keys.is_empty() {
            return;
        }
        state.busy = true;
        let generation = self.reset_generation;
        let keys = state.keys.clone();
        let store = self.learning_store.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-shared-reset", move || {
            let result = store
                .reset_learning(keys)
                .map_err(|error| error.to_string())
                .and_then(|_| store.load().map_err(|error| error.to_string()));
            let _ = tx.send(Event::LearningReset(generation, Box::new(result)));
        }) {
            if let Some(state) = &mut self.shared_reset {
                state.busy = false;
            }
            self.error = Some(error.to_string());
        }
    }
    fn matches_review_snapshot(&self, snapshot: &LearningSessionSnapshot) -> bool {
        self.selected.as_ref().is_some_and(|id| {
            snapshot.session.mode == self.queue
                && snapshot.stats_scope.repertoire_ids.len() == 1
                && snapshot.stats_scope.repertoire_ids.contains(id)
                && snapshot.stats_scope.side == self.side
                && snapshot.stats_scope.max_depth
                    == self
                        .library
                        .repertoire(id)
                        .map(|repertoire| repertoire.max_plies)
                && snapshot.stats_scope.only_active
                && snapshot.stats_scope.branch_root.is_none()
                && snapshot.stats_scope.learning_priority == self.priority
        })
    }
    fn has_review_snapshot(&self) -> bool {
        self.learning
            .session_snapshots
            .iter()
            .any(|snapshot| self.matches_review_snapshot(snapshot))
    }
    fn has_unfinished_review_snapshot(&self) -> bool {
        self.learning.session_snapshots.iter().any(|snapshot| {
            snapshot.phase != LearningSessionPhase::Finished
                && self.matches_review_snapshot(snapshot)
        })
    }
    fn selected_start_kind(&self, summary: &LearningAggregate) -> StartKind {
        if self.queue == QueueMode::SpacedRepetition && self.has_unfinished_review_snapshot() {
            StartKind::Review
        } else {
            mode_start_kind(self.queue, summary)
        }
    }
    fn review_snapshot(&self) -> Option<LearningSessionSnapshot> {
        self.learning
            .session_snapshots
            .iter()
            .filter(|snapshot| self.matches_review_snapshot(snapshot))
            .max_by_key(|snapshot| snapshot.revision)
            .cloned()
    }
    fn reset_review_cursor(&mut self) {
        if self.queue == QueueMode::SpacedRepetition
            || self.has_unsaved()
            || self.preparing
            || self.review_reset_pending
        {
            return;
        }
        let Some(mut snapshot) = self.review_snapshot() else {
            return;
        };
        self.invalidate_reset_jobs();
        let generation = self.reset_generation;
        self.review_reset_pending = true;
        let store = self.learning_store.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-review-reset", move || {
            let result = store
                .reset_review_order(&mut snapshot)
                .map_err(|error| error.to_string())
                .and_then(|_| store.load().map_err(|error| error.to_string()));
            let _ = tx.send(Event::ReviewReset(generation, Box::new(result)));
        }) {
            self.review_reset_pending = false;
            self.error = Some(error.to_string());
        }
    }
    fn checked_name(&self, name: &str, excluding: Option<&RepertoireId>) -> Result<String, String> {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            return Err(tr("The repertoire name must not be empty").to_owned());
        }
        if name.chars().count() > 120 {
            return Err(tr("The repertoire name is longer than 120 characters").to_owned());
        }
        if self.library.repertoires.iter().any(|r| {
            Some(&r.id) != excluding
                && r.name
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase()
                    == name.to_lowercase()
        }) {
            return Err(tr("A repertoire with this name already exists").to_owned());
        }
        Ok(name)
    }
    fn start(&mut self, settings: &mut Settings) {
        if self.learning_error.is_some()
            || self.library_error.is_some()
            || self.has_unsaved()
            || self.preparing
        {
            return;
        }
        let Some((id, name)) = self
            .selected
            .as_ref()
            .and_then(|id| self.library.repertoire(id))
            .map(|r| (r.id.clone(), r.name.clone()))
        else {
            return;
        };
        let Some((scope, summary)) = self
            .selected_aggregate()
            .map(|scoped| (scoped.scope.clone(), scoped.summary.clone()))
        else {
            return;
        };
        let kind = self.selected_start_kind(&summary);
        if kind == StartKind::Unavailable {
            return;
        }
        let introduce_new =
            self.queue == QueueMode::SpacedRepetition && kind == StartKind::IntroduceNew;
        let scope = if self.queue == QueueMode::SpacedRepetition {
            scope
        } else {
            scope.with_learning_priority(self.priority)
        };
        let queue = self.queue;
        let mode = training_mode(self.side);
        let context_policy = settings
            .repertoire_context_policies
            .get(id.as_str())
            .copied()
            .unwrap_or_default();
        let library = self.library.clone();
        let store = self.learning_store.clone();
        let tx = self.tx.clone();
        self.training_generation += 1;
        let generation = self.training_generation;
        settings
            .repertoire_modes
            .insert(id.to_string(), (mode, queue));
        settings
            .repertoire_priorities
            .insert(id.to_string(), self.priority);
        self.close_editor();
        self.invalidate_reset_jobs();
        self.training_name = name;
        self.scroll_to_selected = true;
        self.preparing = true;
        if let Err(error) = crate::platform::spawn("repertoire-training-start", move || {
            let result = LearningTrainingUi::new_with_policy(
                library,
                store,
                scope,
                SrsSettings::default(),
                introduce_new,
                false,
                queue,
                context_policy,
            )
            .map(Box::new)
            .map_err(|e| e.to_string());
            let _ = tx.send(Event::TrainingReady(generation, result));
        }) {
            self.preparing = false;
            self.error = Some(error.to_string());
        }
    }
    fn import_path(
        &mut self,
        path: PathBuf,
        target: Option<RepertoireId>,
        ctx: Option<egui::Context>,
    ) {
        if self.cancel.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let tx = self.tx.clone();
        let base = self.library.clone();
        let result = crate::platform::spawn("repertoire-import", move || {
            let result = read_import(&path, target, base, &cancel);
            let _ = tx.send(Event::Imported(result));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        if let Err(e) = result {
            self.cancel = None;
            self.error = Some(e.to_string());
        }
    }
    fn pick_import(&mut self, target: Option<RepertoireId>, ctx: &egui::Context) {
        if self.cancel.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let base = self.library.clone();
        if let Err(e) = crate::platform::spawn("repertoire-import-dialog", move || {
            let result = rfd::FileDialog::new()
                .add_filter("PGN", &["pgn", "PGN"])
                .pick_file()
                .ok_or_else(|| tr("Cancelled").to_owned())
                .and_then(|path| read_import(&path, target, base, &cancel));
            let _ = tx.send(Event::Imported(result));
            ctx.request_repaint();
        }) {
            self.cancel = None;
            self.error = Some(e.to_string());
        }
    }
    fn confirm_import(&mut self, settings: &Settings) -> bool {
        let Some(preview) = self.import.take() else {
            return false;
        };
        let result = self.merge_import(&preview);
        match result {
            Ok((library, id)) if self.persist(library.clone()) => {
                self.select(id, settings);
                true
            }
            Ok(_) => {
                self.import = Some(preview);
                false
            }
            Err(e) => {
                self.error = Some(e);
                self.import = Some(preview);
                false
            }
        }
    }
    fn merge_import(&self, preview: &ImportPreview) -> Result<(Library, RepertoireId), String> {
        if preview.library.revision != self.library.revision {
            return Err(tr("The library changed; import the PGN again").to_owned());
        }
        let mut library = preview.library.clone();
        if preview.target.is_none() {
            let name = self.checked_name(&preview.name, None)?;
            library
                .rename(&preview.id, name)
                .map_err(|e| e.to_string())?;
        }
        Ok((library, preview.id.clone()))
    }
    fn export(&mut self, ctx: Option<egui::Context>) {
        let Some(r) = self
            .selected
            .as_ref()
            .and_then(|id| self.library.repertoire(id))
        else {
            return;
        };
        let pgn = r.pgn.clone();
        let name = format!("{}.pgn", r.name.replace(['/', '\\'], "_"));
        let tx = self.tx.clone();
        if let Err(e) = crate::platform::spawn("repertoire-export", move || {
            let result = if let Some(path) = std::env::var_os("CP_REPERTOIRE_EXPORT_FILE")
                .map(PathBuf::from)
                .or_else(|| {
                    rfd::FileDialog::new()
                        .set_file_name(name)
                        .add_filter("PGN", &["pgn"])
                        .save_file()
                }) {
                chess_core::pgn::store::write_atomic(&path, pgn.as_bytes())
                    .map_err(|e| e.to_string())
            } else {
                Ok(())
            };
            let _ = tx.send(Event::Exported(result));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        }) {
            self.error = Some(e.to_string());
        }
    }
    fn poll(&mut self, settings: &mut Settings) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Aggregated(key, result) => {
                    if self.current_aggregate_key().as_ref() == Some(&key)
                        && let Err(error) = &*result
                    {
                        self.error = Some(error.clone());
                    }
                    if let Some(value) = self.aggregates.get_mut(&key) {
                        *value = Some(*result);
                    }
                }
                Event::ResetImpact(generation, result) => {
                    if generation != self.reset_generation {
                        continue;
                    }
                    match *result {
                        Ok((impact, keys)) => {
                            if let Some(state) = &mut self.shared_reset {
                                state.impact = Some(impact);
                                state.keys = keys;
                            }
                        }
                        Err(error) => {
                            self.shared_reset = None;
                            self.error = Some(error);
                        }
                    }
                }
                Event::LearningReset(generation, result) => {
                    if generation != self.reset_generation {
                        continue;
                    }
                    match *result {
                        Ok(data) => {
                            self.learning = data;
                            self.learning_error = None;
                            self.shared_reset = None;
                            self.aggregates.clear();
                            self.training = None;
                            self.training_visible = false;
                            self.reset_generation = self.reset_generation.wrapping_add(1);
                        }
                        Err(error) => {
                            if let Some(state) = &mut self.shared_reset {
                                state.busy = false;
                            }
                            self.error = Some(error);
                        }
                    }
                }
                Event::ReviewReset(generation, result) => {
                    if generation != self.reset_generation {
                        continue;
                    }
                    self.review_reset_pending = false;
                    match *result {
                        Ok(data) => {
                            self.learning = data;
                            self.learning_error = None;
                            self.aggregates.clear();
                            self.training = None;
                            self.training_visible = false;
                            self.reset_generation = self.reset_generation.wrapping_add(1);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Event::Indexed(generation, trees, openings, variant_counts) => {
                    if generation == self.index_generation {
                        self.trees = trees;
                        self.openings = openings;
                        self.variant_counts = variant_counts;
                        self.indexing = false;
                        self.update_selected_tree();
                    }
                }
                Event::TrainingReady(generation, result) => {
                    if generation != self.training_generation || !self.preparing {
                        continue;
                    }
                    self.preparing = false;
                    match result {
                        Ok(training) if training.target_available() => {
                            self.training = Some(*training);
                            self.training_visible = true;
                            if let Some(moves) = self.debug_pending_moves.take() {
                                self.debug_moves(&moves);
                            }
                        }
                        Ok(_) => self.error = Some(tr("No targets in this mode").to_owned()),
                        Err(error) => self.error = Some(error),
                    }
                }
                Event::Imported(result) => {
                    let cancelled = self
                        .cancel
                        .take()
                        .is_some_and(|c| c.load(Ordering::Relaxed));
                    if cancelled {
                        continue;
                    }
                    match result {
                        Ok(preview) => self.import = Some(preview),
                        Err(e) => self.error = Some(e),
                    }
                }
                Event::Exported(Err(e)) => self.error = Some(e),
                Event::Exported(Ok(())) => {}
            }
        }
        if self.import.is_some() && std::env::var_os("CP_REPERTOIRE_CONFIRM_IMPORT").is_some() {
            self.confirm_import(settings);
        }
        if self.cancel.is_none()
            && self.import.is_none()
            && let Some(what) = self.debug_start.take()
        {
            self.debug_open(&what, settings);
        }
    }
    pub fn show(&mut self, ui: &mut Ui, settings: &mut Settings) -> Vec<RepertoireAction> {
        self.poll(settings);
        let mut actions = Vec::new();
        let available = ui.available_rect_before_wrap();
        let background = ui.visuals().window_fill();
        ui.painter_at(available)
            .rect_filled(available, 0.0, background);
        #[cfg(test)]
        {
            self.debug_layout.editor = None;
        }
        if self.training_visible && self.training.is_some() {
            let mut training = self.training.take().expect("checked training session");
            let name = self.training_name.clone();
            let emitted = training.show(ui, settings, &name);
            if self.training_visible {
                self.training = Some(training);
            }
            for action in emitted {
                match action {
                    TrainingAction::Back => {
                        self.reload_learning();
                        self.training_visible = false;
                        self.scroll_to_selected = true;
                    }
                    TrainingAction::Sound(sound) => actions.push(RepertoireAction::Sound(sound)),
                    TrainingAction::Toast(text) => actions.push(RepertoireAction::Toast(text)),
                }
            }
            return actions;
        }
        if self.editor.is_some() {
            self.errors(ui);
            #[cfg(test)]
            {
                self.debug_layout.editor = Some(ui.available_rect_before_wrap());
            }
            let mut editor = self.editor.take().expect("checked repertoire editor");
            let emitted = editor.show(ui, settings, &self.library_store);
            for action in emitted {
                self.handle_editor_action(action);
            }
            if self.editor_id.is_some() {
                self.editor = Some(editor);
            }
            return actions;
        }
        if self.dashboard.is_some() {
            let mut dashboard = self.dashboard.take().expect("checked learning dashboard");
            let close = dashboard
                .show(ui, settings)
                .contains(&LearningDashboardAction::Close);
            if close {
                self.reload_learning();
            } else {
                self.dashboard = Some(dashboard);
            }
            return actions;
        }
        if !ui.ctx().egui_wants_keyboard_input()
            && !crate::app::overlay_open(ui.ctx())
            && self.add.is_none()
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            actions.push(RepertoireAction::Close);
        }
        egui::Panel::top("repertoire_header").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(if self.return_to_game.is_some() {
                        tr("← To game")
                    } else {
                        tr("← Databases")
                    })
                    .clicked()
                {
                    actions.push(RepertoireAction::Close);
                }
                ui.heading(tr("Opening training"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text(tr("Settings")).clicked() {
                        actions.push(RepertoireAction::OpenSettings);
                    }
                });
            });
        });
        self.show_workspace(ui, settings, &mut actions);
        self.show_add(ui, &mut actions);
        actions
    }
    fn errors(&mut self, ui: &mut Ui) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
        let mut status = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut status;
        ui.set_clip_rect(rect);
        #[cfg(test)]
        {
            self.debug_layout.status = Some(rect);
            self.debug_layout.message = None;
            self.debug_layout.retry = None;
            self.debug_layout.cancel = None;
        }
        let error = self
            .library_error
            .clone()
            .or_else(|| self.learning_error.clone())
            .or_else(|| self.error.clone());
        let background = if self.indexing {
            Some(tr("Indexing variations and ECO names…"))
        } else if self.preparing {
            Some(tr("Preparing the training session…"))
        } else if self.cancel.is_some() {
            Some(tr("Importing PGN…"))
        } else {
            None
        };
        if background.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut retry = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if error.is_some() {
                let response = ui.button(tr("Retry"));
                retry = response.clicked();
                #[cfg(test)]
                {
                    self.debug_layout.retry = Some(response.rect);
                }
            }
            if self.cancel.is_some() {
                let response = ui.button(tr("Cancel"));
                if response.clicked()
                    && let Some(cancel) = &self.cancel
                {
                    cancel.store(true, Ordering::Relaxed);
                }
                #[cfg(test)]
                {
                    self.debug_layout.cancel = Some(response.rect);
                }
            }
            if background.is_some() {
                ui.add(egui::Spinner::new().size(16.0));
            }
            if let Some(text) = error.as_deref().or(background) {
                let hover = text.to_owned();
                let text = if error.is_some() {
                    RichText::new(text).color(ui.visuals().error_fg_color)
                } else {
                    RichText::new(text)
                };
                let response = ui.add(egui::Label::new(text).truncate().halign(egui::Align::Min));
                #[cfg(test)]
                {
                    self.debug_layout.message = Some(response.rect);
                }
                response.on_hover_text(hover);
            }
        });
        if retry {
            match load_library(&self.library_store) {
                Ok(data) => {
                    self.library = data;
                    self.library_error = None;
                    self.rebuild_tree();
                }
                Err(e) => self.library_error = Some(e.message),
            }
            self.reload_learning();
            self.error = None;
        }
    }
    fn show_library(&mut self, ui: &mut Ui, settings: &Settings) {
        self.errors(ui);
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("+ Create")).clicked() {
                self.edit_name = Some((None, String::new()));
            }
            if ui
                .add_enabled(
                    self.cancel.is_none() && self.library_error.is_none(),
                    egui::Button::new(tr("Import PGN…")),
                )
                .clicked()
            {
                self.pick_import(None, ui.ctx());
            }
        });
        self.name_editor(ui, settings);
        if self.import.is_some() {
            self.import_preview(ui, settings);
        }
        if self.library.repertoires.is_empty() {
            ui.heading(tr("No repertoires yet"));
            ui.label(tr("Create an empty one or import PGN. You can also select a move in any game and choose “Add to repertoire…”."));
        }
        if self.library.repertoires.len() > 8 {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(tr("Search")));
        }
        if let Some(id) = self.delete.clone() {
            ui.separator();
            ui.label(tr(
                "Delete repertoire? Source PGN files and learning history will not change.",
            ));
            ui.horizontal(|ui| {
                if ui.button(tr("Cancel")).clicked() {
                    self.delete = None;
                }
                if ui.button(tr("Delete")).clicked() {
                    let mut library = self.library.clone();
                    if library.delete(&id).is_ok() && self.persist(library) {
                        self.delete = None;
                        self.selected = None;
                        self.update_selected_tree();
                        if let Some(next) = self.library.repertoires.first().map(|r| r.id.clone()) {
                            self.select(next, settings);
                        } else {
                            self.training_visible = false;
                            self.training = None;
                        }
                    }
                }
            });
        }

        let mut rows: Vec<_> = self
            .library
            .repertoires
            .iter()
            .filter(|r| r.name.to_lowercase().contains(&self.search.to_lowercase()))
            .map(|r| LibraryRow {
                id: r.id.clone(),
                name: r.name.clone(),
                openings: self.openings.get(&r.id).cloned().unwrap_or_default(),
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        let scroll_row = if self.scroll_to_selected {
            rows.iter()
                .position(|r| Some(&r.id) == self.selected.as_ref())
        } else {
            None
        };
        let mut clicked = None;
        let mut import = None;
        let mut export = None;
        ui.add_space(8.0);
        let compact = ui.available_width() < 480.0;
        ui.style_mut().interaction.selectable_labels = false;
        #[cfg(test)]
        let table_top = ui.next_widget_position();
        #[cfg(test)]
        let table_width = ui.available_width();
        let mut table = egui_extras::TableBuilder::new(ui)
            .id_salt("repertoire_table")
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::remainder().at_least(120.0).clip(true))
            .column(egui_extras::Column::exact(52.0))
            .column(egui_extras::Column::exact(if compact {
                60.0
            } else {
                70.0
            }))
            .column(egui_extras::Column::exact(28.0));
        if let Some(index) = scroll_row {
            table = table.scroll_to_row(index, Some(egui::Align::Center));
            self.scroll_to_selected = false;
        }
        table
            .header(26.0, |mut header| {
                for (column, key) in ["Name", "ECO", "Variants", ""].into_iter().enumerate() {
                    header.col(|ui| {
                        if column == 2 {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(RichText::new(tr(key)).strong())
                                            .truncate(),
                                    );
                                },
                            );
                        } else {
                            ui.add(egui::Label::new(RichText::new(tr(key)).strong()).truncate());
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(LibraryRow::height(), rows.len(), |mut row| {
                    let r = &rows[row.index()];
                    row.set_selected(self.selected.as_ref() == Some(&r.id));
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = OPENING_GAP;
                            ui.add_space(ROW_VERTICAL_PADDING);
                            table_text_line(
                                ui,
                                RichText::new(&r.name).strong().size(16.0),
                                TITLE_HEIGHT,
                            )
                            .on_hover_text(&r.name);
                            ui.add_space(TITLE_GAP - OPENING_GAP);
                            for (index, opening) in r.visible_openings().iter().enumerate() {
                                let more =
                                    index + 1 == OPENING_LINES && r.openings.len() > OPENING_LINES;
                                let text = if more {
                                    format!("{} …", opening.name)
                                } else {
                                    opening.name.clone()
                                };
                                let response = table_text_line(
                                    ui,
                                    RichText::new(text).size(14.0),
                                    OPENING_HEIGHT,
                                );
                                if more {
                                    response.on_hover_text(
                                        r.openings
                                            .iter()
                                            .map(|opening| {
                                                format!("{} {}", opening.eco, opening.name)
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n"),
                                    );
                                } else {
                                    response.on_hover_text(&opening.name);
                                }
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = OPENING_GAP;
                            ui.add_space(ROW_VERTICAL_PADDING);
                            table_text_line(
                                ui,
                                RichText::new(" ").strong().size(16.0),
                                TITLE_HEIGHT,
                            );
                            ui.add_space(TITLE_GAP - OPENING_GAP);
                            if r.openings.is_empty() {
                                table_text_line(
                                    ui,
                                    RichText::new("—").size(14.0).weak(),
                                    OPENING_HEIGHT,
                                );
                            } else {
                                for (index, opening) in r.visible_openings().iter().enumerate() {
                                    let more = index + 1 == OPENING_LINES
                                        && r.openings.len() > OPENING_LINES;
                                    let text = if more {
                                        format!("{} …", opening.eco)
                                    } else {
                                        opening.eco.clone()
                                    };
                                    table_text_line(
                                        ui,
                                        RichText::new(text).size(14.0),
                                        OPENING_HEIGHT,
                                    );
                                }
                            }
                        });
                    });
                    row.col(|ui| {
                        count_cell(ui, self.variant_counts.get(&r.id).copied());
                    });
                    row.col(|ui| {
                        ui.menu_button("⋯", |ui| {
                            if ui.button(tr("Rename")).clicked() {
                                self.edit_name = Some((Some(r.id.clone()), r.name.clone()));
                                ui.close();
                            }
                            if ui.button(tr("Import PGN into this repertoire…")).clicked() {
                                import = Some(r.id.clone());
                                ui.close();
                            }
                            if ui.button(tr("Export PGN…")).clicked() {
                                export = Some(r.id.clone());
                                ui.close();
                            }
                            if ui.button(tr("Delete…")).clicked() {
                                self.delete = Some(r.id.clone());
                                ui.close();
                            }
                        });
                    });
                    if row.response().clicked() {
                        clicked = Some(r.id.clone());
                    }
                });
            });
        #[cfg(test)]
        {
            let table_bottom = ui.next_widget_position();
            self.debug_layout.table = Some(egui::Rect::from_min_max(
                table_top,
                egui::pos2(table_top.x + table_width, table_bottom.y),
            ));
        }
        if let Some(id) = clicked {
            self.select(id, settings);
        }
        if let Some(id) = import {
            self.pick_import(Some(id), ui.ctx());
        }
        if let Some(id) = export {
            self.select(id, settings);
            self.export(Some(ui.ctx().clone()));
        }
    }
    fn name_editor(&mut self, ui: &mut Ui, settings: &Settings) {
        let Some((id, mut name)) = self.edit_name.clone() else {
            return;
        };
        let mut save = false;
        let mut cancel = false;
        ui.horizontal_wrapped(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .char_limit(120)
                    .hint_text(tr("Name")),
            );
            save = ui.button(tr("Save")).clicked()
                || response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            cancel = ui.button(tr("Cancel")).clicked();
        });
        self.edit_name = Some((id.clone(), name.clone()));
        if cancel {
            self.edit_name = None;
        }
        if save {
            self.save_name(id, &name, settings);
        }
    }
    fn save_name(&mut self, id: Option<RepertoireId>, name: &str, settings: &Settings) {
        match self.checked_name(name, id.as_ref()) {
            Ok(name) => {
                let mut library = self.library.clone();
                let result = if let Some(id) = id {
                    library.rename(&id, name).map(|()| id)
                } else {
                    library.create_empty(name)
                };
                match result {
                    Ok(id) if self.persist(library.clone()) => {
                        self.edit_name = None;
                        self.select(id, settings);
                    }
                    Ok(_) => {}
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn import_preview(&mut self, ui: &mut Ui, settings: &Settings) {
        let Some(preview) = self.import.as_mut() else {
            return;
        };
        ui.separator();
        ui.strong(tr("Import PGN…"));
        if preview.target.is_none() {
            ui.add(
                egui::TextEdit::singleline(&mut preview.name)
                    .char_limit(120)
                    .hint_text(tr("Name")),
            );
        }
        ui.label(format!(
            "{}: {} · {}: {}",
            tr("Games"),
            preview.records,
            tr("New moves"),
            preview.report.added_nodes
        ));
        ui.horizontal(|ui| {
            if ui.button(tr("Cancel")).clicked() {
                self.import = None;
            }
            if ui.button(tr("Add")).clicked() {
                self.confirm_import(settings);
            }
        });
    }
    fn show_workspace(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        let width = ui.available_width();
        let height = ui.available_height();
        let stacked = width < 900.0 && height > width;
        if stacked {
            let available = ui.available_rect_before_wrap();
            let panel_height = (height * 0.52).clamp(340.0, 520.0);
            let board_rect = egui::Rect::from_min_max(
                available.min,
                egui::pos2(available.right(), available.bottom() - panel_height),
            );
            let side_rect = egui::Rect::from_min_max(
                egui::pos2(available.left(), board_rect.bottom()),
                available.max,
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(board_rect), |ui| {
                egui::CentralPanel::default().show(ui, |ui| self.preview_board(ui, settings));
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(side_rect), |ui| {
                ui.set_clip_rect(side_rect);
                egui::Frame::side_top_panel(ui.style())
                    .show(ui, |ui| self.workspace_panel(ui, settings, actions));
            });
        } else {
            let width = ui.available_width();
            egui::Panel::right("repertoire_workspace_side")
                .resizable(true)
                .default_size((width * 0.42).clamp(420.0, 560.0))
                .size_range(360.0..=(width - 220.0).max(360.0))
                .show(ui, |ui| self.workspace_panel(ui, settings, actions));
            egui::CentralPanel::default().show(ui, |ui| self.preview_board(ui, settings));
        }
    }
    fn workspace_panel(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        ui.set_width(ui.available_width());
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| self.library.repertoire(id).cloned())
            .map(|r| (r.id.clone(), r));
        let selected_height = (ui.available_height() * 0.48).clamp(180.0, 520.0);
        egui::ScrollArea::vertical()
            .id_salt("repertoire_selected_scroll")
            .max_height(selected_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if let Some((id, r)) = selected {
                    egui::Frame::new()
                        .fill(ui.visuals().faint_bg_color)
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.add_enabled_ui(!self.preparing, |ui| {
                                let previous_side = self.side;
                                let previous_queue = self.queue;
                                let previous_priority = self.priority;
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::Label::new(RichText::new(&r.name).heading())
                                            .truncate(),
                                    )
                                    .on_hover_text(&r.name);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add_enabled(
                                                    !self.has_unsaved(),
                                                    egui::Button::new(tr("Edit")),
                                                )
                                                .clicked()
                                            {
                                                self.open_editor();
                                            }
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                egui::Grid::new("repertoire_selected_controls")
                                    .num_columns(2)
                                    .spacing([12.0, 6.0])
                                    .show(ui, |ui| {
                                        ui.label(tr("Color"));
                                        ui.horizontal(|ui| {
                                            for side in [LearningSide::White, LearningSide::Black] {
                                                ui.selectable_value(
                                                    &mut self.side,
                                                    side,
                                                    learning_side_label(side),
                                                );
                                            }
                                        });
                                        ui.end_row();
                                        ui.label(tr("Mode"));
                                        egui::ComboBox::from_id_salt("repertoire_learning_mode")
                                            .selected_text(queue_mode_label(self.queue))
                                            .width(220.0)
                                            .truncate()
                                            .show_ui(ui, |ui| {
                                                for queue in [
                                                    QueueMode::SpacedRepetition,
                                                    QueueMode::ReviewInOrder,
                                                    QueueMode::LeastRecent,
                                                ] {
                                                    ui.selectable_value(
                                                        &mut self.queue,
                                                        queue,
                                                        queue_mode_label(queue),
                                                    );
                                                }
                                            });
                                        ui.end_row();
                                        if self.queue != QueueMode::SpacedRepetition {
                                            ui.label(tr("Learning priority"));
                                            egui::ComboBox::from_id_salt(
                                                "repertoire_learning_priority",
                                            )
                                            .selected_text(learning_priority_label(self.priority))
                                            .width(220.0)
                                            .truncate()
                                            .show_ui(
                                                ui,
                                                |ui| {
                                                    for priority in [
                                                        LearningPriority::DepthFirst,
                                                        LearningPriority::BreadthOverDepth,
                                                        LearningPriority::MainLinesFirst,
                                                        LearningPriority::CommonPositionsFirst,
                                                    ] {
                                                        ui.selectable_value(
                                                            &mut self.priority,
                                                            priority,
                                                            learning_priority_label(priority),
                                                        );
                                                    }
                                                },
                                            );
                                            ui.end_row();
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label(tr("Variants"));
                                            if let Some(count) = self.variant_counts.get(&id) {
                                                ui.label(count.to_string());
                                            } else {
                                                ui.add(egui::Spinner::new().size(16.0));
                                                ui.ctx().request_repaint_after(
                                                    std::time::Duration::from_millis(100),
                                                );
                                            }
                                        });
                                        let aggregate = self
                                            .selected_aggregate()
                                            .map(|scoped| scoped.summary.clone());
                                        let resume = self.queue == QueueMode::SpacedRepetition
                                            && self.has_unfinished_review_snapshot();
                                        let kind = aggregate
                                            .as_ref()
                                            .map_or(StartKind::Unavailable, |summary| {
                                                self.selected_start_kind(summary)
                                            });
                                        let ready = kind != StartKind::Unavailable
                                            && !self.preparing
                                            && self.learning_error.is_none()
                                            && self.library_error.is_none();
                                        let cta = match (self.queue, kind) {
                                            (QueueMode::SpacedRepetition, StartKind::Review)
                                                if resume =>
                                            {
                                                tr("Continue").to_owned()
                                            }
                                            (QueueMode::SpacedRepetition, StartKind::Review) => {
                                                format!(
                                                    "{} · {}",
                                                    tr("Start training"),
                                                    aggregate
                                                        .as_ref()
                                                        .map_or(0, |summary| summary.due)
                                                )
                                            }
                                            (
                                                QueueMode::SpacedRepetition,
                                                StartKind::IntroduceNew,
                                            ) => tr("Introduce new moves").to_owned(),
                                            _ => tr("Start training").to_owned(),
                                        };
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add_enabled_ui(ready, |ui| {
                                                    if crate::widgets::primary_wide(
                                                        ui,
                                                        &cta,
                                                        ACTION_BUTTON_WIDTH,
                                                    )
                                                    .clicked()
                                                    {
                                                        self.start(settings);
                                                        if self.preparing {
                                                            actions.push(
                                                                RepertoireAction::SettingsChanged,
                                                            );
                                                        }
                                                    }
                                                });
                                            },
                                        );
                                        ui.end_row();
                                    });
                                if self.queue == QueueMode::ReviewInOrder {
                                    ui.horizontal(|ui| {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                #[cfg(test)]
                                                {
                                                    self.debug_layout.reset_review =
                                                        Some(ui.max_rect());
                                                }
                                                let enabled = self.has_review_snapshot()
                                                    && !self.review_reset_pending
                                                    && !self.has_unsaved();
                                                if ui
                                                    .add_enabled(
                                                        enabled,
                                                        egui::Button::new(tr("Reset Review Order"))
                                                            .min_size(egui::vec2(
                                                                ACTION_BUTTON_WIDTH,
                                                                26.0,
                                                            )),
                                                    )
                                                    .clicked()
                                                {
                                                    self.reset_review_cursor();
                                                }
                                                if self.review_reset_pending {
                                                    ui.add(egui::Spinner::new().size(16.0));
                                                    ui.ctx().request_repaint_after(
                                                        std::time::Duration::from_millis(100),
                                                    );
                                                }
                                            },
                                        );
                                    });
                                }
                                ui.add_space(8.0);
                                let mut context_policy = settings
                                    .repertoire_context_policies
                                    .get(id.as_str())
                                    .copied()
                                    .unwrap_or_default();
                                if crate::learning_context_controls::show(ui, &mut context_policy) {
                                    settings
                                        .repertoire_context_policies
                                        .insert(id.to_string(), context_policy);
                                    actions.push(RepertoireAction::SettingsChanged);
                                }
                                ui.add_space(6.0);
                                let preferences_changed = previous_side != self.side
                                    || previous_queue != self.queue
                                    || previous_priority != self.priority;
                                if previous_side != self.side || previous_queue != self.queue {
                                    settings.repertoire_modes.insert(
                                        id.to_string(),
                                        (training_mode(self.side), self.queue),
                                    );
                                }
                                if previous_priority != self.priority {
                                    settings
                                        .repertoire_priorities
                                        .insert(id.to_string(), self.priority);
                                }
                                if preferences_changed {
                                    self.close_editor();
                                    self.invalidate_reset_jobs();
                                    actions.push(RepertoireAction::SettingsChanged);
                                }
                                ui.separator();
                                self.statistics_panel(ui);
                            });
                        });
                }
            });
        ui.separator();
        self.show_library(ui, settings);
    }
    fn current_aggregate_key(&self) -> Option<AggregateKey> {
        Some(AggregateKey {
            id: self.selected.clone()?,
            side: self.side,
            library_revision: self.library.revision,
            learning_revision: self.learning.revision,
            minute: now() / 60,
        })
    }
    fn selected_aggregate(&mut self) -> Option<&ScopedLearningAggregate> {
        let key = self.current_aggregate_key()?;
        self.request_aggregate(key.clone());
        self.aggregates.get(&key)?.as_ref()?.as_ref().ok()
    }
    fn request_aggregate(&mut self, key: AggregateKey) {
        if self.aggregates.contains_key(&key) {
            return;
        }
        if self.aggregates.iter().any(|(pending, value)| {
            let mut comparable = pending.clone();
            comparable.minute = key.minute;
            value.is_none() && comparable == key
        }) {
            return;
        }
        let max_depth = self
            .library
            .repertoire(&key.id)
            .map(|repertoire| repertoire.max_plies);
        let scope =
            StatsScope::for_repertoire(key.id.clone(), key.side, now()).with_max_depth(max_depth);
        let library = self.library.clone();
        let learning = self.learning.learning.clone();
        let tx = self.tx.clone();
        let job_key = key.clone();
        self.aggregates.retain(|old, value| {
            old.library_revision == key.library_revision
                && old.learning_revision == key.learning_revision
                && (value.is_none() || old.minute >= key.minute - 1)
        });
        self.aggregates.insert(key.clone(), None);
        if let Err(error) = crate::platform::spawn("repertoire-aggregate", move || {
            let result = aggregate_scope(&library, &learning, &scope, &SrsSettings::default())
                .map_err(|error| error.to_string());
            let _ = tx.send(Event::Aggregated(job_key, Box::new(result)));
        }) {
            self.aggregates.remove(&key);
            self.error = Some(error.to_string());
        }
    }
    fn preview_board(&mut self, ui: &mut Ui, settings: &Settings) {
        let pos = self
            .tree
            .as_ref()
            .and_then(|tree| tree.game().position(NodeId::ROOT))
            .cloned()
            .unwrap_or_default();
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
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            BoardView {
                pos: &pos,
                orientation: self.side.color(),
                last_move: None,
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
    fn statistics_panel(&mut self, ui: &mut Ui) {
        let aggregate = self
            .selected_aggregate()
            .map(|scoped| (scoped.scope.clone(), scoped.summary.clone()));
        let summary = aggregate.as_ref().map(|(_, summary)| summary);
        let count = |metric: fn(&LearningAggregate) -> usize| {
            summary.map_or_else(|| "…".to_owned(), |summary| metric(summary).to_string())
        };
        let percent = |metric: fn(&LearningAggregate) -> Option<f64>| {
            summary.map_or_else(|| "…".to_owned(), |summary| format_percent(metric(summary)))
        };
        ui.horizontal(|ui| {
            ui.strong(tr("Statistics"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = summary.is_some_and(|summary| summary.total > 0)
                    && !self.has_unsaved()
                    && !self.preparing;
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(tr("Reset"))
                            .min_size(egui::vec2(ACTION_BUTTON_WIDTH, 26.0)),
                    )
                    .clicked()
                    && let Some((scope, _)) = aggregate.clone()
                {
                    self.begin_shared_reset(scope);
                }
                if ui
                    .add_enabled(
                        !self.has_unsaved() && !self.preparing,
                        egui::Button::new(tr("Details"))
                            .min_size(egui::vec2(ACTION_BUTTON_WIDTH, 26.0)),
                    )
                    .clicked()
                {
                    self.open_dashboard();
                }
            });
        });
        self.shared_reset_panel(ui);
        ui.add(
            egui::Label::new(format!(
                "{} · {}",
                tr("Scope: whole repertoire"),
                learning_side_label(self.side)
            ))
            .truncate(),
        );
        ui.label(format!(
            "{}: {}",
            tr("Calculated at"),
            aggregate.as_ref().map_or_else(
                || "…".to_owned(),
                |(scope, _)| format_time(Some(scope.as_of))
            )
        ));
        if summary.is_none() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        egui::Grid::new("repertoire_statistics")
            .num_columns(4)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new(tr("Total")).weak());
                ui.label(count(|summary| summary.total));
                ui.label(RichText::new(tr("Unseen")).weak());
                ui.label(count(|summary| summary.unseen));
                ui.end_row();
                ui.label(RichText::new(tr("Learning")).weak());
                ui.label(count(|summary| summary.learning));
                ui.label(RichText::new(tr("Learnt")).weak());
                ui.label(count(|summary| summary.learnt));
                ui.end_row();
                ui.label(RichText::new(tr("Uncertain")).weak());
                ui.label(count(|summary| summary.uncertain));
                ui.label(RichText::new(tr("Due")).weak());
                ui.label(count(|summary| summary.due));
                ui.end_row();
                ui.label(RichText::new(tr("Learnt %")).weak());
                ui.label(percent(|summary| summary.learnt_percent));
                ui.label(RichText::new(tr("Confident %")).weak());
                ui.label(percent(|summary| summary.confident_percent));
                ui.end_row();
                ui.label(RichText::new(tr("Next review")).weak());
                ui.label(summary.map_or_else(
                    || "…".to_owned(),
                    |summary| format_time(summary.next_due_at),
                ));
                ui.end_row();
            });
        ui.add(
            egui::Label::new(
                RichText::new(tr(
                    "Learnt = Confident + Uncertain · percentages use Total.",
                ))
                .weak(),
            )
            .wrap(),
        );
    }
    fn shared_reset_panel(&mut self, ui: &mut Ui) {
        let Some(state) = &self.shared_reset else {
            return;
        };
        let impact = state.impact.clone();
        let busy = state.busy;
        ui.separator();
        let Some(impact) = impact else {
            ui.spinner();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        ui.label(format!(
            "{}: {} · {}: {}",
            tr("Learning items"),
            impact.reset_count,
            tr("Shared"),
            impact.shared_count
        ));
        let names = impact
            .affected_repertoires
            .iter()
            .map(|repertoire| repertoire.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        ui.add(
            egui::Label::new(format!(
                "{}: {}",
                tr("Affected repertoires"),
                if names.is_empty() { "—" } else { &names }
            ))
            .wrap(),
        );
        ui.label(
            RichText::new(tr(
                "Progress in other repertoires will also become Unseen. PGN files will not change.",
            ))
            .weak(),
        );
        let mut confirmed = state.confirmed;
        ui.add_enabled_ui(!busy, |ui| {
            ui.checkbox(
                &mut confirmed,
                tr("I understand that shared progress will be reset"),
            );
        });
        if let Some(state) = &mut self.shared_reset {
            state.confirmed = confirmed;
        }
        let mut cancel = false;
        let mut confirm = false;
        ui.horizontal(|ui| {
            cancel = ui
                .add_enabled(!busy, egui::Button::new(tr("Cancel")))
                .clicked();
            confirm = ui
                .add_enabled(
                    confirmed && !busy && impact.reset_count > 0,
                    egui::Button::new(tr("Reset shared learning data")),
                )
                .clicked();
            if busy {
                ui.spinner();
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
        if cancel {
            self.cancel_shared_reset();
        }
        if confirm {
            self.confirm_shared_reset();
        }
    }
    fn show_add(&mut self, ui: &mut Ui, actions: &mut Vec<RepertoireAction>) {
        let Some(mut add) = self.add.take() else {
            return;
        };
        let mut cancel = false;
        let mut commit = false;
        egui::Modal::new(egui::Id::new("add_to_repertoire")).show(ui.ctx(), |ui| {
            ui.set_max_width(450.0);
            ui.heading(tr("Add to repertoire"));
            ui.label(prefix_label(&add.source, add.at));
            let start = fen4(add.source.start().position());
            for r in &self.library.repertoires {
                if self
                    .trees
                    .get(&r.id)
                    .is_some_and(|tree| fen4(tree.game().start().position()) == start)
                {
                    ui.radio_value(&mut add.selected, Some(r.id.clone()), &r.name);
                }
            }
            ui.radio_value(&mut add.selected, None, tr("New repertoire"));
            if add.selected.is_none() {
                ui.add(
                    egui::TextEdit::singleline(&mut add.name)
                        .char_limit(120)
                        .hint_text(tr("Name")),
                );
            }
            ui.horizontal(|ui| {
                cancel = ui.button(tr("Cancel")).clicked();
                commit = ui
                    .add_enabled(!self.indexing, egui::Button::new(tr("Add")))
                    .clicked();
            });
        });
        if commit {
            let result = (|| {
                let mut library = self.library.clone();
                let id = if let Some(id) = &add.selected {
                    id.clone()
                } else {
                    let name = self.checked_name(&add.name, None)?;
                    library
                        .create(name, &Game::from_start(add.source.start().clone()))
                        .map_err(|e| e.to_string())?
                };
                let report = library
                    .add_prefix(&id, &add.source, add.at)
                    .map_err(|e| e.to_string())?;
                let name = library
                    .repertoire(&id)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                Ok::<_, String>((library, id, name, report))
            })();
            match result {
                Ok((library, id, name, report)) if self.persist(library.clone()) => {
                    actions.push(RepertoireAction::Added { id, name, report });
                    return;
                }
                Ok(_) => {}
                Err(e) => self.error = Some(e),
            }
        }
        if !cancel {
            self.add = Some(add);
        }
    }
    pub fn debug_import(&mut self, path: PathBuf) {
        self.import_path(path, None, None);
    }
    pub fn debug_confirm_import(&mut self, settings: &Settings) -> bool {
        self.confirm_import(settings)
    }
    pub fn debug_export(&mut self) {
        self.export(None);
    }
    pub fn debug_rename(&mut self, name: String, _settings: &Settings) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let mut library = self.library.clone();
        if let Err(e) = library.rename(&id, name) {
            self.error = Some(e.to_string());
            return false;
        }
        self.persist(library)
    }
    pub fn debug_delete(&mut self, _settings: &Settings) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let mut library = self.library.clone();
        if library.delete(&id).is_err() {
            return false;
        }
        if !self.persist(library) {
            return false;
        }
        self.selected = None;
        true
    }
    pub fn debug_add_from_game(&mut self, source: Game, at: NodeId, title: String, target: String) {
        self.open_add_from_game(source, at, title);
        if let Some(add) = &mut self.add {
            add.name = target;
            add.selected = None;
        }
    }
    pub fn debug_open(&mut self, what: &str, settings: &mut Settings) {
        if let Some(id) = self.selected.clone() {
            self.select(id, settings);
        }
        match what {
            "library" | "detail" => {}
            "dashboard" => self.open_dashboard(),
            "editor" => self.open_editor(),
            "white" | "black" => {
                if self.selected.is_none() || self.cancel.is_some() {
                    self.debug_start = Some(what.to_owned());
                    return;
                }
                self.side = if what == "black" {
                    LearningSide::Black
                } else {
                    LearningSide::White
                };
                if self.selected_aggregate().is_none() {
                    self.debug_start = Some(what.to_owned());
                    return;
                }
                self.start(settings);
            }
            _ => {}
        }
    }
    pub fn debug_moves(&mut self, moves: &str) {
        if let Some(t) = &mut self.training {
            t.debug_moves(moves);
        } else {
            self.debug_pending_moves = Some(moves.to_owned());
        }
    }
    pub fn debug_hint(&mut self) {
        if let Some(t) = &mut self.training {
            t.debug_hint();
        }
    }
}

impl Drop for RepertoireUi {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn read_import(
    path: &Path,
    target: Option<RepertoireId>,
    mut library: Library,
    cancel: &AtomicBool,
) -> Result<ImportPreview, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let db =
        PgnDatabase::from_bytes(bytes, None, &mut |_| {}, cancel).map_err(|e| e.to_string())?;
    if db.is_empty() {
        return Err(tr("No games").to_owned());
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| tr("New repertoire").to_owned());
    let mut name = stem.clone();
    let mut suffix = 2;
    while library
        .repertoires
        .iter()
        .any(|r| r.name.to_lowercase() == name.to_lowercase())
    {
        name = format!("{stem} ({suffix})");
        suffix += 1;
    }
    let mut id = target.clone();
    let mut report = MergeReport::default();
    for index in 0..db.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(tr("Cancelled").to_owned());
        }
        let parsed = db
            .load_game(index)
            .ok_or_else(|| tr("No games").to_owned())?;
        if let Some(d) = parsed
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
        {
            return Err(format!("PGN #{}: {}", index + 1, d.message));
        }
        let current = match &id {
            Some(id) => id.clone(),
            None => {
                let new_id = library
                    .create(name.clone(), &Game::from_start(parsed.game.start().clone()))
                    .map_err(|e| e.to_string())?;
                id = Some(new_id.clone());
                new_id
            }
        };
        let added = library
            .add_game(&current, &parsed.game)
            .map_err(|e| format!("PGN #{}: {e}", index + 1))?;
        report.added_nodes += added.added_nodes;
        report.merged_nodes += added.merged_nodes;
        report.annotation_changes += added.annotation_changes;
    }
    let id = id.ok_or_else(|| tr("No games").to_owned())?;
    Ok(ImportPreview {
        library,
        id,
        records: db.len(),
        report,
        name,
        target,
    })
}
fn pgn_route_count(tree: &RepertoireTree) -> usize {
    let mut count = 0usize;
    let _ = tree.visit_source_paths(&TrainingScope::whole(), |_| {
        count = count.saturating_add(1);
        std::ops::ControlFlow::Continue(())
    });
    count
}
fn start_kind(summary: &LearningAggregate) -> StartKind {
    if summary.total == 0 {
        StartKind::Unavailable
    } else if summary.due > 0 {
        StartKind::Review
    } else if summary.unseen > 0 {
        StartKind::IntroduceNew
    } else {
        StartKind::Unavailable
    }
}
fn mode_start_kind(queue: QueueMode, summary: &LearningAggregate) -> StartKind {
    match queue {
        QueueMode::SpacedRepetition => start_kind(summary),
        QueueMode::ReviewInOrder | QueueMode::LeastRecent if summary.total > 0 => StartKind::Review,
        QueueMode::Practice | QueueMode::ReviewInOrder | QueueMode::LeastRecent => {
            StartKind::Unavailable
        }
    }
}
fn queue_mode_label(queue: QueueMode) -> &'static str {
    tr(match queue {
        QueueMode::Practice | QueueMode::SpacedRepetition => "Review individual moves",
        QueueMode::ReviewInOrder => "Review in order",
        QueueMode::LeastRecent => "Least recent and unseen first",
    })
}
fn learning_priority_label(priority: LearningPriority) -> &'static str {
    tr(match priority {
        LearningPriority::DepthFirst => "Depth first",
        LearningPriority::BreadthOverDepth => "Breadth over depth",
        LearningPriority::MainLinesFirst => "Main lines first",
        LearningPriority::CommonPositionsFirst => "Common positions first",
    })
}
fn training_mode(side: LearningSide) -> TrainingMode {
    match side {
        LearningSide::White => TrainingMode::White,
        LearningSide::Black => TrainingMode::Black,
    }
}
fn learning_side_label(side: LearningSide) -> &'static str {
    tr(match side {
        LearningSide::White => "White",
        LearningSide::Black => "Black",
    })
}
fn format_percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0} %"))
}
fn opening_labels(tree: &RepertoireTree) -> Vec<OpeningLabel> {
    let mut labels = BTreeSet::new();
    let start = tree.game().start().position();
    let _ = tree.visit_source_paths(&TrainingScope::whole(), |moves| {
        let mut position = start.clone();
        let mut keys = Vec::with_capacity(moves.len() + 1);
        keys.push(chess_openings::key(&position));
        for uci in moves {
            let Some(mv) = uci
                .parse::<UciMove>()
                .ok()
                .and_then(|uci| uci.to_move(&position).ok())
            else {
                return std::ops::ControlFlow::Continue(());
            };
            position.play_unchecked(mv);
            keys.push(chess_openings::key(&position));
        }
        if let Some((_, id)) = chess_openings::deepest(&keys) {
            let opening = chess_openings::get(id);
            labels.insert(OpeningLabel {
                eco: opening.eco.clone(),
                name: opening.name.clone(),
            });
        }
        std::ops::ControlFlow::Continue(())
    });
    labels.into_iter().collect()
}

fn table_text_line(ui: &mut Ui, text: RichText, height: f32) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(
                egui::Label::new(text)
                    .selectable(false)
                    .truncate()
                    .halign(egui::Align::Min),
            )
        },
    )
    .inner
}

fn count_cell(ui: &mut Ui, count: Option<usize>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if let Some(count) = count {
            ui.label(count.to_string());
        } else {
            ui.spinner();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    });
}
fn now() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
fn load_library(store: &LibraryStore) -> Result<Library, LibraryLoadError> {
    store.load().map_err(|error| LibraryLoadError {
        message: error.to_string(),
    })
}
fn prefix_label(game: &Game, at: NodeId) -> String {
    game.path(at)
        .into_iter()
        .skip(1)
        .filter_map(|id| game.move_label(id))
        .map(|s| crate::san::show(&s).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}
fn stores() -> (LibraryStore, LearningStore) {
    let legacy = std::env::var_os("CP_TRAINING_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
                .map(|d| d.data_local_dir().join("training.json"))
        })
        .unwrap_or_else(|| PathBuf::from("training.json"));
    let dir = std::env::var_os("CP_REPERTOIRE_DIR")
        .map(PathBuf::from)
        .or_else(|| legacy.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    (
        LibraryStore::new(dir.join("repertoires.json")),
        LearningStore::with_legacy(dir.join("repertoire-training.json"), legacy),
    )
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

#[cfg(test)]
#[path = "repertoire_ui_tests.rs"]
mod tests;
