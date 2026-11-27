//! Opening training (docs/design/opening-trainer-v2-spec.md): the «Repertoires» screen
//! with its card and dialogs, and the way to the «Session» and «Lines» screens.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::Duration;

use chess_core::shakmaty::{Chess, Color, Position, Square};
use chess_core::{Game, NodeId, PgnDatabase, Severity};
use egui::{Color32, Key, Modifiers, RichText, Ui, vec2};
use trainer::repertoire::{
    LearningData, LearningItemKey, LearningSide, LearningStore, LearningStoreData, Library,
    LibraryStore, MergeReport, RepertoireId, ResetImpactProjection, SrsSettings, StatsScope,
    active_learning_keys, aggregate_scope, fen4, reset_impact,
};
use web_time::Instant;

use crate::board::BoardState;
use crate::i18n::{tr, trf};
use crate::opening_layout::{self as layout, BoardAnnotations, GAP, NEW_PER_SESSION};
use crate::opening_lines::{LinesAction, OpeningLinesUi};
use crate::opening_session::{
    OpeningSessionUi, PreparingView, SessionAction, SessionKind, show_preparing,
};
use crate::settings::Settings;

pub enum RepertoireAction {
    Close,
    OpenSettings,
    Sound(crate::assets::Sound),
    #[allow(dead_code)]
    Toast(String),
    SettingsChanged,
    Added {
        id: RepertoireId,
        name: String,
        report: MergeReport,
    },
}

const CARD_H: f32 = 166.0;
const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 26.0;
const MESSAGE_H: f32 = 20.0;
const BOTTOM_H: f32 = 30.0;
const BOTTOM_W: f32 = 150.0;
const MAIN_W: f32 = 170.0;
const LINES_W: f32 = 110.0;
const THIRD_W: f32 = 150.0;
/// The table's fixed columns: «Today» phrase, the bar, «A/Y» (room for «999/999»).
const TODAY_W: f32 = 90.0;
const MINI_BAR_W: f32 = 44.0;
const PASSED_W: f32 = 64.0;
const DIALOG_W: f32 = 460.0;
/// A session appears at once when it is ready this fast; otherwise «Preparing…».
const PREPARING_SHOWN_AFTER: Duration = Duration::from_millis(300);
const MESSAGE_SHOWN: Duration = Duration::from_secs(6);
/// Seconds per question in «About {t} min».
const SECONDS_PER_MOVE: usize = 20;
const WARNING_ROWS: usize = 3;
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// What the table and the board need of a repertoire, from its PGN (indexed off the
/// frame).
#[derive(Clone)]
struct RowIndex {
    start: Chess,
    /// The position a session starts from (S0 shows it, not the library's preview).
    initial: Chess,
    last_move: Option<(Square, Square)>,
    has_moves: bool,
}

/// The counts of a repertoire for its colour, "today" meaning until the end of the day.
#[derive(Clone, Debug, Default, PartialEq)]
struct RowStats {
    total: usize,
    due: usize,
    unseen: usize,
    learning: usize,
    learnt: usize,
    /// The next review after today and how many moves are due that day.
    next: Option<(i64, usize)>,
}

impl RowStats {
    /// Moves met in training at least once: learning and learned.
    fn passed(&self) -> usize {
        self.learning + self.learnt
    }

