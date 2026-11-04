use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use chess_core::shakmaty::{CastlingMode, Chess, fen::Fen, san::San, uci::UciMove};
use egui::{Align, Key, Layout, Modifiers, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use trainer::repertoire::{
    BranchLearningAnalytics, LearningAggregate, LearningItemKey, LearningSide, LearningSnapshot,
    LearningStore, LearningStoreData, Library, LocalDatePolicy, MoveLearningStatus,
    PersistedLearningSnapshot, RepertoireId, RepertoireTree, ScheduledBuckets, SequenceHistograms,
    SrsSettings, StatsScope, aggregate_scope, branch_learning_analytics, scheduled_buckets,
    scoped_move_learning_analytics, sequence_histograms,
};

use crate::i18n::{tr, trf};
use crate::learning_branches::BranchSelection;
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearningDashboardAction {
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopeSelection {
    Global,
    Repertoire(RepertoireId),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DashboardTab {
    #[default]
    Summary,
    Branches,
    History,
    Scheduled,
    Sequences,
    Moves,
}

struct SummaryRow {
    id: Option<RepertoireId>,
    name: String,
    summary: LearningAggregate,
}

struct DashboardView {
    selection: ScopeSelection,
    scope: StatsScope,
    summary: LearningAggregate,
    branches: Vec<BranchLearningAnalytics>,
    history: Vec<LearningSnapshot>,
    scheduled: ScheduledBuckets,
    sequences: SequenceHistograms,
    moves: Vec<MoveRow>,
}

struct DashboardData {
    side: LearningSide,
    as_of: i64,
    timezone: LocalDatePolicy,
    summary: Vec<SummaryRow>,
    views: Vec<DashboardView>,
    names: HashMap<RepertoireId, String>,
    labels: HashMap<(RepertoireId, Vec<String>), Vec<String>>,
}

struct MoveRow {
    key: LearningItemKey,
    san: String,
    status: MoveLearningStatus,
    resistant: bool,
    accuracy_percent: Option<f64>,
    independent_attempts: u64,
    correct_count: u64,
    wrong_count: u64,
    correct_streak: u32,
    wrong_streak: u32,
    next_review_at: Option<i64>,
    review_in_seconds: Option<i64>,
    overdue_seconds: Option<i64>,
    uncertain_grace_seconds: Option<i64>,
    response_time_median_ms: Option<f64>,
    wrong_moves: Vec<WrongMoveRow>,
    memberships: Vec<MembershipRow>,
}

struct WrongMoveRow {
    san: String,
    count: u64,
    last_at: i64,
}

struct MembershipRow {
    repertoire: String,
    branch: Vec<String>,
}

enum Event {
    Loaded(u64, Box<Result<DashboardData, String>>),
    Snapshot(u64, Box<Result<PersistedLearningSnapshot, String>>),
}

pub struct LearningDashboardUi {
    library: Library,
    learning_store: LearningStore,
    selection: ScopeSelection,
    side: LearningSide,
    tab: DashboardTab,
    data: Option<DashboardData>,
    loading: bool,
    error: Option<String>,
    snapshot_pending: bool,
    snapshot_message: Option<String>,
    snapshot_error: Option<String>,
    automatic_snapshot_scope: Option<StatsScope>,
    selected_move: Option<LearningItemKey>,
    selected_branch: Option<BranchSelection>,
    generation: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl LearningDashboardUi {
    pub fn new(library: Library, learning_store: LearningStore) -> Self {
        Self::with_selection(
            library,
            learning_store,
            ScopeSelection::Global,
            LearningSide::White,
        )
    }

    pub fn new_for_repertoire(
        library: Library,
        learning_store: LearningStore,
        repertoire: RepertoireId,
        side: LearningSide,
    ) -> Self {
        let selection = if library.repertoire(&repertoire).is_some() {
            ScopeSelection::Repertoire(repertoire)
        } else {
            ScopeSelection::Global
        };
        Self::with_selection(library, learning_store, selection, side)
    }

    fn with_selection(
        library: Library,
        learning_store: LearningStore,
        selection: ScopeSelection,
        side: LearningSide,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut this = Self {
            library,
            learning_store,
            selection,
            side,
            tab: match std::env::var("CP_LEARNING_DASHBOARD_TAB").as_deref() {
                Ok("branches") => DashboardTab::Branches,
                Ok("history") => DashboardTab::History,
                Ok("scheduled") => DashboardTab::Scheduled,
                Ok("sequences") => DashboardTab::Sequences,
                Ok("moves") => DashboardTab::Moves,
                _ => DashboardTab::Summary,
            },
            data: None,
            loading: false,
            error: None,
            snapshot_pending: false,
            snapshot_message: None,
            snapshot_error: None,
            automatic_snapshot_scope: None,
            selected_move: None,
            selected_branch: None,
            generation: 0,
            tx,
            rx,
        };
        this.reload();
        this
    }

    #[cfg(test)]
    pub fn selected_repertoire(&self) -> Option<&RepertoireId> {
        match &self.selection {
            ScopeSelection::Global => None,
            ScopeSelection::Repertoire(id) => Some(id),
        }
    }

    #[cfg(test)]
    pub fn selected_side(&self) -> LearningSide {
        self.side
    }

    #[cfg(test)]
    pub fn wait_until_idle_for_test(&mut self) {
        while self.loading || self.snapshot_pending {
            let event = self
                .rx
                .recv_timeout(Duration::from_secs(5))
                .expect("learning dashboard background work timed out");
            self.tx.send(event).unwrap();
            self.poll();
        }
    }

    pub fn show(&mut self, ui: &mut Ui, settings: &Settings) -> Vec<LearningDashboardAction> {
        crate::san::set(settings.move_notation);
        self.poll();
        let mut actions = Vec::new();
        if !ui.ctx().egui_wants_keyboard_input()
            && !crate::app::overlay_open(ui.ctx())
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            actions.push(LearningDashboardAction::Close);
        }
        if self.loading || self.snapshot_pending {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.heading(tr("Learning dashboard"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(tr("Close")).clicked() {
                    actions.push(LearningDashboardAction::Close);
                }
            });
        });
        ui.add_space(4.0);
        self.scope_controls(ui);
        self.scope_line(ui);
        self.capture_line(ui);
        self.tabs(ui);
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("learning_dashboard_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.loading && self.data.is_none() {
                    loading_body(ui);
                    return;
                }
                if let Some(error) = self.error.clone() {
                    error_body(ui, &error);
                    if ui.button(tr("Retry")).clicked() {
                        self.reload();
                    }
                    return;
                }
                let Some(data) = self.data.as_ref() else {
                    loading_body(ui);
                    return;
                };
                let Some(view) = data
                    .views
                    .iter()
                    .find(|view| view.selection == self.selection)
                else {
                    self.error = Some(tr("The selected scope is no longer available").to_owned());
                    return;
                };
                match self.tab {
                    DashboardTab::Summary => show_summary(ui, data, view),
                    DashboardTab::Branches => crate::learning_branches::show(
                        ui,
                        &view.branches,
                        &data.names,
                        &data.labels,
                        &mut self.selected_branch,
                    ),
                    DashboardTab::History => show_history(ui, view),
                    DashboardTab::Scheduled => show_scheduled(ui, view),
                    DashboardTab::Sequences => show_sequences(ui, view),
                    DashboardTab::Moves => show_moves(ui, view, &mut self.selected_move),
                }
            });
        actions
    }

    fn reload(&mut self) {
        self.load(false);
    }

    fn refresh(&mut self) {
        self.load(true);
    }

    fn load(&mut self, preserve_data: bool) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        if !preserve_data {
            self.data = None;
        }
        let library = self.library.clone();
        let store = self.learning_store.clone();
        let side = self.side;
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("learning-dashboard", move || {
            let result = store
                .load()
                .map_err(|error| error.to_string())
                .and_then(|data| {
                    let as_of = now();
                    DashboardData::build(&library, &data, side, as_of, local_date_policy(as_of))
                });
            let _ = tx.send(Event::Loaded(generation, Box::new(result)));
        }) {
            self.loading = false;
            self.error = Some(error.to_string());
        }
    }

    fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Loaded(generation, result) if generation == self.generation => {
                    self.loading = false;
                    match *result {
                        Ok(data) => {
                            self.data = Some(data);
                            self.error = None;
                            self.ensure_snapshot_after_load();
                        }
                        Err(error) => {
                            self.data = None;
                            self.error = Some(error);
                        }
                    }
                }
                Event::Snapshot(generation, result) if generation == self.generation => {
                    self.snapshot_pending = false;
                    match *result {
                        Ok(saved) => {
                            self.snapshot_error = None;
                            self.snapshot_message = Some(if saved.recorded {
                                trf(
                                    "Snapshot captured: {date}",
                                    &[("date", &saved.snapshot.date)],
                                )
                            } else {
                                trf(
                                    "Snapshot already exists for this date and scope: {date}",
                                    &[("date", &saved.snapshot.date)],
                                )
                            });
                            if saved.recorded {
                                self.refresh();
                            }
                        }
                        Err(error) => {
                            self.snapshot_message = None;
                            self.snapshot_error = Some(error);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn ensure_snapshot_after_load(&mut self) {
        if self.snapshot_pending {
            return;
        }
        let Some((scope, timezone)) = self.selected_capture() else {
            return;
        };
        if self
            .automatic_snapshot_scope
            .as_ref()
            .is_some_and(|attempted| same_scope(attempted, &scope))
        {
            return;
        }
        self.automatic_snapshot_scope = Some(scope.clone());
        self.capture_snapshot(scope, timezone);
    }

    fn scope_controls(&mut self, ui: &mut Ui) {
        let previous_side = self.side;
        let previous_selection = self.selection.clone();
        let enabled = !self.loading && !self.snapshot_pending;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr("Scope")).weak());
                let selected_text = self.selection_label(&self.selection);
                egui::ComboBox::from_id_salt("learning_dashboard_scope")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.selection,
                            ScopeSelection::Global,
                            tr("All repertoires (union)"),
                        );
                        for repertoire in &self.library.repertoires {
                            ui.selectable_value(
                                &mut self.selection,
                                ScopeSelection::Repertoire(repertoire.id.clone()),
                                &repertoire.name,
                            );
                        }
                    });
                ui.label(RichText::new(tr("Side")).weak());
                egui::ComboBox::from_id_salt("learning_dashboard_side")
                    .selected_text(side_label(self.side))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.side, LearningSide::White, tr("White"));
                        ui.selectable_value(&mut self.side, LearningSide::Black, tr("Black"));
                    });
            });
        });
        if previous_selection != self.selection {
            self.selected_branch = None;
            self.snapshot_message = None;
            self.snapshot_error = None;
        }
        if previous_side != self.side {
            self.selected_move = None;
            self.selected_branch = None;
            self.snapshot_message = None;
            self.snapshot_error = None;
            self.reload();
        } else if previous_selection != self.selection {
            self.ensure_snapshot_after_load();
        }
    }

    fn scope_line(&self, ui: &mut Ui) {
        let (side, as_of) = self.data.as_ref().map_or_else(
            || (self.side, "…".to_owned()),
            |data| (data.side, format_time(Some(data.as_of))),
        );
        let text = format!(
            "{}: {} · {}: {} · {}: {}",
            tr("Scope"),
            self.selection_label(&self.selection),
            tr("Side"),
            side_label(side),
            tr("Calculated at"),
            as_of
        );
        ui.add(egui::Label::new(&text).truncate())
            .on_hover_text(text);
    }

    fn selected_capture(&self) -> Option<(StatsScope, LocalDatePolicy)> {
        let data = self.data.as_ref()?;
        let view = data
            .views
            .iter()
            .find(|view| view.selection == self.selection)?;
        Some((view.scope.clone(), data.timezone))
    }

    fn capture_line(&mut self, ui: &mut Ui) {
        let capture = self.selected_capture();
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(width, 30.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                let response = ui
                    .add_enabled(
                        capture.is_some() && !self.snapshot_pending && !self.loading,
                        egui::Button::new(tr("Save statistics to History")),
                    )
                    .on_hover_text(tr(
                        "Save the current numbers for the selected scope and side. PGN and learning progress do not change.",
                    ));
                if response.clicked()
                    && let Some((scope, timezone)) = capture
                {
                    self.capture_snapshot(scope, timezone);
                }
                if self.snapshot_pending {
                    ui.spinner();
                }
                let message = self
                    .snapshot_error
                    .as_ref()
                    .or(self.snapshot_message.as_ref());
                if let Some(message) = message {
                    let color = if self.snapshot_error.is_some() {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.add(egui::Label::new(RichText::new(message).color(color)).truncate())
                        .on_hover_text(message);
                }
            },
        );
    }

    fn capture_snapshot(&mut self, scope: StatsScope, timezone: LocalDatePolicy) {
        self.snapshot_pending = true;
        self.snapshot_message = None;
        self.snapshot_error = None;
        let generation = self.generation;
        let library = self.library.clone();
        let store = self.learning_store.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("learning-dashboard-snapshot", move || {
            let result = capture_snapshot(&store, &library, &scope, timezone);
            let _ = tx.send(Event::Snapshot(generation, Box::new(result)));
        }) {
            self.snapshot_pending = false;
            self.snapshot_error = Some(error.to_string());
        }
    }

    fn tabs(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (DashboardTab::Summary, tr("Summary")),
                (DashboardTab::Branches, tr("Branches")),
                (DashboardTab::History, tr("History")),
                (DashboardTab::Scheduled, tr("Scheduled")),
                (DashboardTab::Sequences, tr("Sequences")),
                (DashboardTab::Moves, tr("Moves")),
            ] {
                if ui.selectable_label(self.tab == tab, label).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    fn selection_label(&self, selection: &ScopeSelection) -> String {
        match selection {
            ScopeSelection::Global => tr("All repertoires (union)").to_owned(),
            ScopeSelection::Repertoire(id) => self
                .library
                .repertoire(id)
                .map(|repertoire| repertoire.name.clone())
                .unwrap_or_else(|| tr("The selected scope is no longer available").to_owned()),
        }
    }
}

