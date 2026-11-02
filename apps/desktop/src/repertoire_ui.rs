use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};

use chess_core::shakmaty::Color;
use chess_core::{Game, NodeId, PgnDatabase, Severity};
use egui::{Key, Modifiers, RichText, Ui};
use trainer::repertoire::{
    Library, LibraryStore, MergeReport, MoveKey, QueueMode, Repertoire, RepertoireId,
    RepertoireTree, SrsPolicy, StatsData, StatsStore, TrainingMode, TrainingScope, fen4,
};

use crate::board::{BoardState, BoardView};
use crate::i18n::tr;
use crate::repertoire_training::{TrainingAction, TrainingUi, mode_label};
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

struct LibraryRow {
    id: RepertoireId,
    name: String,
}

struct ImportPreview {
    library: Library,
    id: RepertoireId,
    records: usize,
    report: MergeReport,
    name: String,
    target: Option<RepertoireId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CountKey {
    id: RepertoireId,
    mode: TrainingMode,
    queue: QueueMode,
    policy: SrsPolicy,
    scope: TrainingScope,
    library_revision: u64,
    stats_revision: u64,
    minute: i64,
}

enum Event {
    Counted(CountKey, usize),
    Indexed(u64, HashMap<RepertoireId, Arc<RepertoireTree>>),
    TrainingReady(u64, Result<Box<TrainingUi>, String>),
    Imported(Result<ImportPreview, String>),
    Exported(Result<(), String>),
}

struct AddFromGame {
    source: Game,
    at: NodeId,
    selected: Option<RepertoireId>,
    name: String,
}

pub struct RepertoireUi {
    library_store: LibraryStore,
    stats_store: StatsStore,
    library: Library,
    stats: StatsData,
    library_error: Option<String>,
    stats_error: Option<String>,
    error: Option<String>,
    selected: Option<RepertoireId>,
    detail: bool,
    active: NodeId,
    scope: TrainingScope,
    tree: Option<Arc<RepertoireTree>>,
    trees: HashMap<RepertoireId, Arc<RepertoireTree>>,
    counts: HashMap<CountKey, Option<usize>>,
    index_generation: u64,
    indexing: bool,
    preparing: bool,
    training_generation: u64,
    rows: Vec<(NodeId, usize)>,
    expanded: HashSet<NodeId>,
    cleanup: Option<RepertoireId>,
    board: BoardState,
    mode: TrainingMode,
    queue: QueueMode,
    policy: SrsPolicy,
    training: Option<TrainingUi>,
    training_visible: bool,
    return_to_game: Option<String>,
    add: Option<AddFromGame>,
    edit_name: Option<(Option<RepertoireId>, String)>,
    delete: Option<RepertoireId>,
    search: String,
    legacy: bool,
    import: Option<ImportPreview>,
    cancel: Option<Arc<AtomicBool>>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    debug_start: Option<String>,
    debug_pending_moves: Option<String>,
}

impl RepertoireUi {
    pub fn new() -> Self {
        let (library_store, stats_store) = stores();
        Self::with_stores(library_store, stats_store)
    }
    fn with_stores(library_store: LibraryStore, stats_store: StatsStore) -> Self {
        let (library, library_error) = match library_store.load() {
            Ok(data) => (data, None),
            Err(e) => (Library::default(), Some(e.to_string())),
        };
        let (stats, stats_error) = match stats_store.load() {
            Ok(data) => (data, None),
            Err(e) => (StatsData::default(), Some(e.to_string())),
        };
        let selected = library.repertoires.first().map(|r| r.id.clone());
        let (tx, rx) = mpsc::channel();
        let mut this = Self {
            library_store,
            stats_store,
            library,
            stats,
            library_error,
            stats_error,
            error: None,
            selected,
            detail: false,
            active: NodeId::ROOT,
            scope: TrainingScope::whole(),
            tree: None,
            trees: HashMap::new(),
            counts: HashMap::new(),
            index_generation: 0,
            indexing: false,
            preparing: false,
            training_generation: 0,
            rows: Vec::new(),
            expanded: HashSet::new(),
            cleanup: None,
            board: BoardState::default(),
            mode: TrainingMode::White,
            queue: QueueMode::Practice,
            policy: SrsPolicy::Due,
            training: None,
            training_visible: false,
            return_to_game: None,
            add: None,
            edit_name: None,
            delete: None,
            search: String::new(),
            legacy: false,
            import: None,
            cancel: None,
            tx,
            rx,
            debug_start: None,
            debug_pending_moves: None,
        };
        this.rebuild_tree();
        this
    }