    fn new_today(&self) -> usize {
        self.unseen.min(NEW_PER_SESSION)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatsKey {
    library_revision: u64,
    learning_revision: u64,
    sides: Vec<(RepertoireId, LearningSide)>,
    day: i64,
}

/// The card's states (spec §3).
#[derive(Clone, Debug, PartialEq)]
enum CardState {
    Review { due: usize, new: usize },
    OnlyNew { new: usize },
    Done { next: Option<(i64, usize)> },
    Empty,
    OtherColor,
    Counting,
    Failed(String),
}

fn card_state(has_moves: Option<bool>, stats: Option<&Result<RowStats, String>>) -> CardState {
    match stats {
        None => CardState::Counting,
        Some(Err(error)) => CardState::Failed(error.clone()),
        Some(Ok(stats)) if stats.total == 0 => {
            if has_moves == Some(true) {
                CardState::OtherColor
            } else {
                CardState::Empty
            }
        }
        Some(Ok(stats)) if stats.due > 0 => CardState::Review {
            due: stats.due,
            new: stats.new_today(),
        },
        Some(Ok(stats)) if stats.unseen > 0 => CardState::OnlyNew {
            new: stats.new_today(),
        },
        Some(Ok(stats)) => CardState::Done { next: stats.next },
    }
}

/// A PGN read for «Import PGN» or «Add PGN to this repertoire».
struct ImportPreview {
    library: Library,
    id: RepertoireId,
    base_revision: u64,
    games: usize,
    /// Lines of the result, and before the merge (an added file).
    lines: usize,
    lines_before: usize,
    /// Own moves for White and for Black.
    own: [usize; 2],
    suggested: LearningSide,
    default_name: String,
    warnings: Vec<String>,
    report: MergeReport,
}

impl ImportPreview {
    fn own(&self, side: LearningSide) -> usize {
        match side {
            LearningSide::White => self.own[0],
            LearningSide::Black => self.own[1],
        }
    }
}

struct ImportDialog {
    job: u64,
    /// `None`: a new repertoire.
    target: Option<RepertoireId>,
    preview: Option<ImportPreview>,
    name: String,
    side: LearningSide,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
}

enum Dialog {
    Import(ImportDialog),
    New {
        name: String,
        side: LearningSide,
        error: Option<String>,
    },
    Rename {
        id: RepertoireId,
        name: String,
        error: Option<String>,
        select_all: bool,
    },
    Color {
        id: RepertoireId,
        to: LearningSide,
        job: u64,
        count: Option<usize>,
    },
    Reset {
        id: RepertoireId,
        job: u64,
        impact: Option<(ResetImpactProjection, BTreeSet<LearningItemKey>)>,
        busy: bool,
    },
    Delete {
        id: RepertoireId,
    },
}

enum Screen {
    Repertoires,
    Session(Box<OpeningSessionUi>),
    Lines(Box<OpeningLinesUi>),
}

struct Preparing {
    job: u64,
    since: Instant,
    id: RepertoireId,
    name: String,
    kind: SessionKind,
}

struct Message {
    text: String,
    error: bool,
    until: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuItem {
    Train,
    ReviewAll,
    Lines,
    Rename,
    SwitchColor,
    AddPgn,
    Export,
    Reset,
    Delete,
}

enum Event {
    Indexed(u64, HashMap<RepertoireId, RowIndex>),
    Stats(StatsKey, HashMap<RepertoireId, Result<RowStats, String>>),
    Imported(u64, Result<ImportPreview, String>),
    Exported(Result<Option<PathBuf>, String>),
    SessionReady(u64, Result<Box<OpeningSessionUi>, String>),
    LinesReady(u64, Option<Box<OpeningLinesUi>>),
    LearningLoaded(Result<LearningStoreData, String>),
    OtherSide(u64, Result<usize, String>),
    ResetImpact(
        u64,
        Result<(ResetImpactProjection, BTreeSet<LearningItemKey>), String>,
    ),
    ResetDone(u64, Result<LearningStoreData, String>),
}

struct AddFromGame {
    source: Game,
    at: NodeId,
    selected: Option<RepertoireId>,
    name: String,
    side: LearningSide,
}

pub struct RepertoireUi {
    library_store: LibraryStore,
    learning_store: LearningStore,
    library: Library,
    learning: LearningStoreData,
    library_error: Option<String>,
    learning_error: Option<String>,
    selected: Option<RepertoireId>,
    index: HashMap<RepertoireId, RowIndex>,
    index_job: u64,
    stats: HashMap<RepertoireId, Result<RowStats, String>>,
    stats_key: Option<StatsKey>,
    /// The key of the counts shown (older than `stats_key` while counting).
    stats_shown: Option<StatsKey>,
    screen: Screen,
    /// The «Lines» screen a branch session was started from.
    lines_return: Option<Box<OpeningLinesUi>>,
    preparing: Option<Preparing>,
    lines_job: Option<u64>,
    /// The answers are being read again after a session.
    learning_loading: bool,
    dialog: Option<Dialog>,
    message: Option<Message>,
    search: String,
    scroll_to_selected: bool,
    return_to_game: Option<String>,
    add: Option<AddFromGame>,
    board: BoardState,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    initial_selection_pending: bool,
    debug_start: Option<String>,
    debug_pending_moves: Option<String>,
    debug_menu: bool,
    /// Colours of the starter repertoires created on the first opening, written to the
    /// settings by the next `show`.
    starter_sides: Vec<(RepertoireId, LearningSide)>,
}

impl RepertoireUi {
    pub fn new() -> Self {
        let (library_store, learning_store) = stores();
        let starter = seed_starter(&library_store);
        let mut this = Self::with_stores(library_store, learning_store);
        match starter {
            Ok(sides) => this.starter_sides = sides,
            Err(error) => this.show_message(error, true),
        }
        this
    }

    fn with_stores(library_store: LibraryStore, learning_store: LearningStore) -> Self {
        let (library, library_error) = match load_library(&library_store) {
            Ok(data) => (data, None),
            Err(error) => (Library::default(), Some(error)),
        };
        let (learning, learning_error) = match learning_store.load() {
            Ok(data) => (data, None),
            Err(error) => (LearningStoreData::default(), Some(error.to_string())),
        };
        let (tx, rx) = mpsc::channel();
        let mut this = Self {
            library_store,
            learning_store,
            library,
            learning,
            library_error,
            learning_error,
            selected: None,
            index: HashMap::new(),
            index_job: 0,
            stats: HashMap::new(),
            stats_key: None,
            stats_shown: None,
            screen: Screen::Repertoires,
            lines_return: None,
            preparing: None,
            lines_job: None,
            learning_loading: false,
            dialog: None,
            message: None,
            search: String::new(),
            scroll_to_selected: true,
            return_to_game: None,
            add: None,
            board: BoardState::default(),
            tx,
            rx,
            initial_selection_pending: true,
            debug_start: None,
            debug_pending_moves: None,
            debug_menu: false,
            starter_sides: Vec::new(),
        };
        this.selected = this.sorted_ids().first().cloned();
        this.reindex();
        this
    }

    pub fn has_unsaved(&self) -> bool {
        match &self.screen {
            Screen::Session(session) => session.has_unsaved(),
            Screen::Repertoires | Screen::Lines(_) => false,
        }
    }

    /// The window is being closed with an answer not saved: show the session.
    pub fn resolve_unsaved(&mut self) {}

    pub fn open_library(&mut self) {
        self.scroll_to_selected = true;
        if !matches!(self.screen, Screen::Session(_)) {
            self.screen = Screen::Repertoires;
        }
    }

    pub fn open_from_toast(&mut self, id: RepertoireId, title: String, _settings: &Settings) {
        self.return_to_game = Some(title);
        self.select(id);
    }

    pub fn open_add_from_game(&mut self, source: Game, at: NodeId, title: String) {
        self.return_to_game = Some(title);
        let start = fen4(source.start().position());
        let selected = self
            .library
            .repertoires
            .iter()
            .find(|r| fen4(r.game().start().position()) == start)
            .map(|r| r.id.clone());
        // The colour of the move chosen in the game.
        let side = match source.position_before(at).map(|p| p.turn()) {
            Some(Color::Black) => LearningSide::Black,
            _ => LearningSide::White,
        };
        self.add = Some(AddFromGame {
            source,
            at,
            selected,
            name: String::new(),
            side,
        });
    }

    fn sorted_ids(&self) -> Vec<RepertoireId> {
        let mut rows: Vec<_> = self
            .library
            .repertoires
            .iter()
            .map(|r| (r.name.to_lowercase(), r.id.clone()))
            .collect();
        rows.sort();
        rows.into_iter().map(|(_, id)| id).collect()
    }

    fn select(&mut self, id: RepertoireId) {
        self.initial_selection_pending = false;
        if self.library.repertoire(&id).is_some() {
            self.selected = Some(id);
            self.scroll_to_selected = true;
        }
    }

    fn selected_name(&self) -> String {
        self.selected
            .as_ref()
            .and_then(|id| self.library.repertoire(id))
            .map(|r| r.name.clone())
            .unwrap_or_default()
    }

    fn show_message(&mut self, text: impl Into<String>, error: bool) {
        self.message = Some(Message {
            text: text.into(),
            error,
            until: (!error).then(|| Instant::now() + MESSAGE_SHOWN),
        });
    }

    fn reindex(&mut self) {
        self.index_job = next_job();
        let job = self.index_job;
        let repertoires = self.library.repertoires.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-index", move || {
            let index = repertoires
                .into_iter()
                .map(|r| {
                    let game = r.game();
                    let start = layout::common_start(&game);
                    (
                        r.id.clone(),
                        RowIndex {
                            start: layout::position_of(&game, start),
                            initial: layout::position_of(&game, NodeId::ROOT),
                            last_move: layout::last_move(&game, start),
                            has_moves: !game.children(NodeId::ROOT).is_empty(),
                        },
                    )
                })
                .collect();
            let _ = tx.send(Event::Indexed(job, index));
        }) {
            self.show_message(error.to_string(), true);
        }
    }

    fn sides(&self, settings: &Settings) -> Vec<(RepertoireId, LearningSide)> {
        self.sorted_ids()
            .into_iter()
            .map(|id| {
                let side = layout::side_of(settings, &id);
                (id, side)
            })
            .collect()
    }

    /// Recount in the background when the library, the answers, a colour or the day
    /// changed; the previous numbers stay meanwhile.
    fn request_stats(&mut self, settings: &Settings) {
        if self.library_error.is_some() || self.learning_error.is_some() {
            return;
        }
        let today = layout::end_of_today();
        let key = StatsKey {
            library_revision: self.library.revision,
            learning_revision: self.learning.revision,
            sides: self.sides(settings),
            day: today,
        };
        if self.stats_key.as_ref() == Some(&key) {
            return;
        }
        self.stats_key = Some(key.clone());
        let library = self.library.clone();
        let learning = self.learning.learning.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-stats", move || {
            let stats = key
                .sides
                .iter()
                .map(|(id, side)| (id.clone(), row_stats(&library, &learning, id, *side, today)))
                .collect();
            let _ = tx.send(Event::Stats(key, stats));
        }) {
            self.show_message(error.to_string(), true);
        }
    }

    fn reload_learning(&mut self) {
        self.learning_loading = true;
        let store = self.learning_store.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-learning-load", move || {
            let _ = tx.send(Event::LearningLoaded(
                store.load().map_err(|error| error.to_string()),
            ));
        }) {
            self.learning_loading = false;
            self.show_message(error.to_string(), true);
        }
    }

    /// Writes the library; on success it is the one shown.
    fn persist(&mut self, mut library: Library) -> bool {
        if self.library_error.is_some() {
            return false;
        }
        match self.library_store.save(&mut library) {
            Ok(()) => {
                self.library = library;
                self.reindex();
                true
            }
            Err(error) => {
                self.show_message(
                    trf("Could not save: {e}", &[("e", &error.to_string())]),
                    true,
                );
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

    fn start(&mut self, kind: SessionKind, settings: &Settings) {
        if self.preparing.is_some() || self.has_unsaved() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let name = self.selected_name();
        let side = layout::side_of(settings, &id);
        let scope = StatsScope::for_repertoire(id.clone(), side, layout::end_of_today());
        let library = self.library.clone();
        let store = self.learning_store.clone();
        let job = next_job();
        let tx = self.tx.clone();
        let session_kind = kind.clone();
        let slow = debug_slow();
        if let Err(error) = crate::platform::spawn("repertoire-session-start", move || {
            if let Some(slow) = slow {
                std::thread::sleep(slow);
            }
            let result = OpeningSessionUi::new(library, store, scope, session_kind).map(Box::new);
            let _ = tx.send(Event::SessionReady(job, result));
        }) {
            self.show_message(error.to_string(), true);
            return;
        }
        self.preparing = Some(Preparing {
            job,
            since: Instant::now(),
            id,
            name,
            kind,
        });
    }

    fn more_new(&mut self) {
        let Screen::Session(session) = &self.screen else {
            return;
        };
        let Some(id) = self.selected.clone() else {
            return;
        };
        let snapshot = session.snapshot().clone();
        let orientation = session.orientation();
        let library = self.library.clone();
        let store = self.learning_store.clone();
        let job = next_job();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-session-more", move || {
            let result =
                OpeningSessionUi::more_new(library, store, snapshot, orientation).map(Box::new);
            let _ = tx.send(Event::SessionReady(job, result));
        }) {
            self.show_message(error.to_string(), true);
            return;
        }
        self.preparing = Some(Preparing {
            job,
            since: Instant::now(),
            id,
            name: self.selected_name(),
            kind: SessionKind::Daily,
        });
    }

    fn open_lines(&mut self, settings: &Settings, cursor: Option<Vec<String>>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let side = layout::side_of(settings, &id);
        let library = self.library.clone();
        let learning = self.learning.learning.clone();
        let job = next_job();
        self.lines_job = Some(job);
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-lines", move || {
            let lines = OpeningLinesUi::new(&library, &learning, id, side, cursor).map(Box::new);
            let _ = tx.send(Event::LinesReady(job, lines));
        }) {
            self.lines_job = None;
            self.show_message(error.to_string(), true);
        }
    }

    fn pick_import(&mut self, target: Option<RepertoireId>, ctx: &egui::Context) {
        let job = next_job();
        let cancel = Arc::new(AtomicBool::new(false));
        let base = self.library.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let worker_cancel = cancel.clone();
        let worker_target = target.clone();
        if let Err(error) = crate::platform::spawn("repertoire-import-dialog", move || {
            let picked = rfd::FileDialog::new()
                .add_filter("PGN", &["pgn", "PGN"])
                .pick_file();
            let Some(path) = picked else {
                let _ = tx.send(Event::Imported(job, Err(String::new())));
                ctx.request_repaint();
                return;
            };
            let _ = tx.send(Event::Imported(
                job,
                read_import(&path, worker_target, base, &worker_cancel),
            ));
            ctx.request_repaint();
        }) {
            self.show_message(error.to_string(), true);
            return;
        }
        self.dialog = Some(Dialog::Import(ImportDialog {
            job,
            target,
            preview: None,
            name: String::new(),
            side: LearningSide::White,
            error: None,
            cancel,
        }));
    }

    fn import_path(&mut self, path: PathBuf, target: Option<RepertoireId>) {
        let job = next_job();
        let cancel = Arc::new(AtomicBool::new(false));
        let base = self.library.clone();
        let tx = self.tx.clone();
        let worker_cancel = cancel.clone();
        let worker_target = target.clone();
        if let Err(error) = crate::platform::spawn("repertoire-import", move || {
            let _ = tx.send(Event::Imported(
                job,
                read_import(&path, worker_target, base, &worker_cancel),
            ));
        }) {
            self.show_message(error.to_string(), true);
            return;
        }
        self.dialog = Some(Dialog::Import(ImportDialog {
            job,
            target,
            preview: None,
            name: String::new(),
            side: LearningSide::White,
            error: None,
            cancel,
        }));
    }

    /// Imports what the dialog shows. Returns whether it was saved.
    fn confirm_import(&mut self, settings: &mut Settings) -> bool {
        let Some(Dialog::Import(dialog)) = &self.dialog else {
            return false;
        };
        let Some(preview) = &dialog.preview else {
            return false;
        };
        if preview.base_revision != self.library.revision {
            let error = tr("The library changed; import the PGN again").to_owned();
            if let Some(Dialog::Import(dialog)) = &mut self.dialog {
                dialog.error = Some(error);
            }
            return false;
        }
        let mut library = preview.library.clone();
        let id = preview.id.clone();
        let target = dialog.target.clone();
        let side = dialog.side;
        if target.is_none() {
            match self.checked_name(&dialog.name, None) {
                Ok(name) => {
                    if let Err(error) = library.rename(&id, name) {
                        if let Some(Dialog::Import(dialog)) = &mut self.dialog {
                            dialog.error = Some(error.to_string());
                        }
                        return false;
                    }
                }
                Err(error) => {
                    if let Some(Dialog::Import(dialog)) = &mut self.dialog {
                        dialog.error = Some(error);
                    }
                    return false;
                }
            }
        }
        if !self.persist(library) {
            return false;
        }
        if target.is_none() {
            layout::set_side(settings, &id, side);
        }
        self.dialog = None;
        self.select(id);
        true
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
        if let Err(error) = crate::platform::spawn("repertoire-export", move || {
            let path = std::env::var_os("CP_REPERTOIRE_EXPORT_FILE")
                .map(PathBuf::from)
                .or_else(|| {
                    rfd::FileDialog::new()
                        .set_file_name(name)
                        .add_filter("PGN", &["pgn"])
                        .save_file()
                });
            let result = match path {
                Some(path) => chess_core::pgn::store::write_atomic(&path, pgn.as_bytes())
                    .map(|()| Some(path))
                    .map_err(|error| error.to_string()),
                None => Ok(None),
            };
            let _ = tx.send(Event::Exported(result));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        }) {
            self.show_message(error.to_string(), true);
        }
    }

    fn open_color_dialog(&mut self, settings: &Settings) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let to = match layout::side_of(settings, &id) {
            LearningSide::White => LearningSide::Black,
            LearningSide::Black => LearningSide::White,
        };
        let job = next_job();
        let library = self.library.clone();
        let tx = self.tx.clone();
        let scope = StatsScope::for_repertoire(id.clone(), to, layout::now());
        if let Err(error) = crate::platform::spawn("repertoire-other-side", move || {
            let count = active_learning_keys(&library, &scope)
                .map(|keys| keys.len())
                .map_err(|error| error.to_string());
            let _ = tx.send(Event::OtherSide(job, count));
        }) {
            self.show_message(error.to_string(), true);
            return;
        }
        self.dialog = Some(Dialog::Color {
            id,
            to,
            job,
            count: None,
        });
    }

    fn open_reset_dialog(&mut self, settings: &Settings) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let scope =
            StatsScope::for_repertoire(id.clone(), layout::side_of(settings, &id), layout::now());
        let job = next_job();
        let library = self.library.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-reset-impact", move || {
            let result = reset_impact(&library, &scope)
                .and_then(|impact| {
                    active_learning_keys(&library, &scope).map(|keys| (impact, keys))
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(Event::ResetImpact(job, result));
        }) {
            self.show_message(error.to_string(), true);
            return;
        }
        self.dialog = Some(Dialog::Reset {
            id,
            job,
            impact: None,
            busy: false,
        });
    }

    fn confirm_reset(&mut self) {
        let Some(Dialog::Reset {
            job,
            impact: Some((_, keys)),
            busy,
            ..
        }) = &mut self.dialog
        else {
            return;
        };
        if *busy {
            return;
        }
        *busy = true;
        let job = *job;
        let keys = keys.clone();
        let store = self.learning_store.clone();
        let tx = self.tx.clone();
        if let Err(error) = crate::platform::spawn("repertoire-reset", move || {
            let result = store
                .reset_learning(keys)
                .map_err(|error| error.to_string())
                .and_then(|_| store.load().map_err(|error| error.to_string()));
            let _ = tx.send(Event::ResetDone(job, result));
        }) {
            self.dialog = None;
            self.show_message(error.to_string(), true);
        }
    }

    fn delete_selected(&mut self) {
        let Some(Dialog::Delete { id }) = &self.dialog else {
            return;
        };
        let id = id.clone();
        let order = self.sorted_ids();
        let position = order.iter().position(|other| *other == id);
        let mut library = self.library.clone();
        if let Err(error) = library.delete(&id) {
            self.show_message(error.to_string(), true);
            return;
        }
        if !self.persist(library) {
            return;
        }
        self.dialog = None;
        let order = self.sorted_ids();
        self.selected = position
            .and_then(|position| order.get(position.min(order.len().saturating_sub(1))))
            .cloned();
        self.scroll_to_selected = true;
    }

    fn poll(&mut self, settings: &mut Settings) {
        if self.initial_selection_pending {
            self.initial_selection_pending = false;
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Indexed(job, index) => {
                    if job == self.index_job {
                        self.index = index;
                    }
                }
                Event::Stats(key, stats) => {
                    if self.stats_key.as_ref() == Some(&key) {
                        self.stats = stats;
                        self.stats_shown = Some(key);
                    }
                }
                Event::Imported(job, result) => {
                    let Some(Dialog::Import(dialog)) = &mut self.dialog else {
                        continue;
                    };
                    if dialog.job != job {
                        continue;
                    }
                    match result {
                        Ok(preview) => {
                            dialog.name = preview.default_name.clone();
                            dialog.side = preview.suggested;
                            if let Some(target) = &dialog.target {
                                dialog.side = layout::side_of(settings, target);
                            }
                            dialog.preview = Some(preview);
                        }
                        // The file chooser was closed.
                        Err(error) if error.is_empty() => self.dialog = None,
                        Err(error) => {
                            self.dialog = None;
                            self.show_message(error, true);
                        }
                    }
                }
                Event::Exported(Ok(Some(path))) => {
                    self.show_message(trf("Saved to {path}", &[("path", &path.display())]), false)
                }
                Event::Exported(Ok(None)) => {}
                Event::Exported(Err(error)) => {
                    self.show_message(trf("Could not save: {e}", &[("e", &error)]), true)
                }
                Event::SessionReady(job, result) => {
                    if self.preparing.as_ref().is_none_or(|p| p.job != job) {
                        continue;
                    }
                    self.preparing = None;
                    match result {
                        Ok(mut session) if session.target_available() => {
                            if let Screen::Lines(lines) =
                                std::mem::replace(&mut self.screen, Screen::Repertoires)
                            {
                                self.lines_return = Some(lines);
                            }
                            session.set_back_to_lines(self.lines_return.is_some());
                            self.screen = Screen::Session(session);
                            if let Some(moves) = self.debug_pending_moves.take() {
                                self.debug_moves(&moves);
                            }
                        }
                        Ok(_) => {
                            self.screen_back_from_session();
                            self.show_message(tr("Nothing to train here right now"), false);
                        }
                        Err(error) => {
                            self.screen_back_from_session();
                            self.show_message(error, true);
                        }
                    }
                }
                Event::LinesReady(job, lines) => {
                    if self.lines_job != Some(job) {
                        continue;
                    }
                    self.lines_job = None;
                    if let Some(mut lines) = lines {
                        if let Ok(path) = std::env::var("CP_REPERTOIRE_LINE") {
                            let path: Vec<String> =
                                path.split_whitespace().map(str::to_owned).collect();
                            lines.debug_select(&path);
                        }
                        self.screen = Screen::Lines(lines);
                    }
                }
                Event::LearningLoaded(Ok(data)) => {
                    self.learning_loading = false;
                    self.learning = data;
                    self.learning_error = None;
                }
                Event::LearningLoaded(Err(error)) => {
                    self.learning_loading = false;
                    self.learning_error = Some(error);
                }
                Event::OtherSide(job, result) => {
                    if let Some(Dialog::Color {
                        job: expected,
                        count,
                        ..
                    }) = &mut self.dialog
                        && *expected == job
                    {
                        match result {
                            Ok(value) => *count = Some(value),
                            Err(error) => {
                                self.dialog = None;
                                self.show_message(error, true);
                            }
                        }
                    }
                }
                Event::ResetImpact(job, result) => {
                    if let Some(Dialog::Reset {
                        job: expected,
                        impact,
                        ..
                    }) = &mut self.dialog
                        && *expected == job
                    {
                        match result {
                            Ok(value) => *impact = Some(value),
                            Err(error) => {
                                self.dialog = None;
                                self.show_message(error, true);
                            }
                        }
                    }
                }
                Event::ResetDone(job, result) => {
                    if !matches!(&self.dialog, Some(Dialog::Reset { job: expected, .. }) if *expected == job)
                    {
                        continue;
                    }
                    self.dialog = None;
                    match result {
                        Ok(data) => {
                            self.learning = data;
                            self.learning_error = None;
                        }
                        Err(error) => self.show_message(error, true),
                    }
                }
            }
        }
        if std::env::var_os("CP_REPERTOIRE_CONFIRM_IMPORT").is_some()
            && matches!(&self.dialog, Some(Dialog::Import(d)) if d.preview.is_some())
        {
            self.confirm_import(settings);
        }
        if let Some(message) = &self.message
            && message.until.is_some_and(|until| Instant::now() >= until)
        {
            self.message = None;
        }
        if self.preparing.is_none()
            && self.dialog.is_none()
            && let Some(what) = self.debug_start.take()
        {
            self.debug_open(&what, settings);
        }
        self.request_stats(settings);
    }

    fn screen_back_from_session(&mut self) {
        self.screen = match self.lines_return.take() {
            Some(lines) => Screen::Lines(lines),
            None if matches!(self.screen, Screen::Lines(_)) => {
                std::mem::replace(&mut self.screen, Screen::Repertoires)
            }
            None => Screen::Repertoires,
        };
    }

    pub fn show(&mut self, ui: &mut Ui, settings: &mut Settings) -> Vec<RepertoireAction> {
        self.poll(settings);
        let mut actions = Vec::new();
        if !self.starter_sides.is_empty() {
            for (id, side) in self.starter_sides.drain(..) {
                layout::set_side(settings, &id, side);
            }
            actions.push(RepertoireAction::SettingsChanged);
        }
        let available = ui.available_rect_before_wrap();
        let background = ui.visuals().window_fill();
        ui.painter_at(available)
            .rect_filled(available, 0.0, background);
        if self.preparing.is_some() || self.lines_job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        if let Some(preparing) = &self.preparing
            && preparing.since.elapsed() >= PREPARING_SHOWN_AFTER
        {
            let side = layout::side_of(settings, &preparing.id);
            let index = self.index.get(&preparing.id);
            let start = index.map(|row| row.initial.clone()).unwrap_or_default();
            let subtitle = preparing.kind.subtitle(side);
            let view = PreparingView {
                name: &preparing.name,
                subtitle: &subtitle,
                back: if matches!(self.screen, Screen::Lines(_)) {
                    tr("⬅ Lines")
                } else {
                    tr("⬅ Repertoires")
                },
                position: &start,
                orientation: layout::orientation(side),
                last_move: None,
                lines: preparing.kind != SessionKind::Daily,
            };
            for action in show_preparing(ui, settings, &mut self.board, &view) {
                match action {
                    SessionAction::Back => {
                        self.preparing = None;
                        self.screen_back_from_session();
                    }
                    SessionAction::OpenSettings => actions.push(RepertoireAction::OpenSettings),
                    SessionAction::Sound(_) | SessionAction::MoreNew => {}
                }
            }
            return actions;
        }
        match std::mem::replace(&mut self.screen, Screen::Repertoires) {
            Screen::Session(mut session) => {
                let name = self.selected_name();
                let emitted = session.show(ui, settings, &name);
                self.screen = Screen::Session(session);
                for action in emitted {
                    match action {
                        SessionAction::Back => {
                            self.reload_learning();
                            self.screen_back_from_session();
                            if let Screen::Lines(lines) = &mut self.screen {
                                lines.refresh(&self.library, &self.learning.learning);
                            }
                            self.scroll_to_selected = true;
                        }
                        SessionAction::OpenSettings => actions.push(RepertoireAction::OpenSettings),
                        SessionAction::Sound(sound) => actions.push(RepertoireAction::Sound(sound)),
                        SessionAction::MoreNew => self.more_new(),
                    }
                }
                return actions;
            }
            Screen::Lines(mut lines) => {
                let emitted = lines.show(ui, settings);
                self.screen = Screen::Lines(lines);
                for action in emitted {
                    self.lines_action(action, settings, &mut actions);
                }
                return actions;
            }
            Screen::Repertoires => {}
        }
        self.repertoires_screen(ui, settings, &mut actions);
        self.show_dialog(ui.ctx(), settings, &mut actions);
        self.show_add(ui, settings, &mut actions);
        actions
    }

    fn lines_action(
        &mut self,
        action: LinesAction,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        match action {
            LinesAction::Back => {
                self.screen = Screen::Repertoires;
                self.scroll_to_selected = true;
            }
            LinesAction::OpenSettings => actions.push(RepertoireAction::OpenSettings),
            LinesAction::Train { through, label } => {
                let kind = if through.is_empty() {
                    SessionKind::ReviewAll
                } else {
                    SessionKind::Branch { through, label }
                };
                self.start(kind, settings);
            }
            LinesAction::SetDisabled { path, disabled } => {
                let Screen::Lines(lines) = &self.screen else {
                    return;
                };
                let id = lines.id().clone();
                let mut library = self.library.clone();
                let Some(repertoire) = library.repertoire_mut(&id) else {
                    return;
                };
                if let Err(error) = repertoire.set_prefix_disabled(&path, disabled) {
                    self.show_message(error.to_string(), true);
                    return;
                }
                if self.persist(library)
                    && let Screen::Lines(lines) = &mut self.screen
                {
                    lines.refresh(&self.library, &self.learning.learning);
                }
            }
        }
    }

    fn keyboard(&mut self, ui: &Ui, settings: &mut Settings, actions: &mut Vec<RepertoireAction>) {
        if ui.ctx().egui_wants_keyboard_input()
            || crate::app::overlay_open(ui.ctx())
            || self.dialog.is_some()
            || self.add.is_some()
        {
            return;
        }
        let mut command = None;
        ui.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::Escape) {
                command = Some("close");
            } else if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                command = Some("down");
            } else if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                command = Some("up");
            } else if input.consume_key(Modifiers::NONE, Key::Enter) {
                command = Some("main");
            }
        });
        let rows = self.visible_ids();
        let current = self
            .selected
            .as_ref()
            .and_then(|id| rows.iter().position(|row| row == id));
        match command {
            Some("close") => actions.push(RepertoireAction::Close),
            Some("down") => {
                let next = current.map_or(0, |i| (i + 1).min(rows.len().saturating_sub(1)));
                if let Some(id) = rows.get(next).cloned() {
                    self.select(id);
                }
            }
            Some("up") => {
                let next = current.map_or(0, |i| i.saturating_sub(1));
                if let Some(id) = rows.get(next).cloned() {
                    self.select(id);
                }
            }
            Some("main") => self.main_action(ui.ctx(), settings),
            _ => {}
        }
    }

    fn visible_ids(&self) -> Vec<RepertoireId> {
        let search = self.search.to_lowercase();
        self.sorted_ids()
            .into_iter()
            .filter(|id| {
                self.library
                    .repertoire(id)
                    .is_some_and(|r| r.name.to_lowercase().contains(&search))
            })
            .collect()
    }

    fn selected_card(&self) -> CardState {
        let Some(id) = &self.selected else {
            return CardState::Counting;
        };
        if let Some(error) = self.library_error.clone().or(self.learning_error.clone()) {
            return CardState::Failed(error);
        }
        card_state(
            self.index.get(id).map(|row| row.has_moves),
            self.stats.get(id),
        )
    }

    /// The card's main button.
    fn main_action(&mut self, ctx: &egui::Context, settings: &mut Settings) {
        match self.selected_card() {
            CardState::Review { .. } | CardState::OnlyNew { .. } => {
                self.start(SessionKind::Daily, settings)
            }
            CardState::Done { .. } => self.start(SessionKind::ReviewAll, settings),
            CardState::Empty => {
                let target = self.selected.clone();
                self.pick_import(target, ctx);
            }
            CardState::OtherColor => self.open_color_dialog(settings),
            CardState::Counting => {}
            CardState::Failed(_) => self.retry_load(),
        }
    }

    fn retry_load(&mut self) {
        match load_library(&self.library_store) {
            Ok(library) => {
                self.library = library;
                self.library_error = None;
                self.reindex();
            }
            Err(error) => self.library_error = Some(error),
        }
        match self.learning_store.load() {
            Ok(data) => {
                self.learning = data;
                self.learning_error = None;
            }
            Err(error) => self.learning_error = Some(error.to_string()),
        }
        self.stats_key = None;
        self.stats.clear();
    }

    fn menu_item(&mut self, item: MenuItem, ctx: &egui::Context, settings: &mut Settings) {
        match item {
            MenuItem::Train => self.main_action(ctx, settings),
            MenuItem::ReviewAll => self.start(SessionKind::ReviewAll, settings),
            MenuItem::Lines => self.open_lines(settings, None),
            MenuItem::Rename => {
                if let Some(id) = self.selected.clone() {
                    self.dialog = Some(Dialog::Rename {
                        name: self.selected_name(),
                        id,
                        error: None,
                        select_all: true,
                    });
                }
            }
            MenuItem::SwitchColor => self.open_color_dialog(settings),
            MenuItem::AddPgn => {
                let target = self.selected.clone();
                self.pick_import(target, ctx);
            }
            MenuItem::Export => self.export(Some(ctx.clone())),
            MenuItem::Reset => self.open_reset_dialog(settings),
            MenuItem::Delete => {
                if let Some(id) = self.selected.clone() {
                    self.dialog = Some(Dialog::Delete { id });
                }
            }
        }
    }

    fn repertoires_screen(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        self.keyboard(ui, settings, actions);
        let back = if self.return_to_game.is_some() {
            tr("⬅ To game")
        } else {
            tr("⬅ Library")
        };
        let back = if ui.available_width() < 600.0 {
            "⬅"
        } else {
            back
        };
        let count = self.library.repertoires.len();
        let subtitle = if count == 0 {
            String::new()
        } else {
            trf("{n} repertoires", &[("n", &count)])
        };
        let mut header = (false, false);
        let mut menu = None;
        layout::screen(
            ui,
            self,
            |_, ui| {
                header = crate::widgets::section_header_sub(
                    ui,
                    back,
                    tr("Opening training (α version)"),
                    &subtitle,
                )
            },
            |this, ui| menu = this.panel(ui, settings),
            |this, ui| this.preview_board(ui, settings),
        );
        if header.0 {
            actions.push(RepertoireAction::Close);
        }
        if header.1 {
            actions.push(RepertoireAction::OpenSettings);
        }
        if let Some(item) = menu {
            self.menu_item(item, ui.ctx(), settings);
        }
    }

    fn preview_board(&mut self, ui: &mut Ui, settings: &Settings) {
        let row = self.selected.as_ref().and_then(|id| self.index.get(id));
        let position = row.map(|row| row.start.clone()).unwrap_or_default();
        let last_move = row.and_then(|row| row.last_move);
        let side = self
            .selected
            .as_ref()
            .map_or(LearningSide::White, |id| layout::side_of(settings, id));
        layout::still_board(
            ui,
            &mut self.board,
            settings,
            &position,
            layout::orientation(side),
            last_move,
            &BoardAnnotations::default(),
        );
    }

    /// The right panel; returns a chosen menu item (or the card's buttons).
    fn panel(&mut self, ui: &mut Ui, settings: &Settings) -> Option<MenuItem> {
        let mut chosen = None;
        let rect = ui.available_rect_before_wrap();
        ui.spacing_mut().item_spacing.y = GAP;
        if self.library.repertoires.is_empty() {
            self.empty_panel(ui, rect);
            return None;
        }
        ui.add_space(4.0);
        chosen = chosen.or(self.card(ui, settings));
        if self.library.repertoires.len() > 8 {
            layout::fixed_block(ui, 24.0, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(tr("Search"))
                        .desired_width(f32::INFINITY),
                );
            });
        }
        let table_h = (ui.available_height() - MESSAGE_H - BOTTOM_H - 3.0 * GAP).max(HEADER_H);
        let table = layout::fixed_block(ui, table_h, |ui| self.table(ui, settings));
        chosen = chosen.or(table);
        layout::fixed_block(ui, MESSAGE_H, |ui| self.message_strip(ui));
        layout::fixed_block(ui, BOTTOM_H, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.library_error.is_none(),
                        egui::Button::new(tr("Import PGN…"))
                            .truncate()
                            .min_size(vec2(BOTTOM_W, BOTTOM_H)),
                    )
                    .clicked()
                {
                    self.pick_import(None, ui.ctx());
                }
                if ui
                    .add_enabled(
                        self.library_error.is_none(),
                        egui::Button::new(tr("New repertoire"))
                            .truncate()
                            .min_size(vec2(BOTTOM_W, BOTTOM_H)),
                    )
                    .clicked()
                {
                    self.dialog = Some(Dialog::New {
                        name: String::new(),
                        side: LearningSide::White,
                        error: None,
                    });
                }
            });
        });
        chosen
    }

    fn empty_panel(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let height = 200.0;
        let top = rect.top() + ((rect.height() - height) / 2.0).max(0.0);
        let area =
            egui::Rect::from_min_size(egui::pos2(rect.left(), top), vec2(rect.width(), height));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::Center)),
            |ui| {
                ui.heading(tr("No repertoires yet"));
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(RichText::new(tr("A repertoire is your opening moves as a PGN with variations. Import a file, or open any game, pick a move and choose “Add to repertoire…”.")).weak())
                        .wrap(),
                );
                ui.add_space(12.0);
                if ui
                    .add_enabled_ui(self.library_error.is_none(), |ui| {
                        crate::widgets::primary_wide(ui, tr("Import PGN…"), BOTTOM_W)
                    })
                    .inner
                    .clicked()
                {
                    self.pick_import(None, ui.ctx());
                }
                if ui
                    .add_enabled(
                        self.library_error.is_none(),
                        egui::Button::new(tr("New repertoire")).min_size(vec2(BOTTOM_W, 30.0)),
                    )
                    .clicked()
                {
                    self.dialog = Some(Dialog::New {
                        name: String::new(),
                        side: LearningSide::White,
                        error: None,
                    });
                }
                if let Some(error) = &self.library_error {
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(trf("Could not read the repertoires: {e}", &[("e", error)]))
                                .color(ui.visuals().warn_fg_color),
                        )
                        .wrap(),
                    );
                }
            },
        );
    }

    fn card(&mut self, ui: &mut Ui, settings: &Settings) -> Option<MenuItem> {
        let Some(id) = self.selected.clone() else {
            layout::fixed_block(ui, CARD_H, |_| {});
            return None;
        };
        let state = self.selected_card();
        let stats = self
            .stats
            .get(&id)
            .and_then(|stats| stats.as_ref().ok())
            .cloned();
        let side = layout::side_of(settings, &id);
        let name = self.selected_name();
        let mut chosen = None;
        layout::fixed_block(ui, CARD_H, |ui| {
            let inner = layout::card_inner(ui, CARD_H);
            layout::card(ui, false, false).show(ui, |ui| {
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                ui.spacing_mut().item_spacing.y = 6.0;
                let width = ui.available_width();
                // 1. The name and the colour.
                layout::fixed_block(ui, 26.0, |ui| {
                    ui.horizontal(|ui| {
                        let you = format!("{} {}", layout::king(side), layout::you_play(side));
                        let you_w = text_width(ui, &you, 14.0) + 8.0;
                        ui.allocate_ui_with_layout(
                            vec2((width - you_w).max(0.0), 26.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| layout::line(ui, RichText::new(&name).heading()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(you).weak());
                        });
                    });
                });
                // 2. Today.
                let (today, note) = card_texts(&state, side);
                layout::fixed_block(ui, 22.0, |ui| {
                    let mut text = RichText::new(today).strong().size(15.0);
                    if matches!(state, CardState::Failed(_)) {
                        text = text.color(ui.visuals().warn_fg_color);
                    }
                    layout::line(ui, text);
                });
                // 3. Progress.
                layout::fixed_block(ui, 28.0, |ui| progress(ui, stats.as_ref()));
                // 4. A quiet explanation.
                layout::fixed_block(ui, 20.0, |ui| {
                    layout::line(ui, RichText::new(note).weak());
                });
                // 5. Buttons.
                layout::fixed_block(ui, 30.0, |ui| {
                    ui.horizontal(|ui| {
                        let (label, enabled, item) = main_button(&state, side);
                        let gap = ui.spacing().item_spacing.x;
                        let pad = 2.0 * ui.spacing().button_padding.x + 2.0;
                        let room = ui.available_width();
                        // The row is the same in every state with moves: «Review all» is
                        // there unless the main button already is it. A narrow panel makes
                        // the buttons narrower, down to their words.
                        let third = !matches!(
                            state,
                            CardState::Empty
                                | CardState::Failed(_)
                                | CardState::Counting
                                | CardState::OtherColor
                        ) && !matches!(item, Some(MenuItem::ReviewAll));
                        let main_need = text_width(ui, &label, 14.0) + pad;
                        let lines_need = text_width(ui, tr("Lines…"), 14.0) + pad;
                        let third_need = text_width(ui, tr("Review all"), 14.0) + pad;
                        let (main_w, lines_w, third_w) = if third {
                            let spare = room - main_need - lines_need - third_need - 2.0 * gap;
                            let grow = |need: f32, most: f32, spare: &mut f32| {
                                let add = (most - need).max(0.0).min(spare.max(0.0));
                                *spare -= add;
                                need + add
                            };
                            let mut spare = spare;
                            let main_w = grow(main_need, MAIN_W, &mut spare);
                            let lines_w = grow(lines_need, LINES_W, &mut spare);
                            (main_w, lines_w, grow(third_need, THIRD_W, &mut spare))
                        } else {
                            (MAIN_W.min((room - LINES_W - gap).max(0.0)), LINES_W, 0.0)
                        };
                        let response = ui
                            .add_enabled_ui(enabled, |ui| {
                                crate::widgets::primary_wide(ui, &label, main_w)
                            })
                            .inner;
                        let response = if matches!(state, CardState::Counting) {
                            response.on_disabled_hover_text(tr("Counting moves…"))
                        } else {
                            response
                        };
                        if response.clicked() {
                            chosen = item;
                        }
                        if ui
                            .add_enabled(
                                !matches!(state, CardState::Empty | CardState::Failed(_)),
                                egui::Button::new(tr("Lines…"))
                                    .truncate()
                                    .min_size(vec2(lines_w, 30.0)),
                            )
                            .clicked()
                        {
                            chosen = Some(MenuItem::Lines);
                        }
                        if third
                            && ui
                                .add(
                                    egui::Button::new(tr("Review all"))
                                        .truncate()
                                        .min_size(vec2(third_w, 30.0)),
                                )
                                .on_hover_text(tr("Review all lines"))
                                .clicked()
                        {
                            chosen = Some(MenuItem::ReviewAll);
                        }
                    });
                });
            });
        });
        chosen
    }

    fn table(&mut self, ui: &mut Ui, settings: &Settings) -> Option<MenuItem> {
        let rows = self.visible_ids();
        let scroll_row = if std::mem::take(&mut self.scroll_to_selected) {
            rows.iter()
                .position(|id| Some(id) == self.selected.as_ref())
        } else {
            None
        };
        let mut clicked = None;
        let mut double = false;
        let mut chosen = None;
        let open_menu = std::mem::take(&mut self.debug_menu);
        ui.style_mut().interaction.selectable_labels = false;
        let mut table = egui_extras::TableBuilder::new(ui)
            .id_salt("opening_repertoires")
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::remainder().at_least(80.0).clip(true))
            .column(egui_extras::Column::exact(22.0))
            .column(egui_extras::Column::exact(TODAY_W))
            .column(egui_extras::Column::exact(MINI_BAR_W))
            .column(egui_extras::Column::exact(PASSED_W))
            .column(egui_extras::Column::exact(26.0));
        if let Some(index) = scroll_row {
            table = table.scroll_to_row(index, None);
        }
        table
            .header(HEADER_H, |mut header| {
                header.col(|ui| {
                    ui.add(egui::Label::new(RichText::new(tr("Name")).strong()).truncate());
                });
                header.col(|_| {});
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(tr("Today")).strong()).truncate());
                    });
                });
                // «Passed» heads the bar and the number together.
                header.col(|_| {});
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(tr("Passed")).strong()).truncate());
                    });
                });
                header.col(|_| {});
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let id = &rows[row.index()];
                    let Some(repertoire) = self.library.repertoire(id) else {
                        return;
                    };
                    let side = layout::side_of(settings, id);
                    let stats = self.stats.get(id);
                    let selected = self.selected.as_ref() == Some(id);
                    row.set_selected(selected);
                    row.col(|ui| {
                        layout::line(ui, repertoire.name.as_str());
                    });
                    row.col(|ui| {
                        ui.label(layout::king(side))
                            .on_hover_text(layout::you_play(side));
                    });
                    let has_moves = self.index.get(id).is_none_or(|row| row.has_moves);
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match stats {
                                Some(Ok(_)) if !has_moves => {}
                                Some(Ok(stats)) => {
                                    // A call to action in the normal colour; the rest quiet.
                                    let text = if stats.due > 0 {
                                        RichText::new(trf("review {n}", &[("n", &stats.due)]))
                                    } else if stats.new_today() > 0 {
                                        RichText::new(trf("new: {n}", &[("n", &stats.new_today())]))
                                            .weak()
                                    } else if let Some((at, _)) = stats.next {
                                        RichText::new(layout::when(at, layout::now())).weak()
                                    } else {
                                        RichText::new("—").weak()
                                    };
                                    ui.add(egui::Label::new(text).truncate()).on_hover_text(trf(
                                        "To review {n}, new in the session {m}",
                                        &[("n", &stats.due), ("m", &stats.new_today())],
                                    ));
                                }
                                Some(Err(_)) => {
                                    ui.label(RichText::new("—").weak());
                                }
                                None => {
                                    ui.label(RichText::new("…").weak());
                                }
                            }
                        });
                    });
                    let passed = matches!(stats, Some(Ok(stats)) if has_moves && stats.total > 0);
                    row.col(|ui| {
                        if let Some(Ok(stats)) = stats
                            && passed
                        {
                            let room = ui.available_rect_before_wrap();
                            let bar = egui::Rect::from_min_size(
                                egui::pos2(room.left(), room.center().y - 3.0),
                                vec2(MINI_BAR_W, 6.0),
                            );
                            progress_bar(ui, bar, Some(stats), selected);
                            ui.allocate_rect(room, egui::Sense::hover())
                                .on_hover_text(passed_hint(stats));
                        }
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match stats {
                                Some(Ok(_)) if !has_moves => {
                                    ui.add(
                                        egui::Label::new(RichText::new(tr("no moves")).weak())
                                            .truncate(),
                                    )
                                    .on_hover_text(tr("The repertoire has no moves yet: import a PGN or add a line from a game"));
                                }
                                Some(Ok(stats)) if passed => {
                                    ui.label(format!("{}/{}", stats.passed(), stats.total))
                                        .on_hover_text(passed_hint(stats));
                                }
                                Some(_) => {
                                    ui.label(RichText::new("—").weak());
                                }
                                None => {
                                    ui.label(RichText::new("…").weak());
                                }
                            }
                        });
                    });
                    let mut opened = false;
                    row.col(|ui| {
                        let button = ui.add(egui::Button::new("⋯").frame(false));
                        let mut popup = egui::Popup::menu(&button);
                        if open_menu && self.selected.as_ref() == Some(id) {
                            popup = popup.open_memory(Some(egui::SetOpenCommand::Bool(true)));
                        }
                        if let Some(item) =
                            popup
                                .show(|ui| menu_items(ui, side, stats))
                                .and_then(|inner| {
                                    opened = true;
                                    inner.inner
                                })
                        {
                            chosen = Some(item);
                        }
                    });
                    let response = row.response();
                    if let Some(item) = egui::Popup::context_menu(&response)
                        .show(|ui| menu_items(ui, side, stats))
                        .and_then(|inner| {
                            opened = true;
                            inner.inner
                        })
                    {
                        chosen = Some(item);
                    }
                    if opened && self.selected.as_ref() != Some(id) {
                        clicked = Some(id.clone());
                    }
                    if response.clicked() {
                        clicked = Some(id.clone());
                    }
                    if response.double_clicked() {
                        clicked = Some(id.clone());
                        double = true;
                    }
                });
            });
        if let Some(id) = clicked {
            self.selected = Some(id);
        }
        if double {
            chosen = Some(MenuItem::Train);
        }
        chosen
    }

    fn message_strip(&mut self, ui: &mut Ui) {
        let Some(message) = &self.message else {
            return;
        };
        let mut close = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = crate::widgets::small_close_button(ui).clicked();
                let mut text = RichText::new(&message.text);
                text = if message.error {
                    text.color(ui.visuals().warn_fg_color)
                } else {
                    text.weak()
                };
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    layout::line(ui, text);
                });
            });
        });
        if close {
            self.message = None;
        }
    }

    fn show_dialog(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let names: HashMap<RepertoireId, String> = self
            .library
            .repertoires
            .iter()
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        let name_of = |id: &RepertoireId| names.get(id).cloned().unwrap_or_default();
        let mut keep = true;
        let mut result = None;
        let enter = ctx.input(|input| input.key_pressed(Key::Enter));
        let mut dialog = dialog;
        let modal = egui::Modal::new(egui::Id::new("opening_dialog")).show(ctx, |ui| {
            ui.set_width(DIALOG_W);
            match &mut dialog {
                Dialog::Import(import) => {
                    result = import_dialog(ui, import, name_of, enter);
                }
                Dialog::New { name, side, error } => {
                    ui.heading(tr("New repertoire"));
                    ui.add_space(8.0);
                    name_field(ui, name, error, false);
                    side_choice(ui, side);
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(RichText::new(tr("Moves are added from a PGN file or with “Add to repertoire…” in any game.")).weak())
                            .wrap(),
                    );
                    result = dialog_buttons(ui, tr("Create"), true, None, enter);
                }
                Dialog::Rename {
                    name,
                    error,
                    select_all,
                    ..
                } => {
                    ui.heading(tr("Rename repertoire"));
                    ui.add_space(8.0);
                    name_field(ui, name, error, std::mem::take(select_all));
                    result = dialog_buttons(ui, tr("Save"), true, None, enter);
                }
                Dialog::Color { id, to, count, .. } => {
                    let name = name_of(id);
                    ui.heading(match to {
                        LearningSide::Black => trf("Play “{name}” as Black?", &[("name", &name)]),
                        LearningSide::White => trf("Play “{name}” as White?", &[("name", &name)]),
                    });
                    ui.add_space(8.0);
                    let count = count.map_or_else(|| "…".to_owned(), |n| n.to_string());
                    ui.add(
                        egui::Label::new(match to {
                            LearningSide::Black => trf(
                                "You will be asked Black's moves: {y}. Progress for White is kept and comes back if you switch again.",
                                &[("y", &count)],
                            ),
                            LearningSide::White => trf(
                                "You will be asked White's moves: {y}. Progress for Black is kept and comes back if you switch again.",
                                &[("y", &count)],
                            ),
                        })
                        .wrap(),
                    );
                    result = dialog_buttons(ui, tr("Switch"), true, None, enter);
                }
                Dialog::Reset { id, impact, busy, .. } => {
                    let name = name_of(id);
                    ui.heading(trf("Reset progress of “{name}”?", &[("name", &name)]));
                    ui.add_space(8.0);
                    match impact {
                        None => {
                            ui.label(RichText::new(tr("Counting moves…")).weak());
                        }
                        Some((impact, keys)) => {
                            ui.add(
                                egui::Label::new(trf(
                                    "All {y} moves of this repertoire become new again: answers and review dates are erased.",
                                    &[("y", &keys.len())],
                                ))
                                .wrap(),
                            );
                            ui.add(
                                egui::Label::new(tr("The moves themselves and PGN files do not change."))
                                    .wrap(),
                            );
                            let others: Vec<_> = impact
                                .affected_repertoires
                                .iter()
                                .filter(|other| other.id != *id)
                                .collect();
                            if impact.shared_count > 0 && !others.is_empty() {
                                let names = others
                                    .iter()
                                    .map(|other| other.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(trf(
                                            "{s} of these moves are also in: {names}. They become new there too.",
                                            &[("s", &impact.shared_count), ("names", &names)],
                                        ))
                                        .color(ui.visuals().warn_fg_color),
                                    )
                                    .wrap(),
                                );
                            }
                        }
                    }
                    let ready = impact.as_ref().is_some_and(|(_, keys)| !keys.is_empty()) && !*busy;
                    result = dialog_buttons(ui, tr("Reset"), ready, Some(()), enter);
                }
                Dialog::Delete { id } => {
                    let name = name_of(id);
                    ui.heading(trf("Delete “{name}”?", &[("name", &name)]));
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(tr(
                            "The repertoire leaves this list. PGN files on disk do not change.",
                        ))
                        .wrap(),
                    );
                    ui.add(
                        egui::Label::new(tr(
                            "Your answers are kept: import the same PGN again and the progress returns.",
                        ))
                        .wrap(),
                    );
                    result = dialog_buttons(ui, tr("Delete"), true, Some(()), enter);
                }
            }
        });
        if modal.should_close() && result.is_none() {
            result = Some(false);
        }
        match result {
            Some(false) => {
                if let Dialog::Import(import) = &dialog {
                    import.cancel.store(true, Ordering::Relaxed);
                }
                keep = false;
            }
            Some(true) => {
                self.dialog = Some(dialog);
                self.confirm_dialog(settings, actions);
                return;
            }
            None => {}
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }

    fn confirm_dialog(&mut self, settings: &mut Settings, actions: &mut Vec<RepertoireAction>) {
        match self.dialog.take() {
            Some(Dialog::Import(import)) => {
                self.dialog = Some(Dialog::Import(import));
                self.confirm_import(settings);
            }
            Some(Dialog::New { name, side, .. }) => match self.checked_name(&name, None) {
                Ok(name) => {
                    let mut library = self.library.clone();
                    match library.create_empty(name.clone()) {
                        Ok(id) => {
                            if self.persist(library) {
                                layout::set_side(settings, &id, side);
                                actions.push(RepertoireAction::SettingsChanged);
                                self.select(id);
                            } else {
                                self.dialog = Some(Dialog::New {
                                    name,
                                    side,
                                    error: None,
                                });
                            }
                        }
                        Err(error) => {
                            self.dialog = Some(Dialog::New {
                                name,
                                side,
                                error: Some(error.to_string()),
                            })
                        }
                    }
                }
                Err(error) => {
                    self.dialog = Some(Dialog::New {
                        name,
                        side,
                        error: Some(error),
                    })
                }
            },
            Some(Dialog::Rename { id, name, .. }) => match self.checked_name(&name, Some(&id)) {
                Ok(checked) => {
                    let mut library = self.library.clone();
                    match library.rename(&id, checked) {
                        Ok(()) => {
                            if !self.persist(library) {
                                self.dialog = Some(Dialog::Rename {
                                    id,
                                    name,
                                    error: None,
                                    select_all: false,
                                });
                            }
                        }
                        Err(error) => {
                            self.dialog = Some(Dialog::Rename {
                                id,
                                name,
                                error: Some(error.to_string()),
                                select_all: false,
                            })
                        }
                    }
                }
                Err(error) => {
                    self.dialog = Some(Dialog::Rename {
                        id,
                        name,
                        error: Some(error),
                        select_all: false,
                    })
                }
            },
            Some(Dialog::Color { id, to, .. }) => {
                layout::set_side(settings, &id, to);
                actions.push(RepertoireAction::SettingsChanged);
            }
            Some(dialog @ Dialog::Reset { .. }) => {
                self.dialog = Some(dialog);
                self.confirm_reset();
            }
            Some(dialog @ Dialog::Delete { .. }) => {
                self.dialog = Some(dialog);
                self.delete_selected();
            }
            None => {}
        }
    }

    fn show_add(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        actions: &mut Vec<RepertoireAction>,
    ) {
        let Some(mut add) = self.add.take() else {
            return;
        };
        let mut cancel = false;
        let mut commit = false;
        let start = fen4(add.source.start().position());
        let candidates: Vec<(RepertoireId, String)> = self
            .library
            .repertoires
            .iter()
            .filter(|r| {
                self.index.contains_key(&r.id) && fen4(r.game().start().position()) == start
            })
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        egui::Modal::new(egui::Id::new("add_to_repertoire")).show(ui.ctx(), |ui| {
            ui.set_max_width(450.0);
            ui.heading(tr("Add to repertoire"));
            ui.label(prefix_label(&add.source, add.at));
            for (id, name) in &candidates {
                ui.radio_value(&mut add.selected, Some(id.clone()), name);
            }
            ui.radio_value(&mut add.selected, None, tr("New repertoire"));
            if add.selected.is_none() {
                ui.add(
                    egui::TextEdit::singleline(&mut add.name)
                        .char_limit(120)
                        .hint_text(tr("Name")),
                );
                side_choice(ui, &mut add.side);
            }
            ui.horizontal(|ui| {
                cancel = ui.button(tr("Cancel")).clicked();
                commit = ui.button(tr("Add")).clicked();
            });
        });
        if commit {
            let result = (|| {
                let mut library = self.library.clone();
                let new = add.selected.is_none();
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
                Ok::<_, String>((library, id, name, report, new))
            })();
            match result {
                Ok((library, id, name, report, new)) if self.persist(library.clone()) => {
                    if new {
                        layout::set_side(settings, &id, add.side);
                        actions.push(RepertoireAction::SettingsChanged);
                    }
                    actions.push(RepertoireAction::Added { id, name, report });
                    return;
                }
                Ok(_) => {}
                Err(error) => self.show_message(error, true),
            }
        }
        if !cancel {
            self.add = Some(add);
        }
    }

    pub fn debug_import(&mut self, path: PathBuf) {
        let target =
            std::env::var_os("CP_REPERTOIRE_IMPORT_INTO").and_then(|_| self.selected.clone());
        self.import_path(path, target);
    }

    /// `CP_REPERTOIRE_CONFIRM_IMPORT`: confirmed in `poll`, where the chosen colour can
    /// be written to the settings.
    pub fn debug_confirm_import(&mut self, _settings: &Settings) -> bool {
        false
    }

    pub fn debug_export(&mut self) {
        self.export(None);
    }

    pub fn debug_rename(&mut self, name: String, _settings: &Settings) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let mut library = self.library.clone();
        if let Err(error) = library.rename(&id, name) {
            self.show_message(error.to_string(), true);
            return false;
        }
        self.persist(library)
    }

    pub fn debug_delete(&mut self, _settings: &Settings) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        self.dialog = Some(Dialog::Delete { id });
        self.delete_selected();
        self.dialog.is_none()
    }

    pub fn debug_add_from_game(&mut self, source: Game, at: NodeId, title: String, target: String) {
        self.open_add_from_game(source, at, title);
        if let Some(add) = &mut self.add {
            add.name = target;
            add.selected = None;
        }
    }

    /// QA hooks (docs/qa/opening-trainer-v2-devhooks.md): `CP_REPERTOIRE=<what>`, after
    /// `CP_REPERTOIRE_SELECT=<part of a name>`.
    pub fn debug_open(&mut self, what: &str, settings: &mut Settings) {
        if let Ok(name) = std::env::var("CP_REPERTOIRE_SELECT") {
            let name = name.to_lowercase();
            if let Some(id) = self
                .library
                .repertoires
                .iter()
                .find(|r| r.name.to_lowercase().contains(&name))
                .map(|r| r.id.clone())
            {
                self.select(id);
            }
        }
        let ready = self
            .selected
            .as_ref()
            .is_some_and(|id| self.stats.contains_key(id) && self.index.contains_key(id));
        let needs_stats = !matches!(what, "library" | "detail" | "new" | "message");
        if needs_stats && !ready && !self.library.repertoires.is_empty() {
            self.debug_start = Some(what.to_owned());
            return;
        }
        let ctx = egui::Context::default();
        match what {
            "white" | "black" => {
                if let Some(id) = self.selected.clone() {
                    let side = if what == "black" {
                        LearningSide::Black
                    } else {
                        LearningSide::White
                    };
                    if layout::side_of(settings, &id) != side {
                        layout::set_side(settings, &id, side);
                        self.debug_start = Some("train".to_owned());
                        return;
                    }
                    self.start(SessionKind::Daily, settings);
                }
            }
            "train" => self.start(SessionKind::Daily, settings),
            "main" => self.main_action(&ctx, settings),
            "review" => self.start(SessionKind::ReviewAll, settings),
            "lines" => {
                let path = std::env::var("CP_REPERTOIRE_LINE")
                    .ok()
                    .map(|path| path.split_whitespace().map(str::to_owned).collect());
                self.open_lines(settings, path);
            }
            "branch" => {
                let through: Vec<String> = std::env::var("CP_REPERTOIRE_LINE")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                // The label in the notation of the application, as «Lines» gives it.
                let label = self
                    .selected
                    .as_ref()
                    .and_then(|id| self.library.repertoire(id))
                    .map(|r| r.game())
                    .and_then(|game| {
                        layout::node_at(&game, &through).map(|at| layout::path_text(&game, at))
                    })
                    .unwrap_or_else(|| through.join(" "));
                self.start(SessionKind::Branch { through, label }, settings);
            }
            "menu" => self.debug_menu = true,
            "new" => {
                self.dialog = Some(Dialog::New {
                    name: std::env::var("CP_REPERTOIRE_NAME").unwrap_or_default(),
                    side: LearningSide::White,
                    error: None,
                });
                if std::env::var_os("CP_REPERTOIRE_NAME").is_some() {
                    self.confirm_dialog(settings, &mut Vec::new());
                }
            }
            "rename" => self.menu_item(MenuItem::Rename, &ctx, settings),
            "color" => self.open_color_dialog(settings),
            "reset" => self.open_reset_dialog(settings),
            "delete" => self.menu_item(MenuItem::Delete, &ctx, settings),
            "message" => self.show_message(
                trf(
                    "Could not save: {e}",
                    &[("e", &"Permission denied (os error 13)")],
                ),
                true,
            ),
            _ => {}
        }
    }

    pub fn debug_moves(&mut self, moves: &str) {
        if let Screen::Session(session) = &mut self.screen {
            session.debug_moves(moves);
        } else {
            self.debug_pending_moves = Some(moves.to_owned());
        }
    }

    pub fn debug_hint(&mut self) {
        if let Screen::Session(session) = &mut self.screen {
            session.debug_hint();
        }
    }
}