impl DashboardData {
    fn build(
        library: &Library,
        data: &LearningStoreData,
        side: LearningSide,
        as_of: i64,
        timezone: LocalDatePolicy,
    ) -> Result<Self, String> {
        let ids = library
            .repertoires
            .iter()
            .map(|repertoire| repertoire.id.clone())
            .collect::<Vec<_>>();
        let labels = membership_labels(library);
        let names = library
            .repertoires
            .iter()
            .map(|repertoire| (repertoire.id.clone(), repertoire.name.clone()))
            .collect();
        let global_scope = StatsScope::new(ids, side, as_of);
        let global = DashboardView::build(
            library,
            data,
            ScopeSelection::Global,
            global_scope,
            timezone,
            &labels,
        )?;
        let global_summary = global.summary.clone();
        let mut views = vec![global];
        let mut summary = Vec::with_capacity(library.repertoires.len() + 1);
        for repertoire in &library.repertoires {
            let scope = StatsScope::for_repertoire(repertoire.id.clone(), side, as_of);
            let view = DashboardView::build(
                library,
                data,
                ScopeSelection::Repertoire(repertoire.id.clone()),
                scope,
                timezone,
                &labels,
            )?;
            summary.push(SummaryRow {
                id: Some(repertoire.id.clone()),
                name: repertoire.name.clone(),
                summary: view.summary.clone(),
            });
            views.push(view);
        }
        summary.push(SummaryRow {
            id: None,
            name: String::new(),
            summary: global_summary,
        });
        Ok(Self {
            side,
            as_of,
            timezone,
            summary,
            views,
            names,
            labels,
        })
    }
}

