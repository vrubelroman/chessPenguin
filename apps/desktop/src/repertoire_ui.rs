use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};

use chess_core::shakmaty::{Color, Position, uci::UciMove};
use chess_core::{Game, NodeId, PgnDatabase, Severity};
use egui::{Key, Modifiers, RichText, Ui};
use trainer::repertoire::{
    Library, LibraryStore, MergeReport, QueueMode, Repertoire, RepertoireId, RepertoireTree,
    SrsPolicy, StatsData, StatsStore, TrainingMode, TrainingScope, fen4,
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

impl LibraryRow {
    fn height() -> f32 {
        28.0 + 22.0 * OPENING_LINES as f32
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
    library: Option<Library>,
    message: String,
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
    Indexed(
        u64,
        HashMap<RepertoireId, Arc<RepertoireTree>>,
        HashMap<RepertoireId, Vec<OpeningLabel>>,
    ),
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
    scroll_to_selected: bool,
    tree: Option<Arc<RepertoireTree>>,
    trees: HashMap<RepertoireId, Arc<RepertoireTree>>,
    openings: HashMap<RepertoireId, Vec<OpeningLabel>>,
    counts: HashMap<CountKey, Option<usize>>,
    index_generation: u64,
    indexing: bool,
    preparing: bool,
    training_generation: u64,
    cleanup: Option<RepertoireId>,
    board: BoardState,
    mode: TrainingMode,
    training: Option<TrainingUi>,
    training_visible: bool,
    return_to_game: Option<String>,
    add: Option<AddFromGame>,
    edit_name: Option<(Option<RepertoireId>, String)>,
    delete: Option<RepertoireId>,
    reset_stats: Option<RepertoireId>,
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
        let (library, library_error) = match load_library(&library_store) {
            Ok(data) => (data, None),
            Err(e) => (e.library.unwrap_or_default(), Some(e.message)),
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
            scroll_to_selected: true,
            tree: None,
            trees: HashMap::new(),
            openings: HashMap::new(),
            counts: HashMap::new(),
            index_generation: 0,
            indexing: false,
            preparing: false,
            training_generation: 0,
            cleanup: None,
            board: BoardState::default(),
            mode: TrainingMode::White,
            training: None,
            training_visible: false,
            return_to_game: None,
            add: None,
            edit_name: None,
            delete: None,
            reset_stats: None,
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
        self.scroll_to_selected = true;
        if let Some(training) = &self.training {
            self.stats = training.stats().clone();
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
        self.preparing = false;
        self.training_visible = false;
        self.training = None;
        self.reset_stats = None;
        self.scroll_to_selected = true;
        self.selected = Some(id.clone());
        self.mode = settings
            .repertoire_modes
            .get(id.as_str())
            .map(|(mode, _)| *mode)
            .unwrap_or(TrainingMode::White);
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
        normalize_library(&mut self.library);
        self.counts.clear();
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
            for r in repertoires {
                let tree = Arc::new(RepertoireTree::new(&r));
                openings.insert(r.id.clone(), opening_labels(&tree));
                trees.insert(r.id.clone(), tree);
            }
            let _ = tx.send(Event::Indexed(generation, trees, openings));
        }) {
            self.indexing = false;
            self.error = Some(error.to_string());
        }
    }
    fn persist(&mut self, mut library: Library) -> bool {
        if self.library_error.is_some() {
            return false;
        }
        normalize_library(&mut library);
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
        if self.preparing {
            return;
        }
        let repertoire = r.clone();
        let mode = self.mode;
        let stats = self.stats.clone();
        let store = self.stats_store.clone();
        let tx = self.tx.clone();
        self.training_generation += 1;
        let generation = self.training_generation;
        settings
            .repertoire_modes
            .insert(r.id.to_string(), (mode, QueueMode::Practice));
        self.scroll_to_selected = true;
        self.preparing = true;
        if let Err(error) = crate::platform::spawn("repertoire-training-start", move || {
            let result = TrainingUi::new_random(&repertoire, mode, stats, store, false)
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
                Event::Indexed(generation, trees, openings) => {
                    if generation == self.index_generation {
                        self.trees = trees;
                        self.openings = openings;
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
        if self.training_visible && self.training.is_some() {
            let mut training = self.training.take().expect("checked training session");
            let name = self
                .library
                .repertoire(training.repertoire_id())
                .map(|r| r.name.clone())
                .unwrap_or_default();
            let locked = training.has_unsaved();
            let emitted = training.show(ui, settings, &name, |ui| {
                ui.add_enabled_ui(!locked, |ui| self.show_library(ui, settings));
            });
            self.stats = training.stats().clone();
            if self.training_visible {
                self.training = Some(training);
            }
            for action in emitted {
                match action {
                    TrainingAction::Back => {
                        self.training_visible = false;
                        self.scroll_to_selected = true;
                    }
                    TrainingAction::Sound(sound) => actions.push(RepertoireAction::Sound(sound)),
                    TrainingAction::Toast(text) => actions.push(RepertoireAction::Toast(text)),
                }
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
        if self.indexing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Indexing variations and ECO names…"));
            });
        }
        if self.preparing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Preparing the training session…"));
            });
        }
        if self.indexing || self.preparing || self.cancel.is_some() {
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
                    match load_library(&self.library_store) {
                        Ok(data) => {
                            self.library = data;
                            self.library_error = None;
                            self.rebuild_tree();
                        }
                        Err(e) => {
                            if let Some(library) = e.library {
                                self.library = library;
                                self.rebuild_tree();
                            }
                            self.library_error = Some(e.message);
                        }
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
                ui.label(tr("Importing PGN…"));
                if ui.button(tr("Cancel")).clicked() {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
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
            if !self.stats.legacy_lines.is_empty() {
                let previous = crate::i18n::trf(
                    "Previous lines · {n}",
                    &[("n", &self.stats.legacy_lines.len())],
                );
                if ui.selectable_label(self.legacy, previous).clicked() {
                    self.legacy = !self.legacy;
                }
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
                        self.update_selected_tree();
                        if let Some(next) = self.library.repertoires.first().map(|r| r.id.clone()) {
                            self.select(next, settings);
                        } else {
                            self.training_visible = false;
                            self.training = None;
                        }
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
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&r.name).strong())
                                    .selectable(false)
                                    .truncate(),
                            )
                            .on_hover_text(&r.name);
                            for (index, opening) in r.visible_openings().iter().enumerate() {
                                let more =
                                    index + 1 == OPENING_LINES && r.openings.len() > OPENING_LINES;
                                let text = if more {
                                    format!("{} …", opening.name)
                                } else {
                                    opening.name.clone()
                                };
                                let response = ui.add(
                                    egui::Label::new(RichText::new(text).small())
                                        .selectable(false)
                                        .truncate(),
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
                        ui.vertical(|ui| {
                            if r.openings.is_empty() {
                                ui.label(RichText::new("—").weak());
                            } else {
                                ui.add(
                                    egui::Label::new(RichText::new(" ").strong()).selectable(false),
                                );
                                for (index, opening) in r.visible_openings().iter().enumerate() {
                                    let more = index + 1 == OPENING_LINES
                                        && r.openings.len() > OPENING_LINES;
                                    let text = if more {
                                        format!("{} …", opening.eco)
                                    } else {
                                        opening.eco.clone()
                                    };
                                    ui.add(
                                        egui::Label::new(RichText::new(text).small())
                                            .selectable(false)
                                            .truncate(),
                                    );
                                }
                            }
                        });
                    });
                    row.col(|ui| {
                        let count = self.task_count(
                            &r.id,
                            TrainingMode::Both,
                            QueueMode::Practice,
                            SrsPolicy::Due,
                            &TrainingScope::whole(),
                        );
                        count_cell(ui, count);
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
        if let Some((id, r)) = selected {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.add_enabled_ui(!self.preparing, |ui| {
                        let previous_mode = self.mode;
                        ui.add(egui::Label::new(RichText::new(&r.name).heading()).truncate())
                            .on_hover_text(&r.name);
                        ui.add_space(6.0);
                        egui::Grid::new("repertoire_selected_controls")
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.label(tr("Color"));
                                ui.horizontal(|ui| {
                                    for mode in [
                                        TrainingMode::White,
                                        TrainingMode::Black,
                                        TrainingMode::Both,
                                    ] {
                                        ui.selectable_value(&mut self.mode, mode, mode_label(mode));
                                    }
                                });
                                ui.end_row();
                                let available = self.available_tasks();
                                ui.horizontal(|ui| {
                                    ui.label(tr("Variants"));
                                    if let Some(count) = available {
                                        ui.label(count.to_string());
                                    } else {
                                        ui.spinner();
                                        ui.ctx().request_repaint_after(
                                            std::time::Duration::from_millis(100),
                                        );
                                    }
                                });
                                let ready = !self.preparing
                                    && available.is_some_and(|n| n > 0)
                                    && self.stats_error.is_none()
                                    && self.library_error.is_none();
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_enabled_ui(ready, |ui| {
                                            if crate::widgets::primary_wide(
                                                ui,
                                                tr("Start training"),
                                                ACTION_BUTTON_WIDTH,
                                            )
                                            .clicked()
                                            {
                                                self.start(settings);
                                                actions.push(RepertoireAction::SettingsChanged);
                                            }
                                        });
                                    },
                                );
                                ui.end_row();
                            });
                        if previous_mode != self.mode {
                            self.reset_stats = None;
                            settings
                                .repertoire_modes
                                .insert(id.to_string(), (self.mode, QueueMode::Practice));
                            actions.push(RepertoireAction::SettingsChanged);
                        }
                        ui.separator();
                        self.statistics_panel(ui, &r);
                    });
                });
        }
        ui.separator();
        self.show_library(ui, settings);
    }
    fn available_tasks(&mut self) -> Option<usize> {
        let id = self.selected.clone()?;
        self.task_count(
            &id,
            self.mode,
            QueueMode::Practice,
            SrsPolicy::Due,
            &TrainingScope::whole(),
        )
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
            let count = if queue == QueueMode::Practice {
                use chess_core::shakmaty::Position;
                let start_turn = tree.position(tree.root()).map(|position| position.turn());
                let mut count = 0usize;
                tree.visit_source_paths(&job_key.scope, |moves| {
                    if start_turn.is_some_and(|turn| path_has_mode_move(mode, turn, moves.len())) {
                        count = count.saturating_add(1);
                    }
                    std::ops::ControlFlow::Continue(())
                })
                .map(|()| count)
                .unwrap_or(0)
            } else if mode == TrainingMode::Both {
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
    fn statistics_panel(&mut self, ui: &mut Ui, r: &Repertoire) {
        let stats: Vec<_> = self
            .stats
            .move_stats
            .iter()
            .filter(|s| s.key.repertoire_id == r.id && s.key.mode == self.mode)
            .collect();
        let attempts = stats.iter().map(|s| u64::from(s.attempts)).sum::<u64>();
        let mistakes = stats.iter().map(|s| u64::from(s.wrong)).sum::<u64>();
        let run = self.stats.runs(&r.id, self.mode);
        let has_data = !stats.is_empty() || run.is_some();
        let (completions, last) = run
            .map(|run| (run.completions, format_time(run.last)))
            .unwrap_or_else(|| (0, "—".to_owned()));
        ui.horizontal(|ui| {
            ui.strong(tr("Statistics"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        has_data,
                        egui::Button::new(tr("Reset"))
                            .min_size(egui::vec2(ACTION_BUTTON_WIDTH, 26.0)),
                    )
                    .clicked()
                {
                    self.reset_stats = Some(r.id.clone());
                }
            });
        });
        egui::Grid::new("repertoire_statistics")
            .num_columns(4)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new(tr("Attempts")).weak());
                ui.label(attempts.to_string());
                ui.label(RichText::new(tr("Mistakes")).weak());
                ui.label(mistakes.to_string());
                ui.end_row();
                ui.label(RichText::new(tr("Completions")).weak());
                ui.label(format!("{completions} · {last}"));
                ui.end_row();
            });
        if self.reset_stats.as_ref() == Some(&r.id) {
            ui.label(tr("Reset statistics for this repertoire and color?"));
            ui.horizontal(|ui| {
                if ui.button(tr("Cancel")).clicked() {
                    self.reset_stats = None;
                }
                if ui.button(tr("Reset")).clicked() {
                    self.reset_statistics(&r.id);
                }
            });
        }
    }

    fn reset_statistics(&mut self, id: &RepertoireId) {
        let mut stats = self.stats.clone();
        stats.remove_repertoire_mode(id, self.mode);
        match self.stats_store.save(&mut stats) {
            Ok(()) => {
                self.stats = stats;
                self.reset_stats = None;
            }
            Err(error) => self.error = Some(error.to_string()),
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
            "library" | "detail" => {}
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
fn path_has_mode_move(mode: TrainingMode, start_turn: Color, plies: usize) -> bool {
    match mode.user_color() {
        Some(color) => plies > usize::from(start_turn != color),
        None => plies > 0,
    }
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
    let mut library = store.load().map_err(|error| LibraryLoadError {
        library: None,
        message: error.to_string(),
    })?;
    if normalize_library(&mut library)
        && let Err(error) = store.save(&mut library)
    {
        return Err(LibraryLoadError {
            library: Some(library),
            message: error.to_string(),
        });
    }
    Ok(library)
}
fn normalize_library(library: &mut Library) -> bool {
    let mut changed = false;
    for repertoire in &mut library.repertoires {
        if repertoire.max_plies != u32::MAX {
            repertoire.max_plies = u32::MAX;
            changed = true;
        }
        if !repertoire.disabled_prefixes.is_empty() {
            repertoire.disabled_prefixes.clear();
            changed = true;
        }
    }
    changed
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