impl Drop for RepertoireUi {
    fn drop(&mut self) {
        if let Some(Dialog::Import(import)) = &self.dialog {
            import.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn next_job() -> u64 {
    NEXT_JOB.fetch_add(1, Ordering::Relaxed)
}

/// `CP_REPERTOIRE_SLOW=<ms>`: a session takes this long to prepare (to see «Preparing…»).
fn debug_slow() -> Option<Duration> {
    std::env::var("CP_REPERTOIRE_SLOW")
        .ok()?
        .parse()
        .ok()
        .map(Duration::from_millis)
}

fn row_stats(
    library: &Library,
    learning: &LearningData,
    id: &RepertoireId,
    side: LearningSide,
    today: i64,
) -> Result<RowStats, String> {
    let scope = StatsScope::for_repertoire(id.clone(), side, today);
    let scoped = aggregate_scope(library, learning, &scope, &SrsSettings::default())
        .map_err(|error| error.to_string())?;
    let summary = &scoped.summary;
    let next = summary.next_due_at.map(|at| {
        let day_end = end_of_day(at);
        let count = scoped
            .items
            .iter()
            .filter(|item| {
                item.introduced
                    && item
                        .next_review_at
                        .is_some_and(|next| next > today && next <= day_end)
            })
            .count();
        (at, count)
    });
    Ok(RowStats {
        total: summary.total,
        due: summary.due,
        unseen: summary.unseen,
        learning: summary.learning,
        learnt: summary.learnt,
        next,
    })
}

fn end_of_day(at: i64) -> i64 {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(at, 0)
        .single()
        .and_then(|time| time.date_naive().succ_opt())
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
        .map_or(at, |midnight| midnight.timestamp() - 1)
}

/// Lines 2 and 4 of the card.
fn card_texts(state: &CardState, side: LearningSide) -> (String, String) {
    match state {
        CardState::Review { due, new } => {
            let today = if *new > 0 {
                trf("Today: {n} to review · {m} new", &[("n", due), ("m", new)])
            } else {
                trf("Today: {n} to review", &[("n", due)])
            };
            let seconds = (due + new) * SECONDS_PER_MOVE;
            let note = if seconds >= 60 {
                trf("About {t} min", &[("t", &seconds.div_ceil(60))])
            } else {
                String::new()
            };
            (today, note)
        }
        CardState::OnlyNew { new } => (
            trf("Today: {m} new moves", &[("m", new)]),
            tr("Nothing to review today").to_owned(),
        ),
        CardState::Done { next } => (
            tr("Done for today").to_owned(),
            next.map(|(at, count)| {
                trf(
                    "Next review: {when} · {n} moves",
                    &[("when", &layout::when(at, layout::now())), ("n", &count)],
                )
            })
            .unwrap_or_default(),
        ),
        CardState::Empty => (
            tr("No moves yet").to_owned(),
            tr("Add a PGN, or use “Add to repertoire…” in any game").to_owned(),
        ),
        CardState::OtherColor => (
            match side {
                LearningSide::White => tr("No moves for White here"),
                LearningSide::Black => tr("No moves for Black here"),
            }
            .to_owned(),
            tr("This PGN may be a repertoire for the other colour").to_owned(),
        ),
        CardState::Counting => (tr("Counting moves…").to_owned(), String::new()),
        CardState::Failed(error) => (tr("Cannot read this repertoire").to_owned(), error.clone()),
    }
}

/// The card's main button: label, enabled, what it does.
fn main_button(state: &CardState, side: LearningSide) -> (String, bool, Option<MenuItem>) {
    match state {
        CardState::Review { .. } => (tr("Train").to_owned(), true, Some(MenuItem::Train)),
        CardState::OnlyNew { new } => (
            trf("Learn {m} new", &[("m", new)]),
            true,
            Some(MenuItem::Train),
        ),
        CardState::Done { .. } => (
            tr("Review all lines").to_owned(),
            true,
            Some(MenuItem::ReviewAll),
        ),
        CardState::Empty => (tr("Add PGN…").to_owned(), true, Some(MenuItem::AddPgn)),
        CardState::OtherColor => (
            match side {
                LearningSide::White => tr("Switch colour to Black"),
                LearningSide::Black => tr("Switch colour to White"),
            }
            .to_owned(),
            true,
            Some(MenuItem::SwitchColor),
        ),
        CardState::Counting => (tr("Train").to_owned(), false, None),
        CardState::Failed(_) => (tr("Try again").to_owned(), true, Some(MenuItem::Train)),
    }
}

/// Row 3 of the card: learnt, in progress, the rest.
/// «Learned 3, learning 10, 75 moves in all» and what «learned» takes.
fn passed_hint(stats: &RowStats) -> String {
    trf(
        "Passed {a} of {y} moves, {l} of them consolidated. A move is consolidated after three correct answers in a row without a hint — usually over several days of reviews.",
        &[
            ("a", &stats.passed()),
            ("y", &stats.total),
            ("l", &stats.learnt),
        ],
    )
}

/// Blue — passed, green inside it — consolidated.
/// Learned — the green of a correct move; learning — the accent blue; the track stands
/// out from the row. On a selected dark row: white, half-white and a dark track.
fn progress_bar(ui: &Ui, bar: egui::Rect, stats: Option<&RowStats>, selected: bool) {
    let visuals = ui.visuals();
    let (track, learnt_fill, learning_fill) = if selected && visuals.dark_mode {
        (
            Color32::from_black_alpha(90),
            Color32::WHITE,
            Color32::from_white_alpha(128),
        )
    } else if selected {
        // The light selection is pale: white would vanish, the selection's text colour shows.
        let ink = visuals.selection.stroke.color;
        (Color32::from_white_alpha(170), ink, ink.gamma_multiply(0.5))
    } else {
        let green = crate::notation::assessment_text_color(chess_core::Nag(1), visuals.dark_mode)
            .unwrap_or(visuals.selection.bg_fill);
        let track = if visuals.dark_mode {
            visuals.widgets.inactive.bg_fill
        } else {
            visuals.widgets.inactive.bg_fill.gamma_multiply(0.9)
        };
        (track, green, visuals.hyperlink_color)
    };
    let painter = ui.painter();
    painter.rect_filled(bar, 2.0, track);
    if let Some(stats) = stats.filter(|stats| stats.total > 0) {
        let total = stats.total as f32;
        let learnt = bar.width() * stats.learnt as f32 / total;
        let learning = bar.width() * stats.learning as f32 / total;
        painter.rect_filled(
            egui::Rect::from_min_size(bar.min, vec2(learnt, bar.height())),
            2.0,
            learnt_fill,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(bar.min + vec2(learnt, 0.0), vec2(learning, bar.height())),
            0.0,
            learning_fill,
        );
    }
}

fn progress(ui: &mut Ui, stats: Option<&RowStats>) {
    let width = ui.available_width();
    let (bar, _) = ui.allocate_exact_size(vec2(width, 8.0), egui::Sense::hover());
    progress_bar(ui, bar, stats, false);
    ui.add_space(-2.0);
    ui.horizontal(|ui| {
        ui.set_height(20.0);
        if let Some(stats) = stats {
            let text = trf(
                "Passed {a} of {y} moves · consolidated {l}",
                &[
                    ("a", &stats.passed()),
                    ("y", &stats.total),
                    ("l", &stats.learnt),
                ],
            );
            ui.add(egui::Label::new(text).truncate())
                .on_hover_text(passed_hint(stats));
        }
    });
}

fn menu_items(
    ui: &mut Ui,
    side: LearningSide,
    stats: Option<&Result<RowStats, String>>,
) -> Option<MenuItem> {
    let has_moves = matches!(stats, Some(Ok(stats)) if stats.total > 0);
    let mut chosen = None;
    let mut item = |ui: &mut Ui, enabled: bool, label: &str, value: MenuItem| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            chosen = Some(value);
            ui.close();
        }
    };
    item(ui, has_moves, tr("Train"), MenuItem::Train);
    item(ui, has_moves, tr("Review all lines"), MenuItem::ReviewAll);
    item(ui, true, tr("Lines…"), MenuItem::Lines);
    ui.separator();
    item(ui, true, tr("Rename…"), MenuItem::Rename);
    item(
        ui,
        true,
        match side {
            LearningSide::White => tr("Switch colour to Black"),
            LearningSide::Black => tr("Switch colour to White"),
        },
        MenuItem::SwitchColor,
    );
    item(
        ui,
        true,
        tr("Add PGN to this repertoire…"),
        MenuItem::AddPgn,
    );
    item(ui, true, tr("Export PGN…"), MenuItem::Export);
    ui.separator();
    item(ui, has_moves, tr("Reset progress…"), MenuItem::Reset);
    item(ui, true, tr("Delete…"), MenuItem::Delete);
    chosen
}

/// The dialog's buttons on the right: «Cancel», then the main one. `danger`: both are
/// plain and Enter means «Cancel». Returns `Some(true)` to confirm, `Some(false)` to
/// cancel.
fn dialog_buttons(
    ui: &mut Ui,
    label: &str,
    enabled: bool,
    danger: Option<()>,
    enter: bool,
) -> Option<bool> {
    let mut result = None;
    ui.add_space(12.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let main = if danger.is_some() {
            ui.add_enabled(enabled, egui::Button::new(label).min_size(vec2(0.0, 30.0)))
        } else {
            ui.add_enabled_ui(enabled, |ui| crate::widgets::primary(ui, label))
                .inner
        };
        if main.clicked() {
            result = Some(true);
        }
        let cancel = ui.add(egui::Button::new(tr("Cancel")).min_size(vec2(0.0, 30.0)));
        if danger.is_some() && !ui.ctx().memory(|m| m.focused().is_some()) {
            cancel.request_focus();
        }
        if cancel.clicked() {
            result = Some(false);
        }
    });
    if result.is_none() && enter {
        if danger.is_some() {
            result = Some(false);
        } else if enabled {
            result = Some(true);
        }
    }
    result
}