impl DashboardView {
    fn build(
        library: &Library,
        data: &LearningStoreData,
        selection: ScopeSelection,
        scope: StatsScope,
        timezone: LocalDatePolicy,
        labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    ) -> Result<Self, String> {
        let settings = SrsSettings::default();
        let scoped = aggregate_scope(library, &data.learning, &scope, &settings)
            .map_err(|error| error.to_string())?;
        let scheduled = scheduled_buckets(scoped.items.iter(), scope.as_of, timezone);
        let sequences = sequence_histograms(scoped.items.iter());
        let branches = branch_learning_analytics(library, &data.learning, &scope, &settings)
            .map_err(|error| error.to_string())?;
        let analytics = scoped_move_learning_analytics(library, &data.learning, &scope, &settings)
            .map_err(|error| error.to_string())?;
        let names = library
            .repertoires
            .iter()
            .map(|repertoire| (repertoire.id.clone(), repertoire.name.clone()))
            .collect::<HashMap<_, _>>();
        let moves = analytics
            .into_iter()
            .map(|analytics| {
                let mut wrong_moves = analytics
                    .wrong_move_histogram
                    .iter()
                    .map(|(uci, stats)| WrongMoveRow {
                        san: san_from_fen4(&analytics.key.position_fen4, uci)
                            .unwrap_or_else(|| "—".to_owned()),
                        count: stats.count,
                        last_at: stats.last_at,
                    })
                    .collect::<Vec<_>>();
                wrong_moves.sort_by(|left, right| {
                    right
                        .count
                        .cmp(&left.count)
                        .then_with(|| right.last_at.cmp(&left.last_at))
                        .then_with(|| left.san.cmp(&right.san))
                });
                let memberships = analytics
                    .memberships
                    .iter()
                    .map(|membership| MembershipRow {
                        repertoire: names
                            .get(&membership.repertoire_id)
                            .cloned()
                            .unwrap_or_else(|| "—".to_owned()),
                        branch: labels
                            .get(&(membership.repertoire_id.clone(), membership.path.clone()))
                            .cloned()
                            .unwrap_or_default(),
                    })
                    .collect();
                MoveRow {
                    san: san_from_fen4(&analytics.key.position_fen4, &analytics.key.expected_uci)
                        .unwrap_or_else(|| "—".to_owned()),
                    key: analytics.key,
                    status: analytics.status,
                    resistant: analytics.resistant,
                    accuracy_percent: analytics.accuracy_percent,
                    independent_attempts: analytics.independent_attempts,
                    correct_count: analytics.correct_count,
                    wrong_count: analytics.wrong_count,
                    correct_streak: analytics.correct_streak,
                    wrong_streak: analytics.wrong_streak,
                    next_review_at: analytics.next_review_at,
                    review_in_seconds: analytics.review_in_seconds,
                    overdue_seconds: analytics.overdue_seconds,
                    uncertain_grace_seconds: analytics.uncertain_grace_seconds,
                    response_time_median_ms: analytics.response_time_median_ms,
                    wrong_moves,
                    memberships,
                }
            })
            .collect();
        let history = data
            .snapshots
            .iter()
            .filter(|snapshot| same_scope(&snapshot.scope, &scope))
            .cloned()
            .collect();
        Ok(Self {
            selection,
            scope,
            summary: scoped.summary,
            branches,
            history,
            scheduled,
            sequences,
            moves,
        })
    }
}

fn capture_snapshot(
    store: &LearningStore,
    library: &Library,
    scope: &StatsScope,
    timezone: LocalDatePolicy,
) -> Result<PersistedLearningSnapshot, String> {
    store
        .record_snapshot(library, scope, &SrsSettings::default(), timezone)
        .map_err(|error| error.to_string())
}

fn same_scope(left: &StatsScope, right: &StatsScope) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.as_of = 0;
    right.as_of = 0;
    left == right
}

fn membership_labels(library: &Library) -> HashMap<(RepertoireId, Vec<String>), Vec<String>> {
    let mut result = HashMap::new();
    for repertoire in &library.repertoires {
        let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
        let game = tree.game();
        for node in game.attached_nodes() {
            let Some(path) = tree.path(node) else {
                continue;
            };
            let labels = game
                .path(node)
                .into_iter()
                .skip(1)
                .filter_map(|id| game.move_label(id))
                .collect::<Vec<_>>();
            result.insert((repertoire.id.clone(), path.to_vec()), labels);
        }
    }
    result
}

fn san_from_fen4(position_fen4: &str, uci: &str) -> Option<String> {
    let fen = format!("{position_fen4} 0 1").parse::<Fen>().ok()?;
    let position = fen.into_position::<Chess>(CastlingMode::Standard).ok()?;
    let mv = uci.parse::<UciMove>().ok()?.to_move(&position).ok()?;
    Some(San::from_move(&position, mv).to_string())
}

fn loading_body(ui: &mut Ui) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 160.0),
        Layout::top_down(Align::Min),
        |ui| {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Loading learning statistics…"));
            });
        },
    );
}

fn error_body(ui: &mut Ui, error: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 120.0),
        Layout::top_down(Align::Min),
        |ui| {
            ui.add_space(12.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        },
    );
}