    pub fn has_unsaved(&self) -> bool {
        self.training.as_ref().is_some_and(TrainingUi::has_unsaved)
    }
    pub fn resolve_unsaved(&mut self) {
        if self.has_unsaved() {
            self.training_visible = true;
        }
    }
    pub fn open_library(&mut self) {
        self.preparing = false;
        self.detail = false;
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
        self.preparing = false;
        self.selected = Some(id.clone());
        self.detail = true;
        self.active = NodeId::ROOT;
        self.expanded.clear();
        self.scope = TrainingScope::whole();
        let (mode, queue) = settings
            .repertoire_modes
            .get(id.as_str())
            .copied()
            .unwrap_or((TrainingMode::White, QueueMode::Practice));
        self.mode = mode;
        self.queue = queue;
        self.update_selected_tree();
    }
    fn update_selected_tree(&mut self) {
        self.tree = self
            .selected
            .as_ref()
            .and_then(|id| self.trees.get(id))
            .cloned();
        self.rows = self
            .tree
            .as_ref()
            .map(|tree| tree_rows(tree.game(), &self.expanded))
            .unwrap_or_default();
    }
    fn rebuild_tree(&mut self) {
        self.counts.clear();
        self.index_generation += 1;
        self.indexing = true;
        self.preparing = false;
        self.trees.clear();
        self.update_selected_tree();
        let generation = self.index_generation;
        let repertoires = self.library.repertoires.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-index", move || {
            let trees = repertoires
                .into_iter()
                .map(|r| (r.id.clone(), Arc::new(RepertoireTree::new(&r))))
                .collect();
            let _ = tx.send(Event::Indexed(generation, trees));
        }) {
            self.indexing = false;
            self.error = Some(error.to_string());
        }
    }
    fn persist(&mut self, mut library: Library) -> bool {
        if self.library_error.is_some() {
            return false;
        }
        match self.library_store.save(&mut library) {
            Ok(()) => {
                self.library = library;
                self.error = None;
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
        if self.stats_error.is_some() || self.library_error.is_some() || self.has_unsaved() {
            return;
        }
        let Some(r) = self
            .selected
            .as_ref()
            .and_then(|id| self.library.repertoire(id))
        else {
            return;
        };
        let queue = if self.mode == TrainingMode::Both && self.queue == QueueMode::SpacedRepetition
        {
            QueueMode::LeastRecent
        } else {
            self.queue
        };
        if self.preparing {
            return;
        }
        let repertoire = r.clone();
        let scope = self.scope.clone();
        let mode = self.mode;
        let policy = self.policy;
        let stats = self.stats.clone();
        let store = self.stats_store.clone();
        let tx = self.tx.clone();
        self.training_generation += 1;
        let generation = self.training_generation;
        settings
            .repertoire_modes
            .insert(r.id.to_string(), (mode, queue));
        self.preparing = true;
        if let Err(error) = crate::platform::spawn("repertoire-training-start", move || {
            let result = TrainingUi::new_with_policy(
                &repertoire,
                scope,
                mode,
                queue,
                stats,
                store,
                false,
                policy,
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
                Event::Indexed(generation, trees) => {
                    if generation == self.index_generation {
                        self.trees = trees;
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
                        Ok(_) => {
                            self.error =
                                Some(tr("No enabled moves for this mode and depth").to_owned())
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Event::Counted(key, count) => {
                    if let Some(value) = self.counts.get_mut(&key) {
                        *value = Some(count);
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
        if self.training_visible {
            if let Some(training) = self.training.as_mut() {
                let name = self
                    .library
                    .repertoire(training.repertoire_id())
                    .map(|r| r.name.as_str())
                    .unwrap_or_default();
                for action in training.show(ui, settings, name) {
                    match action {
                        TrainingAction::Back => {
                            self.stats = training.stats().clone();
                            self.training_visible = false;
                        }
                        TrainingAction::Sound(sound) => {
                            actions.push(RepertoireAction::Sound(sound))
                        }
                        TrainingAction::Toast(text) => actions.push(RepertoireAction::Toast(text)),
                    }
                }
            }
            return actions;
        }
        if !ui.ctx().egui_wants_keyboard_input()
            && !crate::app::overlay_open(ui.ctx())
            && self.add.is_none()
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            if self.detail {
                self.detail = false;
                self.preparing = false;
            } else {
                actions.push(RepertoireAction::Close);
            }
        }
        egui::Panel::top("repertoire_header").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(if self.detail {
                        tr("← Repertoires")
                    } else if self.return_to_game.is_some() {
                        tr("← To game")
                    } else {
                        tr("← Databases")
                    })
                    .clicked()
                {
                    if self.detail {
                        self.detail = false;
                        self.preparing = false;
                    } else {
                        actions.push(RepertoireAction::Close);
                    }
                }
                ui.heading(tr("Opening training"));
                if ui.button("⚙").on_hover_text(tr("Settings")).clicked() {
                    actions.push(RepertoireAction::OpenSettings);
                }
            });
            if self.detail && self.return_to_game.is_some() && ui.link(tr("← To game")).clicked()
            {
                actions.push(RepertoireAction::Close);
            }
            self.errors(ui);
        });
        if self.detail {
            self.show_detail(ui, settings, &mut actions);
        } else {
            egui::CentralPanel::default().show(ui, |ui| self.show_library(ui, settings));
        }
        self.show_add(ui, &mut actions);
        actions
    }
    fn clean_deleted_stats(&mut self) {
        let Some(id) = self.cleanup.clone() else {
            return;
        };
        self.training = None;
        let mut stats = self.stats.clone();
        stats.remove_repertoire(&id);
        match self.stats_store.save(&mut stats) {
            Ok(()) => {
                self.stats = stats;
                self.cleanup = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    fn errors(&mut self, ui: &mut Ui) {
        if self.indexing || self.preparing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Preparing the tree…"));
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(e) = self
            .library_error
            .clone()
            .or_else(|| self.stats_error.clone())
            .or_else(|| self.error.clone())
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                if ui.button(tr("Retry")).clicked() {
                    match self.library_store.load() {
                        Ok(data) => {
                            self.library = data;
                            self.library_error = None;
                            self.rebuild_tree();
                        }
                        Err(e) => self.library_error = Some(e.to_string()),
                    }
                    match self.stats_store.load() {
                        Ok(data) => {
                            self.stats = data;
                            self.stats_error = None;
                        }
                        Err(e) => self.stats_error = Some(e.to_string()),
                    }
                    self.error = None;
                    self.clean_deleted_stats();
                }
            });
        }
        if let Some(cancel) = &self.cancel {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Preparing the tree…"));
                if ui.button(tr("Cancel")).clicked() {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
        }
    }
    fn show_library(&mut self, ui: &mut Ui, settings: &Settings) {
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
            ui.selectable_value(&mut self.legacy, false, tr("Repertoires"));
            if !self.stats.legacy_lines.is_empty() {
                ui.selectable_value(
                    &mut self.legacy,
                    true,
                    crate::i18n::trf(
                        "Previous lines · {n}",
                        &[("n", &self.stats.legacy_lines.len())],
                    ),
                );
            }
        });
        self.name_editor(ui, settings);
        if self.import.is_some() {
            self.import_preview(ui, settings);
        }
        if self.legacy {
            egui::ScrollArea::both().show(ui, |ui| {
                egui::Grid::new("legacy_training")
                    .striped(true)
                    .show(ui, |ui| {
                        for key in ["Line", "Runs", "Last mistakes", "Clean streak", "Recent"] {
                            ui.strong(tr(key));
                        }
                        ui.end_row();
                        for (key, value) in &self.stats.legacy_lines {
                            if let Ok(stats) =
                                serde_json::from_value::<trainer::LineStats>(value.clone())
                            {
                                ui.add(egui::Label::new(key).truncate()).on_hover_text(key);
                                ui.label(stats.runs.to_string());
                                ui.label(
                                    stats.last.map_or_else(|| "—".to_owned(), |v| v.to_string()),
                                );
                                ui.label(stats.clean_streak.to_string());
                                ui.label(&stats.last_date);
                                ui.end_row();
                            }
                        }
                    });
            });
            return;
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
                "Delete repertoire and its statistics? Source PGN files will not change.",
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
                        self.cleanup = Some(id);
                        self.clean_deleted_stats();
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
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        let mut open = None;
        let mut import = None;
        let mut export = None;
        ui.add_space(8.0);
        let compact = ui.available_width() < 760.0;
        egui_extras::TableBuilder::new(ui)
            .id_salt("repertoire_table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::remainder().at_least(120.0))
            .column(egui_extras::Column::exact(if compact {
                82.0
            } else {
                110.0
            }))
            .column(egui_extras::Column::exact(if compact {
                82.0
            } else {
                110.0
            }))
            .column(egui_extras::Column::exact(if compact {
                82.0
            } else {
                130.0
            }))
            .column(egui_extras::Column::exact(28.0))
            .header(30.0, |mut header| {
                for key in ["Name", "White: due", "Black: due", "Both sides", ""] {
                    header.col(|ui| {
                        ui.add(egui::Label::new(RichText::new(tr(key)).strong()).truncate());
                    });
                }
            })
            .body(|body| {
                body.rows(32.0, rows.len(), |mut row| {
                    let r = &rows[row.index()];
                    row.col(|ui| {
                        if ui
                            .add(
                                egui::Button::selectable(
                                    self.selected.as_ref() == Some(&r.id),
                                    &r.name,
                                )
                                .truncate(),
                            )
                            .on_hover_text(&r.name)
                            .clicked()
                        {
                            open = Some(r.id.clone());
                        }
                    });
                    for mode in [TrainingMode::White, TrainingMode::Black] {
                        row.col(|ui| {
                            match self.task_count(
                                &r.id,
                                mode,
                                QueueMode::SpacedRepetition,
                                SrsPolicy::Due,
                                &TrainingScope::whole(),
                            ) {
                                Some(n) => {
                                    ui.label(n.to_string());
                                }
                                None => {
                                    ui.spinner();
                                    ui.ctx().request_repaint_after(
                                        std::time::Duration::from_millis(100),
                                    );
                                }
                            }
                        });
                    }
                    row.col(|ui| {
                        ui.label(
                            self.stats
                                .runs(&r.id, TrainingMode::Both)
                                .map_or_else(|| "—".to_owned(), |s| s.completions.to_string()),
                        );
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
                });
            });
        if let Some(id) = open {
            self.select(id, settings);
        }
        if let Some(id) = import {
            self.pick_import(Some(id), ui.ctx());
        }
        if let Some(id) = export {
            self.selected = Some(id);
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
            match self.checked_name(&name, id.as_ref()) {
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
    fn show_detail(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        let Some(id) = self.selected.clone() else {
            self.detail = false;
            return;
        };
        let Some(r) = self.library.repertoire(&id).cloned() else {
            self.detail = false;
            return;
        };
        let stacked = ui.available_width() < 850.0 && ui.available_height() > 520.0;
        if stacked {
            let panel_height = (ui.available_height() * 0.48).clamp(300.0, 460.0);
            egui::Panel::bottom("repertoire_detail_stacked")
                .resizable(true)
                .default_size(panel_height)
                .size_range(panel_height..=ui.available_height().max(panel_height) * 0.7)
                .show(ui, |ui| self.detail_panel(ui, settings, actions, &id, &r));
        } else {
            let width = ui.available_width();
            egui::Panel::right("repertoire_detail_side")
                .resizable(true)
                .default_size((width * 0.40).clamp(300.0, 460.0))
                .size_range(280.0..=(width - 220.0).max(280.0))
                .show(ui, |ui| self.detail_panel(ui, settings, actions, &id, &r));
        }
        egui::CentralPanel::default().show(ui, |ui| self.preview_board(ui, settings));
    }
    fn detail_panel(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
        id: &RepertoireId,
        r: &Repertoire,
    ) {
        egui::ScrollArea::vertical()
            .id_salt("repertoire_detail_scroll")
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_enabled_ui(!self.preparing, |ui| {
                    let previous = (self.mode, self.queue);
                    ui.add(egui::Label::new(RichText::new(&r.name).heading()).truncate())
                        .on_hover_text(&r.name);
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(tr("Import PGN…")).clicked() {
                            self.pick_import(Some(id.clone()), ui.ctx());
                        }
                        if ui.button(tr("Export PGN…")).clicked() {
                            self.export(Some(ui.ctx().clone()));
                        }
                    });
                    ui.separator();
                    egui::Grid::new("repertoire_options")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(tr("Depth"));
                            ui.horizontal_wrapped(|ui| {
                                let mut depth = r.max_plies;
                                let mut unlimited = depth == u32::MAX;
                                let mut changed =
                                    ui.checkbox(&mut unlimited, tr("No limit")).changed();
                                if !unlimited {
                                    if depth == u32::MAX {
                                        depth = 20;
                                    }
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut depth)
                                                .range(1..=10000)
                                                .suffix(format!(" {}", tr("half-moves"))),
                                        )
                                        .changed();
                                }
                                if changed {
                                    let mut library = self.library.clone();
                                    if let Some(r) = library.repertoire_mut(id) {
                                        r.max_plies = if unlimited { u32::MAX } else { depth };
                                    }
                                    self.persist(library);
                                }
                            });
                            ui.end_row();
                            ui.label(tr("Color"));
                            ui.horizontal_wrapped(|ui| {
                                for mode in
                                    [TrainingMode::White, TrainingMode::Black, TrainingMode::Both]
                                {
                                    ui.selectable_value(&mut self.mode, mode, mode_label(mode));
                                }
                            });
                            ui.end_row();
                            ui.label(tr("Mode"));
                            egui::ComboBox::from_id_salt("repertoire_queue")
                                .width(180.0)
                                .wrap_mode(egui::TextWrapMode::Truncate)
                                .selected_text(queue_label(self.queue))
                                .show_ui(ui, |ui| {
                                    for queue in [
                                        QueueMode::Practice,
                                        QueueMode::SpacedRepetition,
                                    ] {
                                        if self.mode != TrainingMode::Both
                                            || queue != QueueMode::SpacedRepetition
                                        {
                                            ui.selectable_value(
                                                &mut self.queue,
                                                queue,
                                                queue_label(queue),
                                            );
                                        }
                                    }
                                });
                            if self.mode == TrainingMode::Both
                                && self.queue != QueueMode::Practice
                            {
                                self.queue = QueueMode::Practice;
                            }
                            ui.end_row();
                            if self.mode != TrainingMode::Both
                                && self.queue == QueueMode::SpacedRepetition
                            {
                                ui.label(tr("Spaced schedule"));
                                egui::ComboBox::from_id_salt("srs_policy")
                                    .selected_text(policy_label(self.policy))
                                    .show_ui(ui, |ui| {
                                        for policy in [
                                            SrsPolicy::Due,
                                            SrsPolicy::Introduce,
                                            SrsPolicy::EarlyReview,
                                        ] {
                                            ui.selectable_value(
                                                &mut self.policy,
                                                policy,
                                                policy_label(policy),
                                            );
                                        }
                                    });
                            }
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.label(tr(if self.queue == QueueMode::Practice {
                        "Play a variation from start to finish. Any enabled move is accepted; the program plays the other side."
                    } else {
                        "Move review presents individual positions. Any repertoire move is accepted; only the move you play is recorded."
                    }));
                    ui.horizontal_wrapped(|ui| {
                        let available = self.available_tasks();
                        if available.is_none() {
                            ui.spinner();
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(100));
                        }
                        let count_label =
                            available.map_or_else(|| "…".to_owned(), |n| n.to_string());
                        ui.label(format!(
                            "{}: {count_label}",
                            if self.mode == TrainingMode::Both || self.queue == QueueMode::Practice {
                                tr("Paths")
                            } else {
                                tr("Moves")
                            }
                        ));
                        let ready = !self.preparing
                            && (available.is_some_and(|n| n > 0)
                                || ((self.mode == TrainingMode::Both || self.queue == QueueMode::Practice) && self.tree.is_some()))
                            && self.stats_error.is_none()
                            && self.library_error.is_none();
                        if ui
                            .add_enabled(ready, egui::Button::new(tr("Start training")))
                            .clicked()
                        {
                            self.start(settings);
                            actions.push(RepertoireAction::SettingsChanged);
                        }
                        if self.mode != TrainingMode::Both
                            && self.queue == QueueMode::SpacedRepetition
                            && self.policy == SrsPolicy::Due
                            && available == Some(0)
                            && ui.button(tr("Introduce new moves")).clicked()
                        {
                            self.policy = SrsPolicy::Introduce;
                        }
                    });
                    if matches!(self.queue, QueueMode::ReviewInOrder | QueueMode::LeastRecent) {
                        ui.label(tr("Early review · schedule unchanged"));
                    }
                    if previous != (self.mode, self.queue) {
                        settings
                            .repertoire_modes
                            .insert(id.to_string(), (self.mode, self.queue));
                        actions.push(RepertoireAction::SettingsChanged);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(if self.scope.prefix.is_empty() {
                            tr("Whole repertoire").to_owned()
                        } else {
                            format!("{}: {}", tr("Scope"), self.scope.prefix.join(" "))
                        });
                        if ui.button(tr("Reset scope")).clicked() {
                            self.scope = TrainingScope::whole();
                        }
                        if self.active != NodeId::ROOT
                            && ui.button(tr("Use selected as scope")).clicked()
                            && let Some(path) = self.tree.as_ref().and_then(|t| t.path(self.active))
                        {
                            self.scope = TrainingScope::from_prefix(path.to_vec());
                        }
                        if ui.button(tr("End scope here")).clicked()
                            && let Some(path) = self.tree.as_ref().and_then(|t| t.path(self.active))
                        {
                            self.scope.end_prefix = Some(path.to_vec());
                        }
                    });
                    if self.import.is_some() {
                        self.import_preview(ui, settings);
                    }
                    ui.separator();
                    self.tree_panel(ui, r);
                });
            });
    }
    fn available_tasks(&mut self) -> Option<usize> {
        let id = self.selected.clone()?;
        self.task_count(&id, self.mode, self.queue, self.policy, &self.scope.clone())
    }
    fn task_count(
        &mut self,
        id: &RepertoireId,
        mode: TrainingMode,
        queue: QueueMode,
        policy: SrsPolicy,
        scope: &TrainingScope,
    ) -> Option<usize> {
        let key = CountKey {
            id: id.clone(),
            mode,
            queue,
            policy,
            scope: scope.clone(),
            library_revision: self.library.revision,
            stats_revision: self.stats.revision,
            minute: if mode != TrainingMode::Both && queue == QueueMode::SpacedRepetition {
                now() / 60
            } else {
                0
            },
        };
        if let Some(count) = self.counts.get(&key) {
            return *count;
        }
        // Reuse an in-flight count even if a due minute elapsed meanwhile.
        if self.counts.iter().any(|(pending, value)| {
            let mut comparable = pending.clone();
            comparable.minute = key.minute;
            value.is_none() && comparable == key
        }) {
            return None;
        }
        let tree = self.trees.get(id)?.clone();
        let stats = self.stats.clone();
        let tx = self.tx.clone();
        let job_key = key.clone();
        self.counts.retain(|old, value| {
            old.library_revision == key.library_revision
                && old.stats_revision == key.stats_revision
                && (value.is_none() || old.minute == 0 || old.minute >= now() / 60 - 1)
        });
        self.counts.insert(key.clone(), None);
        if let Err(error) = crate::platform::spawn("repertoire-count", move || {
            let count = if mode == TrainingMode::Both || queue == QueueMode::Practice {
                tree.training_path_count(&job_key.scope).unwrap_or(0)
            } else if queue == QueueMode::SpacedRepetition {
                stats
                    .srs_queue(&tree, &job_key.id, mode, now(), &job_key.scope, policy)
                    .len()
            } else {
                stats
                    .scoped_queue(&tree, &job_key.id, mode, queue, now(), &job_key.scope)
                    .len()
            };
            let _ = tx.send(Event::Counted(job_key, count));
        }) {
            self.counts.remove(&key);
            self.error = Some(error.to_string());
        }
        None
    }
    fn preview_board(&mut self, ui: &mut Ui, settings: &Settings) {
        let Some(tree) = &self.tree else {
            return;
        };
        let Some(pos) = tree.game().position(self.active) else {
            return;
        };
        let side = ui.available_width().min(ui.available_height()).max(80.0);
        let available = ui.available_size();
        let offset = egui::vec2(
            (available.x - side).max(0.0) * 0.5,
            (available.y - side).max(0.0) * 0.5,
        );
        let rect = egui::Rect::from_min_size(ui.cursor().min + offset, egui::vec2(side, side));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            BoardView {
                pos,
                orientation: if self.mode == TrainingMode::Black {
                    Color::Black
                } else {
                    Color::White
                },
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
    fn tree_panel(&mut self, ui: &mut Ui, r: &Repertoire) {
        let Some(tree) = &self.tree else {
            return;
        };
        let game = tree.game();
        let mut select = None;
        let mut toggle = None;
        let mut expand = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong(tr("Tree"));
            if ui
                .selectable_label(self.active == NodeId::ROOT, tr("Start position"))
                .clicked()
            {
                select = Some(NodeId::ROOT);
            }
        });
        let rows = &self.rows;
        egui::ScrollArea::vertical()
            .id_salt("repertoire_moves")
            .max_height(260.0)
            .show_rows(ui, 24.0, rows.len(), |ui, range| {
                for index in range {
                    let (node, depth) = rows[index];
                    let path = tree.path(node).unwrap_or(&[]);
                    let disabled = r.is_prefix_disabled(path);
                    ui.horizontal(|ui| {
                        ui.add_space((depth.min(10) as f32) * 12.0);
                        let mut enabled = !disabled;
                        if ui
                            .checkbox(&mut enabled, "")
                            .on_hover_text(tr("Enable for training"))
                            .changed()
                        {
                            toggle = Some((path.to_vec(), !enabled));
                        }
                        if game.children(node).len() > 1 {
                            let is_open = self.expanded.contains(&node);
                            if ui
                                .add_sized(
                                    [18.0, 20.0],
                                    egui::Button::new(if is_open { "▾" } else { "▸" }).frame(false),
                                )
                                .clicked()
                            {
                                expand = Some(node);
                            }
                        } else {
                            ui.add_space(18.0);
                        }
                        let label = game.move_label(node).unwrap_or_default();
                        let text = if disabled {
                            RichText::new(crate::san::show(&label)).weak()
                        } else {
                            RichText::new(crate::san::show(&label))
                        };
                        ui.allocate_ui_with_layout(
                            egui::vec2(100.0, 22.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                if ui.selectable_label(self.active == node, text).clicked() {
                                    select = Some(node);
                                }
                            },
                        );
                        if disabled {
                            ui.label(RichText::new(tr(" ∅ not trained")).weak());
                        }
                        if path.len() > r.max_plies as usize {
                            ui.label(RichText::new(format!(">{}", r.max_plies)).weak());
                        }
                        if !self.scope.prefix.is_empty() && path == self.scope.prefix {
                            ui.label("▶");
                        }
                        let transpositions = tree.transpositions(node);
                        if transpositions.len() > 1 {
                            ui.menu_button(format!("⇄ {}", transpositions.len()), |ui| {
                                for alternate in transpositions {
                                    if ui.button(prefix_label(game, *alternate)).clicked() {
                                        select = Some(*alternate);
                                        ui.close();
                                    }
                                }
                            });
                        }
                        if let Some(n) = game.node(node) {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(n.comments_after().join(" ")).weak(),
                                )
                                .truncate(),
                            );
                        }
                    });
                }
            });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("repertoire_statistics")
            .show(ui, |ui| {
                ui.strong(tr("Statistics"));
                let stats: Vec<_> = self
                    .stats
                    .move_stats
                    .iter()
                    .filter(|s| s.key.repertoire_id == r.id && s.key.mode == self.mode)
                    .collect();
                ui.label(format!(
                    "{}: {} · {}: {}",
                    tr("Attempts"),
                    stats.iter().map(|s| u64::from(s.attempts)).sum::<u64>(),
                    tr("Mistakes"),
                    stats.iter().map(|s| u64::from(s.wrong)).sum::<u64>()
                ));
                if let Some(run) = self.stats.runs(&r.id, self.mode) {
                    ui.label(format!(
                        "{}: {} · {}",
                        tr("Completions"),
                        run.completions,
                        format_time(run.last)
                    ));
                }
                if let Some(n) = game.node(self.active) {
                    ui.label(
                        n.comments_before()
                            .iter()
                            .chain(n.comments_after())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    ui.label(
                        n.nags()
                            .iter()
                            .map(|n| format!("${}", n.0))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
                if self.active != NodeId::ROOT
                    && let Some(before) = game.position_before(self.active)
                {
                    let uci = tree
                        .path(self.active)
                        .and_then(|p| p.last())
                        .cloned()
                        .unwrap_or_default();
                    let key = MoveKey {
                        repertoire_id: r.id.clone(),
                        mode: self.mode,
                        position_fen4: fen4(before),
                        expected_uci: if self.mode == TrainingMode::Both {
                            String::new()
                        } else {
                            uci
                        },
                    };
                    ui.strong(tr("Move statistics"));
                    if let Some(stats) = self.stats.get(&key) {
                        ui.label(format!(
                            "{}: {} · {}: {} · {}: {}",
                            tr("Correct"),
                            stats.correct,
                            tr("Wrong"),
                            stats.wrong,
                            tr("Hints"),
                            stats.hints
                        ));
                        ui.label(format!(
                            "{}: {}",
                            tr("Last attempt"),
                            format_time(stats.last_attempt)
                        ));
                        if self.mode != TrainingMode::Both {
                            ui.label(format!("{}: {}", tr("Next review"), format_time(stats.due)));
                        }
                    } else {
                        ui.label(tr("No attempts in the selected mode"));
                    }
                }
                if self.mode == TrainingMode::Both {
                    let prefix = tree.path(self.active).unwrap_or(&[]);
                    ui.strong(tr("Branch statistics"));
                    for path in
                        self.stats.path_stats.iter().filter(|p| {
                            p.key.repertoire_id == r.id && p.key.path.starts_with(prefix)
                        })
                    {
                        ui.label(format!(
                            "{} · {}: {} · {}",
                            path.key.path.join(" "),
                            tr("Completions"),
                            path.completions,
                            format_time(path.last_completion)
                        ));
                    }
                }
            });
        if let Some(node) = select {
            self.active = node;
            for parent in game.path(node) {
                self.expanded.insert(parent);
            }
        }
        if let Some(node) = expand
            && !self.expanded.remove(&node)
        {
            self.expanded.insert(node);
        }
        if select.is_some() || expand.is_some() {
            self.rows = tree_rows(game, &self.expanded);
        }
        if let Some((path, disabled)) = toggle {
            let mut library = self.library.clone();
            if let Some(rep) = library.repertoire_mut(&r.id) {
                if !disabled && !rep.disabled_prefixes.contains(&path) {
                    self.error = Some(tr("Enable the parent branch first").to_owned());
                    return;
                }
                if let Err(e) = rep.set_prefix_disabled(&path, disabled) {
                    self.error = Some(e.to_string());
                    return;
                }
            }
            self.persist(library);
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
        self.cleanup = Some(id);
        self.selected = None;
        self.clean_deleted_stats();
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
            "library" => self.detail = false,
            "detail" => {}
            "white" | "black" | "both" => {
                if self.selected.is_none() || self.cancel.is_some() {
                    self.debug_start = Some(what.to_owned());
                    return;
                }
                self.mode = match what {
                    "white" => TrainingMode::White,
                    "black" => TrainingMode::Black,
                    _ => TrainingMode::Both,
                };
                self.queue = QueueMode::Practice;
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
fn tree_rows(game: &Game, expanded: &HashSet<NodeId>) -> Vec<(NodeId, usize)> {
    let mut rows = Vec::new();
    let mut stack: Vec<_> = game
        .children(NodeId::ROOT)
        .iter()
        .rev()
        .map(|id| (*id, 0))
        .collect();
    while let Some((id, depth)) = stack.pop() {
        rows.push((id, depth));
        for (index, child) in game.children(id).iter().enumerate().rev() {
            if index == 0 || expanded.contains(&id) {
                stack.push((*child, depth + usize::from(index != 0)));
            }
        }
    }
    rows
}
fn now() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
fn queue_label(q: QueueMode) -> &'static str {
    tr(match q {
        QueueMode::Practice => "Play through variations",
        QueueMode::SpacedRepetition => "Review individual moves",
        QueueMode::ReviewInOrder => "Review in order",
        QueueMode::LeastRecent => "Least recent and unseen first",
    })
}
fn policy_label(p: SrsPolicy) -> &'static str {
    tr(match p {
        SrsPolicy::Due => "Review due moves",
        SrsPolicy::Introduce => "Introduce new moves",
        SrsPolicy::EarlyReview => "Review future moves early",
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
fn stores() -> (LibraryStore, StatsStore) {
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
        StatsStore::with_legacy(dir.join("repertoire-training.json"), legacy),
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