fn name_field(ui: &mut Ui, name: &mut String, error: &mut Option<String>, select_all: bool) {
    ui.label(RichText::new(tr("Name")).strong().size(15.0));
    let output = egui::TextEdit::singleline(name)
        .char_limit(120)
        .desired_width(f32::INFINITY)
        .show(ui);
    if select_all {
        let mut state = output.state.clone();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(name.chars().count()),
            )));
        state.store(ui.ctx(), output.response.id);
        output.response.request_focus();
    }
    if output.response.changed() {
        *error = None;
    }
    // The place of the error is kept: the dialog does not grow when it appears.
    ui.allocate_ui(vec2(ui.available_width(), 18.0), |ui| {
        if let Some(error) = error.as_deref() {
            ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
        }
    });
}

fn side_choice(ui: &mut Ui, side: &mut LearningSide) {
    ui.add_space(4.0);
    ui.label(RichText::new(tr("You play")).strong().size(15.0));
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        for (value, label) in [
            (LearningSide::White, tr("♔ White")),
            (LearningSide::Black, tr("♚ Black")),
        ] {
            if ui
                .add_sized(
                    [222.0, 34.0],
                    egui::Button::selectable(*side == value, RichText::new(label).size(14.0))
                        .frame_when_inactive(true),
                )
                .clicked()
            {
                *side = value;
            }
        }
    });
}