fn show_summary(ui: &mut Ui, data: &DashboardData, view: &DashboardView) {
    let compact = ui.available_width() < 700.0;
    let number_width = 60.0;
    let headers = [
        "Name",
        "Total",
        "Unseen",
        "Learning",
        "Learnt",
        "Uncertain",
        "Due",
        "Learnt %",
        "Confident %",
    ];
    let mut table = TableBuilder::new(ui)
        .id_salt("learning_dashboard_summary")
        .striped(true)
        .vscroll(false)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(80.0).clip(true));
    for _ in 1..headers.len() {
        table = table.column(Column::exact(number_width).clip(true));
    }
    table
        .header(if compact { 40.0 } else { 30.0 }, |mut header| {
            for (index, key) in headers.into_iter().enumerate() {
                header.col(|ui| {
                    let response = if index == 0 {
                        header_cell(ui, tr(key), false)
                    } else {
                        summary_metric_header(ui, tr(key), compact)
                    };
                    response.on_hover_text(match key {
                        "Total" => tr("Total = Unseen + Learning + Learnt."),
                        "Uncertain" => {
                            tr("Learnt = Confident + Uncertain · percentages use Total.")
                        }
                        _ => tr(key),
                    });
                });
            }
        })
        .body(|body| {
            body.rows(24.0, data.summary.len(), |mut row| {
                let summary_row = &data.summary[row.index()];
                let summary = &summary_row.summary;
                let strong = summary_row.id.is_none();
                row.col(|ui| {
                    let name = if strong {
                        tr("Global total")
                    } else {
                        &summary_row.name
                    };
                    let response = text_cell(ui, name, strong, false);
                    if strong {
                        response.on_hover_text(tr(
                            "Global total counts each shared learning item once; repertoire rows can add up to more.",
                        ));
                    } else {
                        response.on_hover_text(&summary_row.name);
                    }
                });
                let values = [
                    summary.total.to_string(),
                    summary.unseen.to_string(),
                    summary.learning.to_string(),
                    summary.learnt.to_string(),
                    summary.uncertain.to_string(),
                    summary.due.to_string(),
                    format_percent(summary.learnt_percent),
                    format_percent(summary.confident_percent),
                ];
                for value in values {
                    row.col(|ui| {
                        text_cell(ui, &value, strong, true);
                    });
                }
            });
        });
    ui.add_space(6.0);
    ui.label(format!(
        "{}: {}",
        tr("Next due at"),
        format_time(view.summary.next_due_at)
    ));
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

fn show_history(ui: &mut Ui, view: &DashboardView) {
    if view.history.is_empty() {
        ui.label(tr(
            "No snapshots yet. Save statistics to History to record the current scope.",
        ));
        ui.add(
            egui::Label::new(
                RichText::new(tr(
                    "Save the current numbers for the selected scope and side. PGN and learning progress do not change.",
                ))
                .weak(),
            )
            .wrap(),
        );
        return;
    }
    crate::learning_charts::history(ui, &view.history);
    ui.add_space(8.0);
    let number_width = 60.0;
    let headers = [
        "Date",
        "Total",
        "Unseen",
        "Learning",
        "Learnt",
        "Uncertain",
        "Due",
    ];
    let mut table = TableBuilder::new(ui)
        .id_salt("learning_dashboard_history")
        .striped(true)
        .max_scroll_height(360.0)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(90.0).clip(true));
    for _ in 1..headers.len() {
        table = table.column(Column::exact(number_width));
    }
    table
        .header(28.0, |mut header| {
            for (index, key) in headers.into_iter().enumerate() {
                header.col(|ui| {
                    header_cell(ui, tr(key), index > 0);
                });
            }
        })
        .body(|body| {
            body.rows(24.0, view.history.len(), |mut row| {
                let snapshot = &view.history[row.index()];
                row.col(|ui| {
                    text_cell(ui, &snapshot.date, false, false).on_hover_text(format!(
                        "{}: {}",
                        tr("Calculated at"),
                        format_time(Some(snapshot.as_of))
                    ));
                });
                let values = [
                    snapshot.total,
                    snapshot.unseen,
                    snapshot.learning,
                    snapshot.learnt,
                    snapshot.uncertain,
                    snapshot.due,
                ];
                for value in values {
                    row.col(|ui| {
                        text_cell(ui, &value.to_string(), false, true);
                    });
                }
            });
        });
}

fn show_scheduled(ui: &mut Ui, view: &DashboardView) {
    let buckets = view.scheduled;
    crate::learning_charts::scheduled(ui, buckets);
    ui.add_space(8.0);
    let rows = [
        (tr("Overdue").to_owned(), buckets.overdue),
        (tr("Today").to_owned(), buckets.today),
        (tr("Next 7 days").to_owned(), buckets.week),
        (tr("Next 30 days").to_owned(), buckets.month),
        (tr("Next 365 days").to_owned(), buckets.year),
        (tr("Later").to_owned(), buckets.later),
    ];
    distribution_table(
        ui,
        "learning_dashboard_scheduled",
        "Scheduled",
        "Scheduled",
        &rows,
        buckets.introduced(),
    )
    .on_hover_text(tr(
        "Today ends at local midnight; the other buckets use fixed 7, 30 and 365-day ranges.",
    ));
    ui.label(format!("{}: {}", tr("Introduced"), buckets.introduced()));
}

fn show_sequences(ui: &mut Ui, view: &DashboardView) {
    if view.moves.is_empty() {
        ui.label(tr("No moves in this scope"));
        return;
    }
    crate::learning_charts::sequences(ui, &view.sequences);
    let correct = histogram_rows(&view.sequences.correct_streak);
    let wrong = histogram_rows(&view.sequences.wrong_streak);
    let attempts = histogram_rows(&view.sequences.attempts);
    let total = view.moves.len();
    if ui.available_width() >= 900.0 {
        ui.columns(3, |columns| {
            distribution_table(
                &mut columns[0],
                "learning_dashboard_correct_sequences",
                "Correct streak",
                "Length / attempts",
                &correct,
                total,
            );
            distribution_table(
                &mut columns[1],
                "learning_dashboard_wrong_sequences",
                "Wrong streak",
                "Length / attempts",
                &wrong,
                total,
            );
            distribution_table(
                &mut columns[2],
                "learning_dashboard_attempt_sequences",
                "Attempts",
                "Length / attempts",
                &attempts,
                total,
            );
        });
    } else {
        distribution_table(
            ui,
            "learning_dashboard_correct_sequences",
            "Correct streak",
            "Length / attempts",
            &correct,
            total,
        );
        distribution_table(
            ui,
            "learning_dashboard_wrong_sequences",
            "Wrong streak",
            "Length / attempts",
            &wrong,
            total,
        );
        distribution_table(
            ui,
            "learning_dashboard_attempt_sequences",
            "Attempts",
            "Length / attempts",
            &attempts,
            total,
        );
    }
}

fn histogram_rows<K: Ord + ToString>(
    histogram: &std::collections::BTreeMap<K, usize>,
) -> Vec<(String, usize)> {
    histogram
        .iter()
        .map(|(value, count)| (value.to_string(), *count))
        .collect()
}