fn import_dialog(
    ui: &mut Ui,
    dialog: &mut ImportDialog,
    name_of: impl Fn(&RepertoireId) -> String,
    enter: bool,
) -> Option<bool> {
    let adding = dialog.target.is_some();
    ui.heading(match &dialog.target {
        Some(id) => trf("Add PGN to “{name}”", &[("name", &name_of(id))]),
        None => tr("Import PGN").to_owned(),
    });
    ui.add_space(8.0);
    if !adding {
        name_field(ui, &mut dialog.name, &mut dialog.error, false);
        side_choice(ui, &mut dialog.side);
        ui.label(RichText::new(tr("Chosen from the PGN — check it")).weak());
        ui.add_space(6.0);
    }
    let mut enabled = false;
    let mut hover = None;
    match &dialog.preview {
        None => {
            ui.label(RichText::new(tr("Reading the file…")).weak());
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        Some(preview) => {
            if adding {
                let lines = preview.lines.saturating_sub(preview.lines_before);
                if preview.report.added_nodes == 0 {
                    ui.label(RichText::new(tr("Nothing new in this file")).weak());
                } else {
                    ui.label(
                        RichText::new(trf(
                            "{m} new moves · {v} new lines",
                            &[("m", &preview.report.added_nodes), ("v", &lines)],
                        ))
                        .weak(),
                    );
                    enabled = true;
                }
            } else {
                let own = preview.own(dialog.side);
                ui.label(
                    RichText::new(trf(
                        "{g} games · {v} lines · {y} your moves",
                        &[("g", &preview.games), ("v", &preview.lines), ("y", &own)],
                    ))
                    .weak(),
                );
                enabled = own > 0 && !dialog.name.trim().is_empty();
                if own == 0 {
                    hover = Some(tr("No moves for this colour"));
                }
            }
            for warning in preview.warnings.iter().take(WARNING_ROWS) {
                layout::line(ui, RichText::new(warning).color(ui.visuals().warn_fg_color));
            }
            if preview.warnings.len() > WARNING_ROWS {
                ui.label(
                    RichText::new(trf(
                        "and {k} more",
                        &[("k", &(preview.warnings.len() - WARNING_ROWS))],
                    ))
                    .color(ui.visuals().warn_fg_color),
                );
            }
            if adding && let Some(error) = &dialog.error {
                ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
            }
        }
    }
    let label = if adding { tr("Add") } else { tr("Import") };
    let result = dialog_buttons(ui, label, enabled, None, enter);
    if let Some(hover) = hover {
        ui.label(RichText::new(hover).weak());
    }
    result
}

fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(size),
            egui::Color32::WHITE,
        )
        .size()
        .x
}

/// The colour whose moves a PGN repertoire gives: the side with fewer positions where
/// the PGN has several moves (one answer to many replies). White on a tie.
fn suggested_side(game: &Game) -> LearningSide {
    let mut branching = [0usize; 2];
    let mut stack = vec![NodeId::ROOT];
    while let Some(node) = stack.pop() {
        let children = game.children(node);
        if children.len() > 1
            && let Some(position) = game.position(node)
        {
            branching[usize::from(position.turn() == Color::Black)] += 1;
        }
        stack.extend(children.iter().copied());
    }
    if branching[1] < branching[0] {
        LearningSide::Black
    } else {
        LearningSide::White
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
    let base_revision = library.revision;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| tr("New repertoire").to_owned());
    let mut warnings = Vec::new();
    let mut id = target.clone();
    let mut report = MergeReport::default();
    let mut event = None;
    let lines_before = target
        .as_ref()
        .and_then(|id| library.repertoire(id))
        .map_or(0, |r| layout::lines(&r.game(), |_| false).len());
    for index in 0..db.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
        let Some(parsed) = db.load_game(index) else {
            warnings.push(trf("Game {n}: could not be read", &[("n", &(index + 1))]));
            continue;
        };
        if let Some(d) = parsed
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
        {
            warnings.push(trf(
                "Game {n}: {message}",
                &[("n", &(index + 1)), ("message", &d.message)],
            ));
        }
        if event.is_none() {
            event = parsed
                .game
                .tag("Event")
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty() && value != "?");
        }
        if parsed.game.children(NodeId::ROOT).is_empty() {
            warnings.push(trf("Game {n} has no moves", &[("n", &(index + 1))]));
            continue;
        }
        let current = match &id {
            Some(id) => id.clone(),
            None => {
                let new_id = library
                    .create(stem.clone(), &Game::from_start(parsed.game.start().clone()))
                    .map_err(|e| e.to_string())?;
                id = Some(new_id.clone());
                new_id
            }
        };
        match library.add_game(&current, &parsed.game) {
            Ok(added) => {
                report.added_nodes += added.added_nodes;
                report.merged_nodes += added.merged_nodes;
                report.annotation_changes += added.annotation_changes;
            }
            Err(error) => warnings.push(trf(
                "Game {n}: {message}",
                &[("n", &(index + 1)), ("message", &error.to_string())],
            )),
        }
    }
    let id = id.ok_or_else(|| tr("No games with moves in this file").to_owned())?;
    let game = library
        .repertoire(&id)
        .map(|r| r.game())
        .ok_or_else(|| tr("No games").to_owned())?;
    let own = [LearningSide::White, LearningSide::Black].map(|side| {
        active_learning_keys(&library, &StatsScope::for_repertoire(id.clone(), side, 0))
            .map_or(0, |keys| keys.len())
    });
    let mut name = event.unwrap_or(stem);
    let base = name.clone();
    let mut suffix = 2;
    while library
        .repertoires
        .iter()
        .any(|r| r.id != id && r.name.to_lowercase() == name.to_lowercase())
    {
        name = format!("{base} ({suffix})");
        suffix += 1;
    }
    Ok(ImportPreview {
        lines: layout::lines(&game, |_| false).len(),
        lines_before,
        suggested: suggested_side(&game),
        library,
        id,
        base_revision,
        games: db.len(),
        own,
        default_name: name,
        warnings,
        report,
    })
}