fn distribution_table(
    ui: &mut Ui,
    id: &'static str,
    title: &'static str,
    first_header: &'static str,
    rows: &[(String, usize)],
    total: usize,
) -> egui::Response {
    let response = ui.strong(tr(title));
    TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .vscroll(false)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(100.0).clip(true))
        .column(Column::exact(70.0))
        .header(28.0, |mut header| {
            header.col(|ui| {
                header_cell(ui, tr(first_header), false);
            });
            header.col(|ui| {
                header_cell(ui, tr("Items"), true);
            });
        })
        .body(|body| {
            body.rows(24.0, rows.len() + 1, |mut row| {
                if row.index() == rows.len() {
                    row.col(|ui| {
                        text_cell(ui, tr("Total"), true, false);
                    });
                    row.col(|ui| {
                        text_cell(ui, &total.to_string(), true, true);
                    });
                    return;
                }
                let (value, count) = &rows[row.index()];
                row.col(|ui| {
                    text_cell(ui, value, false, false);
                });
                row.col(|ui| {
                    text_cell(ui, &count.to_string(), false, true);
                });
            });
        });
    ui.add_space(10.0);
    response
}

fn show_moves(ui: &mut Ui, view: &DashboardView, selected: &mut Option<LearningItemKey>) {
    if view.moves.is_empty() {
        ui.label(tr("No moves in this scope"));
        *selected = None;
        return;
    }
    if selected
        .as_ref()
        .is_none_or(|key| !view.moves.iter().any(|row| &row.key == key))
    {
        *selected = Some(view.moves[0].key.clone());
    }
    let mut clicked = None;
    let compact = ui.available_width() < 700.0;
    TableBuilder::new(ui)
        .id_salt("learning_dashboard_moves")
        .striped(true)
        .sense(egui::Sense::click())
        .max_scroll_height(if compact { 180.0 } else { 300.0 })
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(72.0).clip(true))
        .column(Column::exact(if compact { 68.0 } else { 100.0 }))
        .column(Column::exact(if compact { 58.0 } else { 76.0 }))
        .column(Column::exact(if compact { 58.0 } else { 76.0 }))
        .column(Column::exact(if compact { 108.0 } else { 142.0 }))
        .header(30.0, |mut header| {
            for (index, key) in ["Move", "Status", "Accuracy", "Streak", "Next review"]
                .into_iter()
                .enumerate()
            {
                header.col(|ui| {
                    header_cell(ui, tr(key), index > 0).on_hover_text(if key == "Streak" {
                        format!("{} / {}", tr("Correct streak"), tr("Wrong streak"))
                    } else {
                        tr(key).to_owned()
                    });
                });
            }
        })
        .body(|body| {
            body.rows(24.0, view.moves.len(), |mut row| {
                let move_row = &view.moves[row.index()];
                row.set_selected(selected.as_ref() == Some(&move_row.key));
                row.col(|ui| {
                    text_cell(ui, &crate::san::show(&move_row.san), false, false);
                });
                row.col(|ui| {
                    text_cell(
                        ui,
                        &move_status_label(move_row.status, move_row.resistant),
                        false,
                        false,
                    );
                });
                row.col(|ui| {
                    text_cell(ui, &format_percent(move_row.accuracy_percent), false, true);
                });
                row.col(|ui| {
                    text_cell(
                        ui,
                        &format!("{} / {}", move_row.correct_streak, move_row.wrong_streak),
                        false,
                        true,
                    )
                    .on_hover_text(format!(
                        "{} / {}",
                        tr("Correct streak"),
                        tr("Wrong streak")
                    ));
                });
                row.col(|ui| {
                    text_cell(ui, &format_time(move_row.next_review_at), false, true);
                });
                if row.response().clicked() {
                    clicked = Some(move_row.key.clone());
                }
            });
        });
    if let Some(key) = clicked {
        *selected = Some(key);
    }
    let Some(selected) = selected.as_ref() else {
        return;
    };
    let Some(move_row) = view.moves.iter().find(|row| &row.key == selected) else {
        return;
    };
    ui.separator();
    show_move_details(ui, move_row);
}

fn show_move_details(ui: &mut Ui, move_row: &MoveRow) {
    ui.strong(crate::san::show(&move_row.san));
    let narrow = ui.available_width() < 700.0;
    let details = [
        (
            "Status",
            move_status_label(move_row.status, move_row.resistant),
            "The current learning status of this move.",
        ),
        (
            "Accuracy",
            format_percent(move_row.accuracy_percent),
            "Independent correct answers divided by all independent attempts.",
        ),
        (
            "Attempts",
            format!(
                "{} · {} {} · {} {}",
                move_row.independent_attempts,
                move_row.correct_count,
                tr("Correct"),
                move_row.wrong_count,
                tr("Wrong")
            ),
            "Only independent attempts count; previews, automatic context and replays are excluded.",
        ),
        (
            "Correct streak",
            move_row.correct_streak.to_string(),
            "Consecutive independent correct answers.",
        ),
        (
            "Wrong streak",
            move_row.wrong_streak.to_string(),
            "Consecutive independent wrong answers.",
        ),
        (
            "Next review",
            format_time(move_row.next_review_at),
            "The scheduled review time, or a dash for an unseen move.",
        ),
        (
            "Review in",
            format_duration(move_row.review_in_seconds),
            "Time remaining until the scheduled review.",
        ),
        (
            "Overdue by",
            format_duration(move_row.overdue_seconds),
            "Elapsed time since the scheduled review.",
        ),
        (
            "Median response",
            format_median(move_row.response_time_median_ms),
            "Median response time over independent attempts.",
        ),
        (
            "Uncertain grace",
            format_duration(move_row.uncertain_grace_seconds),
            "The overdue grace period before a learnt move becomes uncertain.",
        ),
    ];
    egui::Grid::new("learning_dashboard_move_details")
        .num_columns(if narrow { 2 } else { 4 })
        .spacing([12.0, 5.0])
        .show(ui, |ui| {
            for (index, (label, value, tooltip)) in details.into_iter().enumerate() {
                ui.label(RichText::new(tr(label)).weak())
                    .on_hover_text(tr(tooltip));
                ui.label(value).on_hover_text(tr(tooltip));
                if narrow || index % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(8.0);
    ui.strong(tr("Wrong answers"));
    if move_row.wrong_moves.is_empty() {
        ui.label(tr("No wrong answers"));
    } else {
        TableBuilder::new(ui)
            .id_salt("learning_dashboard_wrong_moves")
            .striped(true)
            .vscroll(false)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(90.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::exact(150.0))
            .header(28.0, |mut header| {
                header.col(|ui| {
                    header_cell(ui, tr("Move"), false);
                });
                header.col(|ui| {
                    header_cell(ui, tr("Count"), true);
                });
                header.col(|ui| {
                    header_cell(ui, tr("Last at"), true);
                });
            })
            .body(|body| {
                body.rows(24.0, move_row.wrong_moves.len(), |mut row| {
                    let wrong = &move_row.wrong_moves[row.index()];
                    row.col(|ui| {
                        text_cell(ui, &crate::san::show(&wrong.san), false, false);
                    });
                    row.col(|ui| {
                        text_cell(ui, &wrong.count.to_string(), false, true);
                    });
                    row.col(|ui| {
                        text_cell(ui, &format_time(Some(wrong.last_at)), false, true);
                    });
                });
            });
    }
    ui.add_space(8.0);
    ui.strong(tr("Found in"));
    for membership in &move_row.memberships {
        let branch = membership
            .branch
            .iter()
            .map(|label| crate::san::show(label))
            .collect::<Vec<_>>()
            .join(" ");
        let text = if branch.is_empty() {
            membership.repertoire.clone()
        } else {
            format!("{} · {branch}", membership.repertoire)
        };
        ui.add(egui::Label::new(&text).truncate())
            .on_hover_text(tr(
                "Repertoires and branches that contain this learning item.",
            ))
            .on_hover_text(text);
    }
}

fn move_status_label(status: MoveLearningStatus, resistant: bool) -> String {
    let status = match status {
        MoveLearningStatus::Unseen => tr("Unseen"),
        MoveLearningStatus::Learning => tr("Learning"),
        MoveLearningStatus::LearntFresh => tr("Learnt fresh"),
        MoveLearningStatus::Uncertain => tr("Uncertain"),
    };
    if resistant {
        format!("{status} · {}", tr("Resistant"))
    } else {
        status.to_owned()
    }
}

fn header_cell(ui: &mut Ui, text: &str, numeric: bool) -> egui::Response {
    if numeric {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(text).strong()).truncate())
        })
        .inner
    } else {
        ui.add(egui::Label::new(RichText::new(text).strong()).truncate())
    }
}