const STARTER_REPERTOIRES: [(&str, &str, LearningSide); 2] = [
    (
        "Spanish Opening",
        include_str!("../assets/repertoires/spanish-white.pgn"),
        LearningSide::White,
    ),
    (
        "Sicilian Defence: Najdorf",
        include_str!("../assets/repertoires/sicilian-black.pgn"),
        LearningSide::Black,
    ),
];

/// The first opening of the section, before any repertoire library file exists, gets
/// two example repertoires. An existing file, even an empty one, is never touched, so
/// examples deleted by the user do not come back.
fn seed_starter(store: &LibraryStore) -> Result<Vec<(RepertoireId, LearningSide)>, String> {
    if store.path().exists() {
        return Ok(Vec::new());
    }
    let mut library = Library::default();
    let mut sides = Vec::new();
    let cancel = AtomicBool::new(false);
    for (name, pgn, side) in STARTER_REPERTOIRES {
        let db = PgnDatabase::from_bytes(pgn.as_bytes().to_vec(), None, &mut |_| {}, &cancel)
            .map_err(|e| e.to_string())?;
        let mut id = None;
        for index in 0..db.len() {
            let Some(parsed) = db.load_game(index) else {
                continue;
            };
            let current = match &id {
                Some(id) => id,
                None => id.insert(
                    library
                        .create(tr(name), &Game::from_start(parsed.game.start().clone()))
                        .map_err(|e| e.to_string())?,
                ),
            };
            library
                .add_game(current, &parsed.game)
                .map_err(|e| e.to_string())?;
        }
        if let Some(id) = id {
            sides.push((id, side));
        }
    }
    store.save(&mut library).map_err(|e| e.to_string())?;
    Ok(sides)
}

fn load_library(store: &LibraryStore) -> Result<Library, String> {
    let mut library = store.load().map_err(|error| error.to_string())?;
    // The depth limit of 2.2 is not a user's choice: trained lines go to their ends.
    // The PGN does not change.
    for repertoire in &mut library.repertoires {
        repertoire.set_max_plies(u32::MAX);
    }
    Ok(library)
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

#[cfg(test)]
#[path = "repertoire_ui_tests.rs"]
mod tests;