fn summary_metric_header(ui: &mut Ui, text: &str, compact: bool) -> egui::Response {
    let text = RichText::new(text)
        .strong()
        .size(if compact { 12.0 } else { 14.0 });
    ui.add(egui::Label::new(text).wrap())
}

fn text_cell(ui: &mut Ui, text: &str, strong: bool, numeric: bool) -> egui::Response {
    let text = if strong {
        RichText::new(text).strong()
    } else {
        RichText::new(text)
    };
    if numeric {
        let size = egui::vec2(ui.available_width(), ui.available_height());
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            ui.add(egui::Label::new(text).truncate().halign(Align::Max))
        })
        .inner
    } else {
        ui.add(egui::Label::new(text).truncate())
    }
}

fn side_label(side: LearningSide) -> &'static str {
    tr(match side {
        LearningSide::White => "White",
        LearningSide::Black => "Black",
    })
}

fn format_percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0} %"))
}

fn format_median(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0} ms"))
}

fn format_duration(value: Option<i64>) -> String {
    let Some(seconds) = value else {
        return "—".to_owned();
    };
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else if minutes > 0 {
        format!("{minutes} min {seconds} s")
    } else {
        format!("{seconds} s")
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

fn local_date_policy(as_of: i64) -> LocalDatePolicy {
    let offset = chrono::DateTime::from_timestamp(as_of, 0)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .offset()
                .local_minus_utc()
        })
        .unwrap_or(0);
    LocalDatePolicy::fixed(offset)
}

fn now() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use trainer::repertoire::{LearningData, LearningItem, WrongMoveStats, active_learning_keys};

    use super::*;

    const BASE: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;
    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "cp-learning-dashboard-{}-{sequence}-{name}",
                std::process::id()
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

    fn add_repertoire(library: &mut Library, name: &str, pgn: &str) -> RepertoireId {
        let game = chess_core::parse_str(pgn).game;
        library.create(name, &game).unwrap()
    }

    fn key(library: &Library, id: &RepertoireId, expected: &str) -> LearningItemKey {
        active_learning_keys(
            library,
            &StatsScope::for_repertoire(id.clone(), LearningSide::White, BASE),
        )
        .unwrap()
        .into_iter()
        .find(|key| key.expected_uci == expected)
        .unwrap()
    }

    fn introduced(key: LearningItemKey, streak: u32, next: i64) -> LearningItem {
        let mut item = LearningItem::new(key);
        item.introduced = true;
        item.correct_streak = streak;
        item.correct_count = u64::from(streak);
        item.next_review_at = Some(next);
        item.previous_interval_seconds = Some(DAY);
        item
    }

    fn build(library: &Library, data: &LearningStoreData) -> Result<DashboardData, String> {
        DashboardData::build(
            library,
            data,
            LearningSide::White,
            BASE,
            LocalDatePolicy::UTC,
        )
    }

    fn loaded_dashboard(
        library: Library,
        learning_store: LearningStore,
        selection: ScopeSelection,
        data: DashboardData,
    ) -> LearningDashboardUi {
        let automatic_snapshot_scope = data
            .views
            .iter()
            .find(|view| view.selection == selection)
            .map(|view| view.scope.clone());
        let (tx, rx) = mpsc::channel();
        LearningDashboardUi {
            library,
            learning_store,
            selection,
            side: data.side,
            tab: DashboardTab::Summary,
            data: Some(data),
            loading: false,
            error: None,
            snapshot_pending: false,
            snapshot_message: None,
            snapshot_error: None,
            automatic_snapshot_scope,
            selected_move: None,
            selected_branch: None,
            generation: 0,
            tx,
            rx,
        }
    }

    fn render_dashboard(
        dashboard: &mut LearningDashboardUi,
        ctx: &egui::Context,
        settings: &Settings,
        width: f32,
        height: f32,
    ) -> (egui::Rect, egui::FullOutput) {
        let mut content = None;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            },
            |ui| {
                let available = ui.available_rect_before_wrap();
                assert!(dashboard.show(ui, settings).is_empty());
                let rect = ui.min_rect();
                assert!(rect.min.x >= available.min.x, "{rect:?} {available:?}");
                assert!(rect.min.y >= available.min.y, "{rect:?} {available:?}");
                assert!(
                    rect.max.x <= available.max.x + 0.1,
                    "{rect:?} {available:?}"
                );
                assert!(
                    rect.max.y <= available.max.y + 0.1,
                    "{rect:?} {available:?}"
                );
                content = Some(rect);
            },
        );
        (content.unwrap(), output)
    }

    fn assert_narrow_summary(shapes: &[egui::epaint::ClippedShape]) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0));
        let mut confident_height = 0.0;
        for expected in [
            "Name",
            "Total",
            "Unseen",
            "Learning",
            "Learnt",
            "Uncertain",
            "Due",
            "Learnt %",
            "Confident %",
        ] {
            let (rect, clip) = shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text)
                        if text.galley.text().replace('\n', " ") == expected =>
                    {
                        Some((
                            egui::Rect::from_min_size(text.pos, text.galley.size()),
                            clipped.clip_rect,
                        ))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing full summary header {expected}"));
            assert!(
                rect.min.x >= clip.min.x - 0.1 && rect.max.x <= clip.max.x + 0.1,
                "clipped summary header {expected}: {rect:?} {clip:?}"
            );
            if expected != "Name" {
                assert!(
                    rect.width() <= 60.1,
                    "numeric header {expected} exceeds 60 px: {rect:?}"
                );
            }
            if expected == "Confident %" {
                confident_height = rect.height();
            }
        }
        assert!(
            confident_height > 20.0,
            "Confident % should wrap onto two lines"
        );
        for clipped in shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                assert!(
                    rect.min.x >= screen.min.x
                        && rect.min.y >= screen.min.y
                        && rect.max.x <= screen.max.x + 0.1
                        && rect.max.y <= screen.max.y + 0.1,
                    "text outside 640x480: {rect:?} {}",
                    text.galley.text()
                );
            }
        }
    }

    #[test]
    fn mixed_summary_and_global_union_are_deduplicated() {
        let mut library = Library::default();
        let first = add_repertoire(&mut library, "Open", "1. e4 e5 2. Nf3 Nc6 *");
        let second = add_repertoire(&mut library, "Vienna", "1. e4 e5 2. Nc3 Nf6 *");
        let shared = key(&library, &first, "e2e4");
        assert_eq!(shared, key(&library, &second, "e2e4"));
        let mut learning = LearningData::default();
        let mut shared_item = introduced(shared, 3, BASE - 10 * DAY);
        shared_item.wrong_move_histogram = BTreeMap::from([(
            "d2d4".to_owned(),
            WrongMoveStats {
                count: 2,
                last_at: BASE - DAY,
            },
        )]);
        shared_item.independent_response_times_ms = vec![100, 300, 200];
        learning.items.push(shared_item);
        learning
            .items
            .push(introduced(key(&library, &first, "g1f3"), 1, BASE + DAY));
        learning
            .items
            .push(LearningItem::new(key(&library, &second, "b1c3")));
        let data = LearningStoreData {
            learning,
            ..LearningStoreData::default()
        };
        let dashboard = build(&library, &data).unwrap();
        let first_summary = &dashboard.summary[0].summary;
        let second_summary = &dashboard.summary[1].summary;
        let global = &dashboard.summary[2].summary;
        assert_eq!((first_summary.total, first_summary.unseen), (2, 0));
        assert_eq!((first_summary.learning, first_summary.learnt), (1, 1));
        assert_eq!((first_summary.uncertain, first_summary.due), (1, 1));
        assert_eq!((second_summary.total, second_summary.unseen), (2, 1));
        assert_eq!((global.total, global.unseen), (3, 1));
        assert_eq!((global.learning, global.learnt), (1, 1));
        assert_eq!(
            global.total,
            global.unseen + global.learning + global.learnt
        );
        assert!(global.uncertain <= global.learnt);
        assert!(global.total <= first_summary.total + second_summary.total);
        assert!(global.total < first_summary.total + second_summary.total);

        let global_view = &dashboard.views[0];
        assert_eq!(global_view.moves.len(), 3);
        assert!(global_view.moves.iter().all(|row| row.san != "—"));
        assert!(global_view.moves.iter().any(|row| row.san == "e4"));
        assert!(global_view.sequences.attempts.contains_key(&0));
        for total in [
            global_view.sequences.correct_streak.values().sum::<usize>(),
            global_view.sequences.wrong_streak.values().sum::<usize>(),
            global_view.sequences.attempts.values().sum::<usize>(),
        ] {
            assert_eq!(total, global.total);
        }
        let e4 = global_view
            .moves
            .iter()
            .find(|row| row.san == "e4")
            .unwrap();
        assert_eq!(e4.wrong_moves[0].san, "d4");
        assert_eq!(e4.response_time_median_ms, Some(200.0));
        assert_eq!(e4.memberships.len(), 2);
        assert!(
            e4.memberships
                .iter()
                .all(|membership| membership.branch.iter().any(|label| label.contains("e4")))
        );
    }

    #[test]
    fn empty_scope_has_no_false_percentages_or_buckets() {
        let dashboard = build(&Library::default(), &LearningStoreData::default()).unwrap();
        assert_eq!(dashboard.summary.len(), 1);
        let global = &dashboard.summary[0].summary;
        assert_eq!(global.total, 0);
        assert_eq!(global.learnt_percent, None);
        assert_eq!(global.confident_percent, None);
        assert_eq!(format_percent(global.learnt_percent), "—");
        let view = &dashboard.views[0];
        assert!(view.moves.is_empty());
        assert!(view.history.is_empty());
        assert_eq!(view.scheduled.introduced(), 0);
        assert!(view.sequences.correct_streak.is_empty());
        assert!(view.sequences.wrong_streak.is_empty());
        assert!(view.sequences.attempts.is_empty());
    }

    #[test]
    fn scheduled_buckets_partition_introduced_items() {
        let mut library = Library::default();
        let id = add_repertoire(
            &mut library,
            "Spanish",
            "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 *",
        );
        let schedules = [
            ("e2e4", Some(BASE - 1)),
            ("g1f3", Some(BASE + 60)),
            ("f1b5", Some(BASE + 2 * DAY)),
            ("b5a4", Some(BASE + 10 * DAY)),
            ("e1g1", Some(BASE + 60 * DAY)),
            ("f1e1", Some(BASE + 400 * DAY)),
        ];
        let mut learning = LearningData::default();
        for (expected, next) in schedules {
            let mut item = LearningItem::new(key(&library, &id, expected));
            item.introduced = true;
            item.next_review_at = next;
            learning.items.push(item);
        }
        learning
            .items
            .push(LearningItem::new(key(&library, &id, "a4b3")));
        let dashboard = build(
            &library,
            &LearningStoreData {
                learning,
                ..LearningStoreData::default()
            },
        )
        .unwrap();
        let view = &dashboard.views[1];
        assert_eq!(view.summary.total, 7);
        assert_eq!(view.summary.unseen, 1);
        assert_eq!(
            view.scheduled,
            ScheduledBuckets {
                overdue: 1,
                today: 1,
                week: 1,
                month: 1,
                year: 1,
                later: 1,
            }
        );
        assert_eq!(
            view.scheduled.introduced(),
            view.summary.total - view.summary.unseen
        );
    }

    #[test]
    fn snapshot_capture_deduplicates_same_date_and_scope() {
        let dir = TestDir::new("duplicate");
        let store = dir.store();
        let mut library = Library::default();
        let id = add_repertoire(&mut library, "Open", "1. e4 e5 *");
        let scope = StatsScope::for_repertoire(id, LearningSide::White, BASE);
        let first = capture_snapshot(&store, &library, &scope, LocalDatePolicy::UTC).unwrap();
        assert!(first.recorded);
        let mut repeated_scope = scope.clone();
        repeated_scope.as_of += 60;
        let second =
            capture_snapshot(&store, &library, &repeated_scope, LocalDatePolicy::UTC).unwrap();
        assert!(!second.recorded);
        assert_eq!(first.snapshot.date, second.snapshot.date);
        assert_eq!(store.load().unwrap().snapshots.len(), 1);
    }

    #[test]
    fn history_preserves_old_snapshots_after_new_capture() {
        let dir = TestDir::new("history");
        let store = dir.store();
        let mut library = Library::default();
        let id = add_repertoire(&mut library, "Open", "1. e4 e5 *");
        let scope = StatsScope::for_repertoire(id, LearningSide::White, BASE);
        let old = capture_snapshot(&store, &library, &scope, LocalDatePolicy::UTC).unwrap();
        let mut next_scope = scope.clone();
        next_scope.as_of += DAY;
        let new = capture_snapshot(&store, &library, &next_scope, LocalDatePolicy::UTC).unwrap();
        assert!(new.recorded);
        let stored = store.load().unwrap();
        assert_eq!(stored.snapshots.len(), 2);
        let dashboard = build(&library, &stored).unwrap();
        let history = &dashboard.views[1].history;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].date, old.snapshot.date);
        assert_eq!(history[0].as_of, old.snapshot.as_of);
        assert_eq!(history[1].date, new.snapshot.date);
        assert!(history[0].as_of < history[1].as_of);
    }

    #[test]
    fn automatic_snapshot_runs_once_per_scope_and_reopening_does_not_write() {
        let dir = TestDir::new("automatic");
        let store = dir.store();
        let mut library = Library::default();
        let id = add_repertoire(&mut library, "Open", "1. e4 e5 *");
        add_repertoire(&mut library, "Queen's pawn", "1. d4 d5 *");
        let mut dashboard = LearningDashboardUi::new_for_repertoire(
            library.clone(),
            store.clone(),
            id.clone(),
            LearningSide::Black,
        );
        dashboard.wait_until_idle_for_test();

        let stored = store.load().unwrap();
        assert_eq!(stored.snapshots.len(), 1);
        assert_eq!(stored.snapshots[0].scope.side, LearningSide::Black);
        assert_eq!(stored.snapshots[0].scope.repertoire_ids.len(), 1);
        assert!(stored.snapshots[0].scope.repertoire_ids.contains(&id));
        let before_reopen = std::fs::read(store.path()).unwrap();
        let mut reopened = LearningDashboardUi::new_for_repertoire(
            library,
            store.clone(),
            id.clone(),
            LearningSide::Black,
        );
        reopened.wait_until_idle_for_test();
        assert_eq!(std::fs::read(store.path()).unwrap(), before_reopen);

        reopened.selection = ScopeSelection::Global;
        reopened.ensure_snapshot_after_load();
        reopened.wait_until_idle_for_test();
        let stored = store.load().unwrap();
        assert_eq!(stored.snapshots.len(), 2);
        assert!(
            stored
                .snapshots
                .iter()
                .any(|snapshot| snapshot.scope.repertoire_ids.len() == 2)
        );
        let before_existing = std::fs::read(store.path()).unwrap();
        reopened.selection = ScopeSelection::Repertoire(id);
        reopened.ensure_snapshot_after_load();
        reopened.wait_until_idle_for_test();
        assert_eq!(std::fs::read(store.path()).unwrap(), before_existing);
    }

    #[test]
    fn snapshot_refresh_preserves_the_visible_summary_and_its_height() {
        let dir = TestDir::new("refresh");
        let store = dir.store();
        let mut library = Library::default();
        let id = add_repertoire(&mut library, "Open", "1. e4 e5 2. Nf3 Nc6 *");
        let data = build(&library, &LearningStoreData::default()).unwrap();
        let scope = data.views[1].scope.clone();
        let mut dashboard = loaded_dashboard(
            library.clone(),
            store.clone(),
            ScopeSelection::Repertoire(id),
            data,
        );
        let settings = Settings::default();
        let ctx = egui::Context::default();
        let (before, output) = render_dashboard(&mut dashboard, &ctx, &settings, 1000.0, 700.0);
        output.drop_without_applying_deltas();
        let saved = capture_snapshot(&store, &library, &scope, LocalDatePolicy::UTC).unwrap();
        assert!(saved.recorded);

        dashboard
            .tx
            .send(Event::Snapshot(dashboard.generation, Box::new(Ok(saved))))
            .unwrap();
        dashboard.poll();
        assert!(dashboard.loading);
        assert!(dashboard.data.is_some());
        let (after, output) = render_dashboard(&mut dashboard, &ctx, &settings, 1000.0, 700.0);

        assert_eq!(after, before);
        assert!(output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "Global total")
        }));
        assert!(!output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "Loading learning statistics…")
        }));
        output.drop_without_applying_deltas();
        dashboard.wait_until_idle_for_test();
        assert_eq!(store.load().unwrap().snapshots.len(), 1);
    }

    #[test]
    fn every_tab_renders_at_both_window_sizes() {
        let dir = TestDir::new("layout");
        let mut library = Library::default();
        let id = add_repertoire(&mut library, "Open", "1. e4 e5 2. Nf3 Nc6 *");
        let mut learning = LearningData::default();
        let mut item = introduced(key(&library, &id, "e2e4"), 3, BASE + DAY);
        item.wrong_move_histogram = BTreeMap::from([(
            "d2d4".to_owned(),
            WrongMoveStats {
                count: 1,
                last_at: BASE - DAY,
            },
        )]);
        learning.items.push(item);
        let scope = StatsScope::for_repertoire(id.clone(), LearningSide::White, BASE);
        let snapshot = LearningSnapshot::capture(
            &library,
            &learning,
            &scope,
            &SrsSettings::default(),
            LocalDatePolicy::UTC,
        )
        .unwrap();
        let stored = LearningStoreData {
            learning,
            snapshots: vec![snapshot],
            ..LearningStoreData::default()
        };
        let data = build(&library, &stored).unwrap();
        let mut dashboard =
            loaded_dashboard(library, dir.store(), ScopeSelection::Repertoire(id), data);
        let settings = Settings::default();
        let ctx = egui::Context::default();
        for (width, height) in [(1400.0, 900.0), (640.0, 480.0)] {
            for tab in [
                DashboardTab::Summary,
                DashboardTab::Branches,
                DashboardTab::History,
                DashboardTab::Scheduled,
                DashboardTab::Sequences,
                DashboardTab::Moves,
            ] {
                dashboard.tab = tab;
                let (_, output) = render_dashboard(&mut dashboard, &ctx, &settings, width, height);
                let shapes = output.shapes.clone();
                output.drop_without_applying_deltas();
                if width == 640.0 && tab == DashboardTab::Summary {
                    assert_narrow_summary(&shapes);
                }
            }
        }
    }

    #[test]
    fn dashboard_stays_full_when_training_depth_is_limited() {
        let mut library = Library::default();
        let id = add_repertoire(&mut library, "Open", "1. e4 e5 2. Nf3 Nc6 *");
        library.repertoire_mut(&id).unwrap().set_max_plies(2);
        let dashboard = build(&library, &LearningStoreData::default()).unwrap();
        let view = &dashboard.views[1];
        assert_eq!(view.scope.max_depth, None);
        assert_eq!(view.summary.total, 2);
        assert!(!view.branches.is_empty());

        let training_scope =
            StatsScope::for_repertoire(id, LearningSide::White, BASE).with_max_depth(Some(2));
        let training = aggregate_scope(
            &library,
            &LearningData::default(),
            &training_scope,
            &SrsSettings::default(),
        )
        .unwrap();
        assert_eq!(training.summary.total, 1);
    }
}
