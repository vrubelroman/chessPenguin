//! The «Session» screen of opening training (docs/design/opening-trainer-v2-spec.md §4–6):
//! the program plays the line up to the next question, shows a new move before asking
//! it, takes the answer on the board and records it in the background.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use chess_core::shakmaty::{CastlingMode, Chess, Color, Move, Square, fen::Fen, uci::UciMove};
use chess_core::{Game, NodeId};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText, Ui, vec2};
use trainer::repertoire::{
    AttemptApplication, LearningContextStep, LearningData, LearningItem, LearningItemKey,
    LearningPresentation, LearningSessionError, LearningSessionPhase, LearningSessionSnapshot,
    LearningSessionTarget, LearningSide, LearningStore, Library, PersistedSessionAttempt,
    RepertoireId, SrsSettings, StatsScope, TrainingAttempt, TrainingEventType,
    active_learning_keys, fen4,
};
use web_time::{Instant, SystemTime, UNIX_EPOCH};

use crate::assets::Sound;
use crate::board::{Arrow, BoardState, BoardView};
use crate::i18n::{tr, trf};
use crate::opening_layout::{self as layout, BoardAnnotations, GAP, pgn_annotations_at};
use crate::settings::Settings;

const WRONG_SHOWN: Duration = Duration::from_millis(400);
/// How long «Correct» and «Got it» stay after the move's animation.
const FEEDBACK: Duration = Duration::from_millis(600);
const DEFAULT_ANIMATION: Duration = Duration::from_millis(250);
/// Mistakes listed under «Moves to remember».
const REMEMBER_ROWS: usize = 8;
const CARD_H: f32 = 64.0;
const LINE_H: f32 = 20.0;
const ROW_H: f32 = 30.0;
const TOOL_H: f32 = 28.0;
const LEFT_W: f32 = 150.0;
const MAIN_W: f32 = 170.0;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub enum SessionAction {
    /// Leave the session (to the repertoires, or to the lines it was started from).
    Back,
    OpenSettings,
    Sound(Sound),
    /// «10 more new»: the caller prepares the next new moves in the background.
    MoreNew,
}

/// What the session goes through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionKind {
    /// What is due today and up to ten new moves.
    Daily,
    /// Every move of every line, one pass.
    ReviewAll,
    /// Every move of the lines through `through` (UCI from the start).
    Branch { through: Vec<String>, label: String },
}

impl SessionKind {
    pub fn subtitle(&self, side: LearningSide) -> String {
        match self {
            Self::Daily => match side {
                LearningSide::White => tr("Training · you play White").to_owned(),
                LearningSide::Black => tr("Training · you play Black").to_owned(),
            },
            Self::ReviewAll => match side {
                LearningSide::White => tr("Review all lines · you play White").to_owned(),
                LearningSide::Black => tr("Review all lines · you play Black").to_owned(),
            },
            Self::Branch { label, .. } => trf("Branch: {path}", &[("path", label)]),
        }
    }
}

#[derive(Clone)]
struct VisualMove {
    #[cfg_attr(not(test), allow(dead_code))]
    uci: String,
    before: Chess,
    after: Chess,
    from: Square,
    to: Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayOutcome {
    CorrectIndependent,
    WrongIndependent,
    WrongReplay,
    ReplayAfterError,
    /// The move after a hint or a revealed alternative.
    PreviewedReplay,
    /// A new move played after it was shown.
    NewPlayed,
}

#[derive(Clone)]
enum CommitKind {
    Context,
    ReusedContext,
    Play {
        visual: Box<VisualMove>,
        outcome: PlayOutcome,
        shown_prefix: ShownPrefix,
    },
    /// A hint, or the move revealed after another repertoire move.
    Preview,
}

#[derive(Clone)]
struct ShownPrefix {
    repertoire_id: RepertoireId,
    path: Vec<String>,
}

#[derive(Clone)]
struct PendingCommit {
    attempts: Vec<TrainingAttempt>,
    kind: CommitKind,
    /// The question the event answers (for the session's result).
    target: Option<LearningSessionTarget>,
}

enum Playback {
    Automatic {
        current: Box<VisualMove>,
        remaining: VecDeque<LearningContextStep>,
        started_at: Instant,
        duration: Duration,
    },
    Wrong {
        visual: Box<VisualMove>,
        until: Instant,
    },
    Alternative {
        visual: Box<VisualMove>,
        until: Instant,
    },
    Accepted {
        started_at: Instant,
        duration: Duration,
        outcome: PlayOutcome,
    },
}

/// The numbers of the session's result.
#[derive(Default)]
struct Tally {
    first_time: usize,
    mistakes: usize,
    hints: usize,
    new_moves: usize,
    missed: Vec<LearningSessionTarget>,
}

impl Tally {
    fn any(&self) -> bool {
        self.first_time + self.mistakes + self.hints + self.new_moves > 0
    }

    fn miss(&mut self, target: Option<LearningSessionTarget>) {
        if let Some(target) = target
            && !self.missed.iter().any(|missed| missed.key == target.key)
        {
            self.missed.push(target);
        }
    }

    fn missed_key(&self, key: &LearningItemKey) -> bool {
        self.missed.iter().any(|missed| &missed.key == key)
    }
}

/// The states of spec §4.2 (S0 is the caller's: before the session exists).
#[derive(Clone, Debug, PartialEq)]
enum State {
    Watch,
    NextLine,
    New,
    Yours,
    YoursMore(String),
    Correct,
    Wrong,
    Alternative,
    GotIt,
    Hint,
    NewDone,
    SaveError(String),
    Failed(String),
    Summary,
}

/// Computed once when the result is shown.
struct Summary {
    next_review: String,
    more_new: usize,
}

pub struct OpeningSessionUi {
    library: Library,
    store: LearningStore,
    session: LearningSessionSnapshot,
    learning: LearningData,
    kind: SessionKind,
    presentation: Option<LearningPresentation>,
    completed_attempt_ids: BTreeSet<String>,
    pending_commit: Option<PendingCommit>,
    /// The showing of the new move on the board, recorded together with the move once
    /// it is played: a new move left unplayed stays new.
    unplayed_preview: Option<TrainingAttempt>,
    commit_in_flight: Option<mpsc::Receiver<Result<PersistedSessionAttempt, String>>>,
    #[cfg(test)]
    synchronous_commits: bool,
    save_error: Option<String>,
    error: Option<String>,
    board: BoardState,
    board_position: Chess,
    // Parsed once in the training-start worker, never in a frame or save callback.
    annotation_games: BTreeMap<RepertoireId, Game>,
    /// Active answers of this session's scope (a branch: of the branch).
    repertoire_answers: BTreeSet<LearningItemKey>,
    /// Active answers of the whole repertoire, for the result.
    repertoire_keys: BTreeSet<LearningItemKey>,
    /// The lines of a review, to count «line k of n».
    lines: Vec<Vec<String>>,
    line: usize,
    next_line_banner: bool,
    // The visible node is independent of the session's already-advanced target.
    displayed_node: Option<(RepertoireId, NodeId)>,
    board_shown: bool,
    pending_automatic: Option<VecDeque<LearningContextStep>>,
    shown_prefix: Option<ShownPrefix>,
    playback: Option<Playback>,
    needs_prepare: bool,
    animation: Duration,
    /// How long «Correct» and «Got it» stay after the move ([`FEEDBACK`]).
    feedback: Duration,
    ready_since: Option<Instant>,
    last_move: Option<(Square, Square)>,
    orientation: Color,
    /// The next frame puts the board in place without a slide (a skipped context).
    jump: bool,
    /// The move revealed after another repertoire move was played (S5a).
    alternative_revealed: bool,
    /// Left before the end: the result says «Session stopped».
    stopped: bool,
    exit_requested: bool,
    /// Leave at the next frame (an exit asked during a save, nothing answered).
    leave: bool,
    /// Started from the «Lines» screen: going back leads there.
    back_to_lines: bool,
    current_key: Option<LearningItemKey>,
    tally: Tally,
    summary: Option<Summary>,
    shown_state: State,
    pending_sounds: Vec<Sound>,
    #[cfg(test)]
    visual_log: Vec<String>,
}

impl OpeningSessionUi {
    /// A new session (never a resumed one: every answer of an earlier session is already
    /// recorded). Runs in a worker: it reads the store and parses the repertoire.
    pub fn new(
        library: Library,
        store: LearningStore,
        scope: StatsScope,
        kind: SessionKind,
    ) -> Result<Self, String> {
        let data = store.load().map_err(|error| error.to_string())?;
        let id = unique_id("learning-session");
        let srs = SrsSettings::default();
        let mut session = match &kind {
            SessionKind::Daily => LearningSessionSnapshot::new_daily(
                id,
                &library,
                &data.learning,
                scope,
                srs,
                layout::NEW_PER_SESSION,
            ),
            SessionKind::ReviewAll => LearningSessionSnapshot::new_single_pass_review(
                id,
                &library,
                &data.learning,
                scope,
                srs,
            ),
            SessionKind::Branch { through, .. } => LearningSessionSnapshot::new_branch_review(
                id,
                &library,
                &data.learning,
                scope,
                srs,
                through,
            ),
        }
        .map_err(|error| error.to_string())?;
        store
            .save_session(&mut session)
            .map_err(|error| error.to_string())?;
        Self::from_session(library, store, data.learning, session, kind)
    }

    /// «10 more new» after a finished daily session. Runs in a worker.
    pub fn more_new(
        library: Library,
        store: LearningStore,
        mut session: LearningSessionSnapshot,
        orientation: Color,
    ) -> Result<Self, String> {
        let data = store.load().map_err(|error| error.to_string())?;
        session
            .introduce_more(&library, &data.learning, layout::NEW_PER_SESSION)
            .map_err(|error| error.to_string())?;
        store
            .save_session(&mut session)
            .map_err(|error| error.to_string())?;
        let mut this =
            Self::from_session(library, store, data.learning, session, SessionKind::Daily)?;
        this.orientation = orientation;
        Ok(this)
    }

    fn from_session(
        library: Library,
        store: LearningStore,
        learning: LearningData,
        session: LearningSessionSnapshot,
        kind: SessionKind,
    ) -> Result<Self, String> {
        let completed_attempt_ids = learning
            .attempts
            .iter()
            .filter(|attempt| attempt.session_id == session.id())
            .map(|attempt| attempt.client_attempt_id.clone())
            .collect();
        let repertoire_answers = active_learning_keys(&library, &session.stats_scope)
            .map_err(|error| error.to_string())?;
        let mut whole = session.stats_scope.clone();
        whole.branch_root = None;
        let repertoire_keys = if whole == session.stats_scope {
            repertoire_answers.clone()
        } else {
            active_learning_keys(&library, &whole).map_err(|error| error.to_string())?
        };
        let annotation_games: BTreeMap<_, _> = session
            .stats_scope
            .repertoire_ids
            .iter()
            .filter_map(|id| {
                library
                    .repertoire(id)
                    .map(|repertoire| (id.clone(), repertoire.game()))
            })
            .collect();
        let lines = session
            .stats_scope
            .repertoire_ids
            .iter()
            .filter_map(|id| Some((library.repertoire(id)?, annotation_games.get(id)?)))
            .flat_map(|(repertoire, game)| {
                layout::lines(game, |path| repertoire.is_prefix_disabled(path))
            })
            .filter(|line| match &kind {
                SessionKind::Branch { through, .. } => line.starts_with(through),
                SessionKind::Daily | SessionKind::ReviewAll => true,
            })
            .collect();
        let orientation = layout::orientation(session.stats_scope.side);
        Ok(Self {
            library,
            store,
            session,
            learning,
            kind,
            presentation: None,
            completed_attempt_ids,
            pending_commit: None,
            unplayed_preview: None,
            commit_in_flight: None,
            #[cfg(test)]
            synchronous_commits: false,
            save_error: None,
            error: None,
            board: BoardState::default(),
            board_position: Chess::default(),
            annotation_games,
            repertoire_answers,
            repertoire_keys,
            lines,
            line: 0,
            next_line_banner: false,
            displayed_node: None,
            board_shown: false,
            pending_automatic: None,
            shown_prefix: None,
            playback: None,
            needs_prepare: true,
            animation: DEFAULT_ANIMATION,
            feedback: FEEDBACK,
            ready_since: None,
            last_move: None,
            orientation,
            jump: false,
            alternative_revealed: false,
            stopped: false,
            exit_requested: false,
            leave: false,
            back_to_lines: false,
            current_key: None,
            tally: Tally::default(),
            summary: None,
            shown_state: State::Watch,
            pending_sounds: Vec::new(),
            #[cfg(test)]
            visual_log: Vec::new(),
        })
    }

    pub fn has_unsaved(&self) -> bool {
        self.pending_commit.is_some() || self.save_error.is_some()
    }

    pub fn target_available(&self) -> bool {
        self.session.current_target().is_some()
    }

    pub fn set_back_to_lines(&mut self, back_to_lines: bool) {
        self.back_to_lines = back_to_lines;
    }

    pub fn orientation(&self) -> Color {
        self.orientation
    }

    /// The finished session, for «10 more new».
    pub fn snapshot(&self) -> &LearningSessionSnapshot {
        &self.session
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn session_id(&self) -> &str {
        self.session.id()
    }

    fn is_finished(&self) -> bool {
        self.session.phase == LearningSessionPhase::Finished
            && self.playback.is_none()
            && !self.has_unsaved()
    }

    fn summary_visible(&self) -> bool {
        (self.stopped || self.is_finished()) && !self.has_unsaved()
    }

    fn can_input(&self) -> bool {
        !self.has_unsaved()
            && !self.stopped
            && self.error.is_none()
            && !self.needs_prepare
            && self.playback.is_none()
            && self.pending_automatic.is_none()
            && self.session.current_target().is_some()
            && matches!(
                self.session.phase,
                LearningSessionPhase::AwaitingIndependent
                    | LearningSessionPhase::AwaitingContextIndependent
                    | LearningSessionPhase::AwaitingReplayAfterError
                    | LearningSessionPhase::PreviewedReplay
            )
    }

    fn can_hint(&self) -> bool {
        self.can_input()
            && self.session.phase == LearningSessionPhase::AwaitingIndependent
            && !self.session.current_is_new(&self.learning)
    }

    fn tick_at(&mut self, now: Instant, animation: Duration, happened_at: i64) {
        self.animation = animation;
        self.poll_commit(now);
        if self.stopped || self.has_unsaved() {
            return;
        }
        if self.board_shown
            && let Some(steps) = self.pending_automatic.take()
        {
            self.start_next_automatic(steps, now);
        }
        if self.needs_prepare {
            self.prepare_presentation(now);
        }
        loop {
            if self.stopped || self.has_unsaved() || self.error.is_some() {
                return;
            }
            if self.needs_prepare {
                self.prepare_presentation(now);
                continue;
            }
            let Some(deadline) = self.playback_deadline() else {
                return;
            };
            if now < deadline {
                return;
            }
            match self.playback {
                Some(Playback::Wrong { .. } | Playback::Alternative { .. }) => {
                    self.finish_temporary_move(now, happened_at);
                }
                Some(Playback::Accepted { .. }) => self.finish_accepted(now),
                Some(Playback::Automatic { .. }) => {
                    let id = unique_id(self.session.id());
                    self.advance_context_with_id_at(id, now, happened_at);
                }
                None => return,
            }
            if !animation.is_zero() {
                return;
            }
        }
    }

    fn playback_deadline(&self) -> Option<Instant> {
        match &self.playback {
            Some(Playback::Automatic {
                started_at,
                duration,
                ..
            })
            | Some(Playback::Accepted {
                started_at,
                duration,
                ..
            }) => Some(*started_at + *duration),
            Some(Playback::Wrong { until, .. } | Playback::Alternative { until, .. }) => {
                Some(*until)
            }
            None => None,
        }
    }

    fn prepare_presentation(&mut self, now: Instant) {
        let presentation = match self
            .session
            .presentation_with_learning(&self.library, &self.learning)
        {
            Ok(presentation) => presentation,
            Err(error) => {
                self.error = Some(error.to_string());
                self.needs_prepare = false;
                self.ready_since = None;
                return;
            }
        };
        self.error = None;
        self.needs_prepare = false;
        self.presentation = Some(presentation.clone());
        let Some(ref current) = presentation.current_position else {
            self.ready_since = None;
            self.pending_automatic = None;
            return;
        };
        let key = self
            .session
            .current_target()
            .map(|target| target.key.clone());
        if key != self.current_key {
            self.current_key = key;
            self.alternative_revealed = false;
            self.track_line();
        }
        // A following task may extend the line that is already on the board. Keep
        // its context journal, but do not rewind and animate those same moves again.
        // The complete path and repertoire must match; a transposed FEN is not enough.
        if self.reuse_shown_context(&presentation, now) {
            return;
        }
        self.shown_prefix = None;
        if !presentation.automatic_context.is_empty() {
            let mut steps = VecDeque::new();
            for step in presentation.automatic_context {
                match VisualMove::from_context(&step) {
                    Ok(_) => steps.push_back(step),
                    Err(error) => {
                        self.error = Some(error);
                        self.ready_since = None;
                        return;
                    }
                }
            }
            self.ready_since = None;
            let Some(first) = steps.front() else {
                self.error = Some("automatic context has no visual step".to_owned());
                return;
            };
            self.board_position = match parse_position(&first.from) {
                Ok(position) => position,
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            };
            self.set_displayed_context(self.session.context_index);
            self.board_shown = false;
            self.pending_automatic = Some(steps);
            self.playback = None;
            self.last_move = None;
            return;
        }
        self.pending_automatic = None;
        let position = match parse_position(current) {
            Ok(position) => position,
            Err(error) => {
                self.error = Some(error);
                self.ready_since = None;
                return;
            }
        };
        if fen4(&self.board_position) != current.fen4 {
            self.last_move = None;
        }
        self.board_position = position;
        self.set_displayed_context(self.session.context_index);
        self.playback = None;
        self.next_line_banner = false;
        match self.session.phase {
            LearningSessionPhase::AwaitingIndependent
                if self.session.current_is_new(&self.learning) =>
            {
                // A new move is shown before it is asked; the showing is recorded
                // only with the played move.
                self.ready_since = None;
                let id = unique_id(self.session.id());
                self.show_new_with_id_at(id, layout::now());
            }
            LearningSessionPhase::AwaitingIndependent
            | LearningSessionPhase::AwaitingContextIndependent => {
                self.ready_since = Some(now);
            }
            LearningSessionPhase::AwaitingReplayAfterError
            | LearningSessionPhase::PreviewedReplay => {
                self.ready_since = None;
            }
            phase => {
                self.ready_since = None;
                self.error = Some(format!(
                    "learning session has no playable position in {phase:?}"
                ));
            }
        }
    }

    /// The line of the current question (1-based), for a review.
    fn track_line(&mut self) {
        if self.kind == SessionKind::Daily {
            return;
        }
        let Some(target) = self.session.current_target() else {
            return;
        };
        let Some(index) = self
            .lines
            .iter()
            .position(|line| line.starts_with(&target.path))
        else {
            return;
        };
        let line = index + 1;
        if line > self.line {
            self.next_line_banner = self.line > 0;
            self.line = line;
        }
    }

    fn reuse_shown_context(&mut self, presentation: &LearningPresentation, at: Instant) -> bool {
        let Some(prefix) = &self.shown_prefix else {
            return false;
        };
        let Some(target) = self.session.current_target() else {
            return false;
        };
        let Some(remaining) = prefix.path.len().checked_sub(self.session.context_index) else {
            return false;
        };
        if remaining == 0
            || target.repertoire_id != prefix.repertoire_id
            || !presentation.target_path.starts_with(&prefix.path)
            // An independent context question must still be asked. Only reuse a
            // prefix that the core explicitly allows to be automatic in full.
            || presentation.automatic_context.len() < remaining
            || presentation.automatic_context[remaining - 1].to.fen4 != fen4(&self.board_position)
        {
            return false;
        }
        // These moves are already visible. Preserve each AUTO_CONTEXT event,
        // but commit the whole prefix once instead of rereading/writing all
        // history and rebuilding the presentation once per old move.
        self.commit_context_batch(remaining, at)
    }

    /// Records `count` automatic moves of the current line at once (moves already on the
    /// board, or skipped by the user).
    fn commit_context_batch(&mut self, count: usize, at: Instant) -> bool {
        let mut next_session = self.session.clone();
        let mut attempts = Vec::with_capacity(count);
        let happened_at = layout::now();
        for _ in 0..count {
            let id = unique_id(next_session.id());
            match next_session.advance_context_with_learning(
                &self.library,
                &self.learning,
                id,
                happened_at,
            ) {
                Ok(attempt) => attempts.push(attempt),
                Err(error) => {
                    self.error = Some(error.to_string());
                    return true;
                }
            }
        }
        if attempts.is_empty() {
            return false;
        }
        self.session = next_session;
        self.ready_since = None;
        self.pending_commit = Some(PendingCommit {
            attempts,
            kind: CommitKind::ReusedContext,
            target: None,
        });
        self.persist_pending(at);
        true
    }

    /// «Skip»: the position of the question at once, the skipped moves recorded as
    /// shown.
    fn skip_context(&mut self, now: Instant) -> bool {
        if self.has_unsaved() || self.stopped {
            return false;
        }
        let (count, last) = match (&self.playback, &self.pending_automatic) {
            (
                Some(Playback::Automatic {
                    current, remaining, ..
                }),
                _,
            ) => (
                remaining.len() + 1,
                remaining
                    .back()
                    .map(VisualMove::from_context)
                    .unwrap_or_else(|| Ok((**current).clone())),
            ),
            (None, Some(steps)) => match steps.back() {
                Some(step) => (steps.len(), VisualMove::from_context(step)),
                None => return false,
            },
            _ => return false,
        };
        let last = match last {
            Ok(last) => last,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        self.playback = None;
        self.pending_automatic = None;
        if !self.commit_context_batch(count, now) {
            return false;
        }
        self.board_position = last.after.clone();
        self.set_displayed_context(self.session.context_index);
        self.last_move = Some((last.from, last.to));
        self.jump = true;
        true
    }

    fn start_next_automatic(&mut self, mut remaining: VecDeque<LearningContextStep>, now: Instant) {
        let Some(step) = remaining.pop_front() else {
            self.needs_prepare = true;
            self.prepare_presentation(now);
            return;
        };
        let current = match VisualMove::from_context(&step) {
            Ok(current) => current,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.board_position = current.after.clone();
        self.set_displayed_context(self.session.context_index + 1);
        self.last_move = Some((current.from, current.to));
        self.playback = Some(Playback::Automatic {
            current: Box::new(current.clone()),
            remaining,
            started_at: now,
            duration: self.animation.div_f32(step.speed_factor.max(0.01) as f32),
        });
        #[cfg(test)]
        self.visual_log.push(current.uci.clone());
        self.queue_sound(Sound::Move);
    }

    fn advance_context_with_id_at(
        &mut self,
        client_attempt_id: String,
        now: Instant,
        happened_at: i64,
    ) -> bool {
        if self.has_unsaved()
            || !matches!(self.playback, Some(Playback::Automatic { .. }))
            || self.completed_attempt_ids.contains(&client_attempt_id)
        {
            return false;
        }
        let attempt = match self.session.advance_context_with_learning(
            &self.library,
            &self.learning,
            client_attempt_id,
            happened_at,
        ) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.error = Some(error.to_string());
                return false;
            }
        };
        self.begin_commit(attempt, CommitKind::Context, now);
        true
    }

    fn finish_context(&mut self, now: Instant) {
        let Some(Playback::Automatic { remaining, .. }) = self.playback.take() else {
            self.error = Some("automatic context is missing its visual step".to_owned());
            return;
        };
        self.start_next_automatic(remaining, now);
    }

    fn play_move_at(&mut self, mv: Move, now: Instant, happened_at: i64) -> bool {
        let id = unique_id(self.session.id());
        self.play_move_with_id_at(mv, id, now, happened_at)
    }

    fn play_move_with_id_at(
        &mut self,
        mv: Move,
        client_attempt_id: String,
        now: Instant,
        happened_at: i64,
    ) -> bool {
        if !self.can_input() || self.completed_attempt_ids.contains(&client_attempt_id) {
            return false;
        }
        let Some(expected_uci) = self.session.current_expected_uci().map(str::to_owned) else {
            return false;
        };
        let before = self.board_position.clone();
        let uci = mv.to_uci(CastlingMode::Standard).to_string();
        let visual = VisualMove::new(uci.clone(), before, mv);
        let phase = self.session.phase;
        let independent = matches!(
            phase,
            LearningSessionPhase::AwaitingIndependent
                | LearningSessionPhase::AwaitingContextIndependent
        );
        let showing_new = self.session.showing_new;
        let Some(target) = self.session.current_target().cloned() else {
            return false;
        };
        let Some(prefix) = target.path.get(..self.session.context_index) else {
            return false;
        };
        let mut path = prefix.to_vec();
        path.push(uci.clone());
        let shown_prefix = ShownPrefix {
            repertoire_id: target.repertoire_id.clone(),
            path,
        };
        if !independent && uci != expected_uci {
            let key = LearningItemKey::new(target.key.user_id.clone(), fen4(&visual.before), &uci);
            if self.repertoire_answers.contains(&key) {
                self.show_repertoire_alternative(visual, &shown_prefix, now);
                return true;
            }
            self.set_displayed_path(Some(&shown_prefix));
            self.start_playback(visual, PlayOutcome::WrongReplay, now);
            return true;
        }
        let response_time_ms = independent.then(|| {
            self.ready_since.map_or(0, |ready| {
                u64::try_from(now.saturating_duration_since(ready).as_millis()).unwrap_or(u64::MAX)
            })
        });
        let attempt = match self.session.play_uci_with_learning_answers(
            &self.library,
            &self.learning,
            &self.repertoire_answers,
            &uci,
            client_attempt_id,
            happened_at,
            response_time_ms,
        ) {
            Ok(attempt) => attempt,
            Err(LearningSessionError::RepertoireAlternative { .. }) => {
                self.show_repertoire_alternative(visual, &shown_prefix, now);
                return true;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                return false;
            }
        };
        let outcome = match attempt.event_type {
            TrainingEventType::CorrectIndependent => PlayOutcome::CorrectIndependent,
            TrainingEventType::IncorrectIndependent => PlayOutcome::WrongIndependent,
            TrainingEventType::ReplayAfterError => PlayOutcome::ReplayAfterError,
            TrainingEventType::AutoContext if showing_new => PlayOutcome::NewPlayed,
            TrainingEventType::AutoContext => PlayOutcome::PreviewedReplay,
            TrainingEventType::Previewed | TrainingEventType::IllegalMove => {
                self.error = Some("legal board input produced a non-play event".to_owned());
                return false;
            }
        };
        // The credited question: the played answer itself when it was another
        // pending answer of this position.
        let credited = if attempt.item_key() == target.key {
            Some(target)
        } else {
            self.session
                .queue
                .iter()
                .find(|candidate| candidate.key == attempt.item_key())
                .cloned()
        };
        // The legal move is visible immediately, including while its event is
        // being saved. Input stays blocked until persistence acknowledges it.
        // In particular a dropped piece must not jump back to its source square.
        self.board_position = visual.after.clone();
        self.set_displayed_path(Some(&shown_prefix));
        self.last_move = Some((visual.from, visual.to));
        self.begin_commit_for(
            attempt,
            CommitKind::Play {
                visual: Box::new(visual),
                outcome,
                shown_prefix,
            },
            credited,
            now,
        );
        true
    }

    fn play_uci_with_id_at(
        &mut self,
        uci: &str,
        client_attempt_id: String,
        now: Instant,
        happened_at: i64,
    ) -> bool {
        if !self.can_input() || self.completed_attempt_ids.contains(&client_attempt_id) {
            return false;
        }
        let mv = uci
            .parse::<UciMove>()
            .ok()
            .and_then(|uci| uci.to_move(&self.board_position).ok());
        let Some(mv) = mv else {
            return false;
        };
        self.play_move_with_id_at(mv, client_attempt_id, now, happened_at)
    }

    fn start_playback(&mut self, visual: VisualMove, outcome: PlayOutcome, now: Instant) {
        self.ready_since = None;
        self.board_position = visual.after.clone();
        self.last_move = Some((visual.from, visual.to));
        match outcome {
            PlayOutcome::WrongIndependent | PlayOutcome::WrongReplay => {
                self.playback = Some(Playback::Wrong {
                    visual: Box::new(visual),
                    until: now + WRONG_SHOWN,
                });
                self.queue_sound(Sound::Wrong);
            }
            PlayOutcome::CorrectIndependent
            | PlayOutcome::ReplayAfterError
            | PlayOutcome::PreviewedReplay
            | PlayOutcome::NewPlayed => {
                self.playback = Some(Playback::Accepted {
                    started_at: now,
                    duration: self.animation + self.feedback,
                    outcome,
                });
                self.queue_sound(Sound::Move);
            }
        }
    }

    fn show_repertoire_alternative(
        &mut self,
        visual: VisualMove,
        shown_prefix: &ShownPrefix,
        now: Instant,
    ) {
        self.ready_since = None;
        self.board_position = visual.after.clone();
        self.set_displayed_path(Some(shown_prefix));
        self.last_move = Some((visual.from, visual.to));
        self.playback = Some(Playback::Alternative {
            visual: Box::new(visual),
            until: now + WRONG_SHOWN.max(self.animation),
        });
    }

    fn finish_temporary_move(&mut self, now: Instant, happened_at: i64) {
        let (visual, alternative) = match self.playback.take() {
            Some(Playback::Wrong { visual, .. }) => (visual, false),
            Some(Playback::Alternative { visual, .. }) => (visual, true),
            _ => return,
        };
        self.board_position = visual.before;
        self.last_move = None;
        self.needs_prepare = true;
        self.prepare_presentation(now);
        // Another move of the repertoire is no mistake, but «now play {san}» shows the
        // answer: it is recorded as shown, not as known.
        if alternative && self.session.phase == LearningSessionPhase::AwaitingIndependent {
            self.alternative_revealed = true;
            let id = unique_id(self.session.id());
            self.preview_with_id_at(id, now, happened_at);
        } else if alternative {
            self.alternative_revealed = true;
        }
    }

    fn finish_accepted(&mut self, now: Instant) {
        let Some(Playback::Accepted { outcome, .. }) = self.playback.take() else {
            return;
        };
        if outcome == PlayOutcome::CorrectIndependent
            && self.session.phase == LearningSessionPhase::Finished
        {
            self.queue_sound(Sound::Success);
        }
        self.alternative_revealed = false;
        self.needs_prepare = true;
        self.prepare_presentation(now);
    }

    fn request_hint_with_id_at(
        &mut self,
        client_attempt_id: String,
        now: Instant,
        happened_at: i64,
    ) -> bool {
        if !self.can_hint() || self.completed_attempt_ids.contains(&client_attempt_id) {
            return false;
        }
        self.preview_with_id_at(client_attempt_id, now, happened_at)
    }

    /// S2: the new move goes on the board with its arrow. Nothing is written yet.
    fn show_new_with_id_at(&mut self, client_attempt_id: String, happened_at: i64) {
        if self.has_unsaved() || self.completed_attempt_ids.contains(&client_attempt_id) {
            return;
        }
        match self.session.preview_new(client_attempt_id, happened_at) {
            Ok(attempt) => {
                self.unplayed_preview = attempt;
                self.board.clear();
            }
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        }
        // The arrow comes from the presentation of the shown move.
        match self
            .session
            .presentation_with_learning(&self.library, &self.learning)
        {
            Ok(presentation) => self.presentation = Some(presentation),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn preview_with_id_at(
        &mut self,
        client_attempt_id: String,
        now: Instant,
        happened_at: i64,
    ) -> bool {
        if self.has_unsaved() || self.completed_attempt_ids.contains(&client_attempt_id) {
            return false;
        }
        let target = self.session.current_target().cloned();
        let attempt = match self.session.preview(client_attempt_id, happened_at) {
            Ok(Some(attempt)) => attempt,
            Ok(None) => return false,
            Err(error) => {
                self.error = Some(error.to_string());
                return false;
            }
        };
        self.board.clear();
        self.begin_commit_for(attempt, CommitKind::Preview, target, now);
        true
    }

    fn begin_commit(&mut self, attempt: TrainingAttempt, kind: CommitKind, now: Instant) {
        self.begin_commit_for(attempt, kind, None, now);
    }

    fn begin_commit_for(
        &mut self,
        attempt: TrainingAttempt,
        kind: CommitKind,
        target: Option<LearningSessionTarget>,
        now: Instant,
    ) {
        if self.pending_commit.is_some() {
            self.save_error =
                Some("another learning event is still waiting to be saved".to_owned());
            return;
        }
        let mut attempts: Vec<_> = self.unplayed_preview.take().into_iter().collect();
        attempts.push(attempt);
        self.pending_commit = Some(PendingCommit {
            attempts,
            kind,
            target,
        });
        self.persist_pending(now);
    }

    fn persist_pending(&mut self, _now: Instant) {
        let Some(pending) = self.pending_commit.clone() else {
            return;
        };
        if self.commit_in_flight.is_some() {
            return;
        }
        #[cfg(test)]
        if self.synchronous_commits {
            let result = self
                .store
                .apply_attempts_and_session(pending.attempts.clone(), &mut self.session)
                .map_err(|error| error.to_string());
            self.finish_commit(result, _now);
            return;
        }
        let store = self.store.clone();
        let mut session = self.session.clone();
        let (tx, rx) = mpsc::channel();
        self.commit_in_flight = Some(rx);
        self.save_error = None;
        if let Err(error) = crate::platform::spawn("learning-save-attempt", move || {
            let result = store
                .apply_attempts_and_session(pending.attempts, &mut session)
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        }) {
            self.commit_in_flight = None;
            self.save_error = Some(error.to_string());
            self.ready_since = None;
        }
    }

    fn poll_commit(&mut self, now: Instant) {
        let Some(receiver) = &self.commit_in_flight else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("learning save worker disconnected".to_owned())
            }
        };
        self.commit_in_flight = None;
        self.finish_commit(result, now);
    }

    fn finish_commit(&mut self, result: Result<PersistedSessionAttempt, String>, now: Instant) {
        let Some(pending) = self.pending_commit.clone() else {
            return;
        };
        match result {
            Ok(persisted) => {
                let duplicate = persisted.application == AttemptApplication::Duplicate;
                self.learning = persisted.learning;
                self.session = persisted.session;
                self.completed_attempt_ids.extend(
                    pending
                        .attempts
                        .iter()
                        .map(|attempt| attempt.client_attempt_id.clone()),
                );
                self.pending_commit = None;
                self.save_error = None;
                if duplicate {
                    self.playback = None;
                    self.shown_prefix = None;
                    self.ready_since = None;
                    self.needs_prepare = true;
                    self.prepare_presentation(now);
                    self.finish_exit();
                    return;
                }
                self.count(&pending);
                match pending.kind {
                    CommitKind::Context => self.finish_context(now),
                    CommitKind::ReusedContext => self.needs_prepare = true,
                    CommitKind::Play {
                        visual,
                        outcome,
                        shown_prefix,
                    } => {
                        self.shown_prefix = (!matches!(
                            outcome,
                            PlayOutcome::WrongIndependent | PlayOutcome::WrongReplay
                        ))
                        .then_some(shown_prefix);
                        self.start_playback(*visual, outcome, now);
                    }
                    CommitKind::Preview => {
                        self.needs_prepare = true;
                        self.prepare_presentation(now);
                    }
                }
                self.finish_exit();
            }
            Err(error) => {
                self.save_error = Some(error.to_string());
                self.ready_since = None;
            }
        }
    }

    /// The session's result counts recorded events only.
    fn count(&mut self, pending: &PendingCommit) {
        let target = pending.target.clone();
        match &pending.kind {
            CommitKind::Play { outcome, .. } => match outcome {
                PlayOutcome::CorrectIndependent => {
                    if target
                        .as_ref()
                        .is_none_or(|target| !self.tally.missed_key(&target.key))
                    {
                        self.tally.first_time += 1;
                    }
                }
                PlayOutcome::WrongIndependent => {
                    self.tally.mistakes += 1;
                    self.tally.miss(target);
                }
                PlayOutcome::NewPlayed => self.tally.new_moves += 1,
                PlayOutcome::WrongReplay
                | PlayOutcome::ReplayAfterError
                | PlayOutcome::PreviewedReplay => {}
            },
            CommitKind::Preview => {
                self.tally.hints += 1;
                self.tally.miss(target);
            }
            CommitKind::Context | CommitKind::ReusedContext => {}
        }
    }

    fn retry_pending(&mut self, now: Instant) {
        if self.pending_commit.is_some() {
            self.persist_pending(now);
        }
    }

    fn queue_sound(&mut self, sound: Sound) {
        if sound == Sound::Move && self.pending_sounds.last() == Some(&Sound::Move) {
            return;
        }
        self.pending_sounds.push(sound);
    }

    /// Esc, «⬅», «× Finish»: every answer is already recorded, so no question; after
    /// an answer the result shows what was done.
    fn request_exit(&mut self, actions: &mut Vec<SessionAction>) {
        if self.summary_visible() {
            actions.push(SessionAction::Back);
            return;
        }
        if self.save_error.is_some() {
            // «Finish» in S9 gives up the one answer that could not be saved.
            self.pending_commit = None;
            self.save_error = None;
            self.commit_in_flight = None;
        }
        if self.has_unsaved() {
            self.exit_requested = true;
            return;
        }
        if self.tally.any() {
            self.stop();
        } else {
            actions.push(SessionAction::Back);
        }
    }

    fn stop(&mut self) {
        self.stopped = true;
        self.unplayed_preview = None;
        self.pending_automatic = None;
        if let Some(Playback::Automatic { .. }) = self.playback {
            self.playback = None;
        }
        self.ready_since = None;
    }

    /// An exit asked while an answer was being saved.
    fn finish_exit(&mut self) {
        if std::mem::take(&mut self.exit_requested) {
            if self.tally.any() {
                self.stop();
            } else {
                self.leave = true;
            }
        }
    }

    fn keyboard(&mut self, ui: &Ui, actions: &mut Vec<SessionAction>) {
        if ui.ctx().egui_wants_keyboard_input() || crate::app::overlay_open(ui.ctx()) {
            return;
        }
        let state = self.shown_state.clone();
        let more = self.more_new_count() > 0;
        let busy = self.board.is_busy();
        let mut command = None;
        ui.input_mut(|input| {
            if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W))
                || (!busy && input.consume_key(Modifiers::NONE, Key::Escape))
            {
                command = Some("exit");
            } else if input.consume_key(Modifiers::NONE, Key::F) {
                command = Some("flip");
            } else if input.consume_key(Modifiers::NONE, Key::H) {
                command = Some("hint");
            } else if state == State::Summary && more && input.consume_key(Modifiers::NONE, Key::N)
            {
                command = Some("more");
            } else if matches!(
                state,
                State::Watch | State::NextLine | State::Summary | State::SaveError(_)
            ) && (input.consume_key(Modifiers::NONE, Key::Enter)
                || input.consume_key(Modifiers::NONE, Key::Space))
            {
                command = Some("main");
            }
        });
        match command {
            Some("exit") => self.request_exit(actions),
            Some("flip") => self.orientation = self.orientation.other(),
            Some("hint") => {
                let id = unique_id(self.session.id());
                self.request_hint_with_id_at(id, Instant::now(), layout::now());
            }
            Some("more") => actions.push(SessionAction::MoreNew),
            Some("main") => self.main_action(&state, actions),
            _ => {}
        }
    }

    fn main_action(&mut self, state: &State, actions: &mut Vec<SessionAction>) {
        match state {
            State::Watch | State::NextLine => {
                self.skip_context(Instant::now());
            }
            State::Summary => actions.push(SessionAction::Back),
            State::SaveError(_) => self.retry_pending(Instant::now()),
            _ => {}
        }
    }

    fn board_animation_seconds(&mut self, settings: &Settings) -> f32 {
        if self.pending_automatic.is_some() || std::mem::take(&mut self.jump) {
            return 0.0;
        }
        match &self.playback {
            Some(Playback::Automatic { duration, .. }) => duration.as_secs_f32(),
            _ => settings.animation.seconds(),
        }
    }

    fn set_displayed_context(&mut self, played: usize) {
        let prefix = self.session.current_target().and_then(|target| {
            Some(ShownPrefix {
                repertoire_id: target.repertoire_id.clone(),
                path: target.path.get(..played)?.to_vec(),
            })
        });
        self.set_displayed_path(prefix.as_ref());
    }

    fn set_displayed_path(&mut self, prefix: Option<&ShownPrefix>) {
        self.displayed_node = prefix.and_then(|prefix| {
            let game = self.annotation_games.get(&prefix.repertoire_id)?;
            let at = layout::node_at(game, &prefix.path)?;
            Some((prefix.repertoire_id.clone(), at))
        });
    }

    /// The node on the board, when the board really shows it.
    fn shown_node(&self) -> Option<(&Game, NodeId)> {
        let (id, at) = self.displayed_node.as_ref()?;
        let game = self.annotation_games.get(id)?;
        // A visual wrong move outside the source tree must never inherit the
        // expected move's badge or arrows merely because the task advanced.
        (game.position(*at)? == &self.board_position).then_some((game, *at))
    }

    fn displayed_annotations(&self) -> BoardAnnotations {
        match self.shown_node() {
            Some((game, at)) => pgn_annotations_at(game, at),
            None => BoardAnnotations::default(),
        }
    }

    /// The asked move's node in the PGN.
    fn expected_node(&self) -> Option<(&Game, NodeId)> {
        let target = self.session.current_target()?;
        let game = self.annotation_games.get(&target.repertoire_id)?;
        let path = target.path.get(..=self.session.context_index)?;
        Some((game, layout::node_at(game, path)?))
    }

    fn expected_text(&self) -> String {
        self.expected_node()
            .and_then(|(game, node)| layout::move_text(game, node))
            .unwrap_or_default()
    }

    /// «4. Ba4 !» of the move just played.
    fn played_text(&self, dark: bool) -> String {
        let Some((game, node)) = self.shown_node() else {
            return String::new();
        };
        let mut text = layout::move_text(game, node).unwrap_or_default();
        if let Some((symbol, _)) = layout::assessment(game, node, dark) {
            text.push_str(symbol);
        }
        text
    }

    /// The move shown by an arrow: revealed by the core, or a new move about to be shown.
    fn revealed_uci(&self) -> Option<String> {
        if self.playback.is_some() || self.pending_automatic.is_some() {
            return None;
        }
        if let Some(uci) = self
            .presentation
            .as_ref()
            .and_then(|presentation| presentation.revealed_move.clone())
            && self.session.phase != LearningSessionPhase::AwaitingIndependent
        {
            return Some(uci);
        }
        matches!(self.shown_state, State::New)
            .then(|| self.session.current_expected_uci().map(str::to_owned))
            .flatten()
    }

    fn state(&self) -> Option<State> {
        if let Some(error) = &self.save_error {
            return Some(State::SaveError(error.clone()));
        }
        if let Some(error) = &self.error {
            return Some(State::Failed(error.clone()));
        }
        if self.summary_visible() {
            return Some(State::Summary);
        }
        match &self.playback {
            Some(Playback::Automatic { .. }) => return Some(self.watch()),
            Some(Playback::Wrong { .. }) => return Some(State::Wrong),
            Some(Playback::Alternative { .. }) => return Some(State::Alternative),
            Some(Playback::Accepted { outcome, .. }) => {
                return Some(match outcome {
                    PlayOutcome::CorrectIndependent => State::Correct,
                    PlayOutcome::NewPlayed => State::NewDone,
                    PlayOutcome::ReplayAfterError
                    | PlayOutcome::PreviewedReplay
                    | PlayOutcome::WrongIndependent
                    | PlayOutcome::WrongReplay => State::GotIt,
                });
            }
            None => {}
        }
        if self.pending_automatic.is_some() {
            return Some(self.watch());
        }
        if self.pending_commit.is_some() {
            // Between moves: the card keeps what it said.
            return None;
        }
        if self.needs_prepare {
            return None;
        }
        Some(match self.session.phase {
            LearningSessionPhase::AwaitingIndependent
                if self.session.current_is_new(&self.learning) =>
            {
                State::New
            }
            LearningSessionPhase::AwaitingIndependent
            | LearningSessionPhase::AwaitingContextIndependent => self.yours(),
            LearningSessionPhase::AwaitingReplayAfterError => State::Wrong,
            LearningSessionPhase::PreviewedReplay if self.session.showing_new => State::New,
            LearningSessionPhase::PreviewedReplay if self.alternative_revealed => {
                State::Alternative
            }
            LearningSessionPhase::PreviewedReplay => State::Hint,
            LearningSessionPhase::AdvancingContext => self.watch(),
            LearningSessionPhase::Finished => State::Summary,
        })
    }

    fn watch(&self) -> State {
        if self.next_line_banner {
            State::NextLine
        } else {
            State::Watch
        }
    }

    /// S3, or S3a when another answer of this position was already played.
    fn yours(&self) -> State {
        let position = fen4(&self.board_position);
        let played: Vec<String> = self
            .session
            .completed_keys
            .iter()
            .filter(|key| key.position_fen4 == position && self.repertoire_answers.contains(key))
            .filter_map(|key| {
                let mv = key
                    .expected_uci
                    .parse::<UciMove>()
                    .ok()?
                    .to_move(&self.board_position)
                    .ok()?;
                let san = chess_core::shakmaty::san::San::from_move(&self.board_position, mv);
                Some(crate::san::show(&san.to_string()).into_owned())
            })
            .collect();
        if played.is_empty() {
            State::Yours
        } else {
            State::YoursMore(played.join(", "))
        }
    }

    fn more_new_count(&self) -> usize {
        if self.kind != SessionKind::Daily {
            return 0;
        }
        self.summary.as_ref().map_or(0, |summary| summary.more_new)
    }

    fn compute_summary(&mut self) {
        if self.summary.is_some() {
            return;
        }
        let items: HashMap<&LearningItemKey, &LearningItem> = self
            .learning
            .items
            .iter()
            .map(|item| (&item.key, item))
            .collect();
        let today = layout::end_of_today();
        let now = layout::now();
        let mut due_today = 0usize;
        let mut next: Option<i64> = None;
        let mut unseen = 0usize;
        for key in &self.repertoire_keys {
            match items.get(key) {
                Some(item) if item.introduced => {
                    if item.is_due(today) {
                        due_today += 1;
                    } else if let Some(at) = item.next_review_at {
                        next = Some(next.map_or(at, |old| old.min(at)));
                    }
                }
                _ => unseen += 1,
            }
        }
        let next_review = if due_today > 0 {
            trf("today, {n} more", &[("n", &due_today)])
        } else {
            next.map(|at| layout::when(at, now)).unwrap_or_default()
        };
        self.summary = Some(Summary {
            next_review,
            more_new: unseen.min(layout::NEW_PER_SESSION),
        });
    }

    fn board_ui(&mut self, ui: &mut Ui, settings: &Settings) {
        let interactive = self.can_input();
        let annotations = self.displayed_annotations();
        let mut arrows = annotations.arrows;
        if let Some(mv) = self.revealed_uci().and_then(|uci| {
            uci.parse::<UciMove>()
                .ok()
                .and_then(|uci| uci.to_move(&self.board_position).ok())
        }) {
            arrows.push(Arrow {
                from: mv.from().unwrap_or_else(|| mv.to()),
                to: crate::board::ui_target(&mv),
                color: Color32::from_rgba_unmultiplied(21, 120, 27, 190),
                weight: 1.0,
            });
        }
        let alert = match &self.playback {
            Some(Playback::Wrong { visual, .. }) => Some((visual.from, visual.to)),
            _ => None,
        };
        let animation = self.board_animation_seconds(settings);
        let played = BoardView {
            pos: &self.board_position,
            orientation: self.orientation,
            last_move: self.last_move,
            arrows: &arrows,
            circles: &annotations.circles,
            badge: annotations.badge,
            interactive,
            show_coords: settings.show_coords,
            theme: settings.board_theme.theme(),
            pieces: settings.piece_set,
            animation,
            premove: None,
            auto_queen: false,
            alert,
        }
        .show(ui, &mut self.board);
        if let Some(mv) = played {
            self.play_move_at(mv, Instant::now(), layout::now());
        }
        self.board_shown = true;
    }

    /// The card's texts: title, instruction, title colour.
    fn card_texts(&self, state: &State, dark: bool) -> (String, String, Option<Color32>) {
        let dash = |s: &str| s.to_owned();
        let review = self.kind != SessionKind::Daily;
        match state {
            State::Watch => (
                dash(tr("Watch")),
                dash(tr("Moves up to your next question")),
                None,
            ),
            State::NextLine => (
                dash(tr("Next line")),
                trf("{k} of {n}", &[("k", &self.line), ("n", &self.lines.len())]),
                None,
            ),
            State::New => (
                dash(tr("New move")),
                trf(
                    "Play {san} — it is shown by the arrow",
                    &[("san", &self.expected_text())],
                ),
                None,
            ),
            State::Yours => (
                dash(tr("Your move")),
                dash(tr("Play the move from your repertoire")),
                None,
            ),
            State::YoursMore(played) => (
                dash(tr("Your move")),
                trf("One more move of yours here, not {san}", &[("san", played)]),
                None,
            ),
            State::Correct => (
                dash(tr("Correct!")),
                self.played_text(dark),
                crate::notation::assessment_text_color(chess_core::Nag(1), dark),
            ),
            State::NewDone => (dash(tr("New move")), self.played_text(dark), None),
            State::Wrong => (
                dash(tr("Not this move")),
                trf("Correct: {san}. Play it", &[("san", &self.expected_text())]),
                None,
            ),
            State::Alternative => (
                dash(tr("This one is in your repertoire too")),
                trf(
                    "Now the other one: play {san}",
                    &[("san", &self.expected_text())],
                ),
                None,
            ),
            State::GotIt => (
                dash(tr("Got it")),
                if review {
                    self.played_text(dark)
                } else {
                    trf(
                        "{move} will come back in this session",
                        &[("move", &self.played_text(dark))],
                    )
                },
                None,
            ),
            State::Hint => (
                dash(tr("Hint")),
                if review {
                    trf("Play {san}", &[("san", &self.expected_text())])
                } else {
                    trf(
                        "Play {san}. It will come back in this session",
                        &[("san", &self.expected_text())],
                    )
                },
                None,
            ),
            State::SaveError(error) => {
                (dash(tr("Could not save your answer")), error.clone(), None)
            }
            State::Failed(error) => (
                dash(tr("Could not continue the session")),
                error.clone(),
                None,
            ),
            State::Summary => (
                dash(if self.stopped {
                    tr("Session stopped")
                } else {
                    tr("Session complete")
                }),
                dash(tr("Everything is saved")),
                None,
            ),
        }
    }

    fn counter(&self, state: &State) -> (String, &'static str) {
        match &self.kind {
            SessionKind::Daily => {
                let left = if matches!(state, State::Summary) && !self.stopped {
                    0
                } else {
                    self.session.remaining()
                };
                (left.to_string(), tr("left"))
            }
            SessionKind::ReviewAll | SessionKind::Branch { .. } => {
                let k = if self.is_finished() {
                    self.lines.len()
                } else {
                    self.line.max(1).min(self.lines.len())
                };
                (format!("{k}/{}", self.lines.len()), tr("line"))
            }
        }
    }

    fn state_card(&self, ui: &mut Ui, state: &State) {
        let dark = ui.visuals().dark_mode;
        let (title, instruction, title_color) = self.card_texts(state, dark);
        let active = matches!(state, State::New | State::Yours | State::YoursMore(_));
        let alert = matches!(state, State::Wrong | State::SaveError(_) | State::Failed(_));
        let (count, label) = self.counter(state);
        show_card(
            ui,
            &CardView {
                title,
                instruction,
                title_color,
                active,
                alert,
                count,
                label,
            },
        );
    }

    /// Block D: the comment of the last move on the board (in S2, S5, S7 already of
    /// the asked move), or the nearest earlier one of the line.
    fn comment(&self, ui: &mut Ui, state: &State) {
        let start = if matches!(
            state,
            State::New | State::Wrong | State::Hint | State::Alternative
        ) {
            self.expected_node()
        } else {
            self.shown_node()
        };
        let Some((game, start)) = start else {
            return;
        };
        let dark = ui.visuals().dark_mode;
        let mut path = game.path(start);
        while let Some(node) = path.pop() {
            let Some(data) = game.node(node) else {
                continue;
            };
            let text = crate::notation::comment_text(data.comments_after());
            if text.trim().is_empty() {
                continue;
            }
            egui::ScrollArea::vertical()
                .id_salt("opening_comment")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(label) = layout::move_text(game, node) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.label(RichText::new(label).strong());
                            if let Some((symbol, color)) = layout::assessment(game, node, dark) {
                                ui.label(RichText::new(symbol).strong().color(color));
                            }
                        });
                    }
                    ui.add(egui::Label::new(text).wrap());
                });
            return;
        }
    }

    fn summary_body(&self, ui: &mut Ui) {
        let row = |ui: &mut Ui, label: &str, value: String| {
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let width = (ui.available_width() - 60.0).max(0.0);
                    ui.allocate_ui_with_layout(
                        vec2(width, 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| layout::line(ui, label),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        layout::line(ui, RichText::new(value).strong());
                    });
                },
            );
        };
        egui::ScrollArea::vertical()
            .id_salt("opening_summary")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if self.kind != SessionKind::Daily {
                    let k = if self.stopped {
                        self.line.saturating_sub(1)
                    } else {
                        self.lines.len()
                    };
                    row(
                        ui,
                        tr("Lines reviewed"),
                        trf("{k} of {n}", &[("k", &k), ("n", &self.lines.len())]),
                    );
                }
                row(
                    ui,
                    tr("Correct first time"),
                    self.tally.first_time.to_string(),
                );
                row(ui, tr("Mistakes"), self.tally.mistakes.to_string());
                if self.tally.hints > 0 {
                    row(ui, tr("Hints"), self.tally.hints.to_string());
                }
                if self.tally.new_moves > 0 {
                    row(
                        ui,
                        tr("New moves learned"),
                        self.tally.new_moves.to_string(),
                    );
                }
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                if let Some(summary) = &self.summary
                    && !summary.next_review.is_empty()
                {
                    row(ui, tr("Next review"), summary.next_review.clone());
                }
                if self.tally.missed.is_empty() {
                    return;
                }
                ui.add_space(8.0);
                ui.label(RichText::new(tr("Moves to remember")).strong());
                ui.add_space(2.0);
                for target in self.tally.missed.iter().take(REMEMBER_ROWS) {
                    let Some(game) = self.annotation_games.get(&target.repertoire_id) else {
                        continue;
                    };
                    let Some(node) = layout::node_at(game, &target.path) else {
                        continue;
                    };
                    let after = game
                        .parent(node)
                        .and_then(|parent| layout::move_text(game, parent));
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(layout::move_text(game, node).unwrap_or_default())
                                    .strong(),
                            );
                            if let Some(after) = after {
                                layout::line(
                                    ui,
                                    RichText::new(trf("after {path}", &[("path", &after)])).weak(),
                                );
                            }
                        },
                    );
                }
                if self.tally.missed.len() > REMEMBER_ROWS {
                    ui.label(
                        RichText::new(trf(
                            "and {k} more",
                            &[("k", &(self.tally.missed.len() - REMEMBER_ROWS))],
                        ))
                        .weak(),
                    );
                }
            });
    }

    fn side(&mut self, ui: &mut Ui, state: &State, actions: &mut Vec<SessionAction>) {
        let opening = self
            .shown_node()
            .map(|(game, node)| layout::opening_text(game, node))
            .unwrap_or_default();
        let (left, right) = match state {
            State::Summary => {
                let more = self.more_new_count();
                (
                    (more > 0).then(|| {
                        (
                            if more == layout::NEW_PER_SESSION {
                                tr("10 more new (N)").to_owned()
                            } else {
                                trf("{n} more new (N)", &[("n", &more)])
                            },
                            true,
                        )
                    }),
                    Some((
                        if self.back_to_lines {
                            tr("To lines (Enter)")
                        } else {
                            tr("To repertoires (Enter)")
                        }
                        .to_owned(),
                        true,
                    )),
                )
            }
            State::SaveError(_) => (
                Some((tr("Finish").to_owned(), true)),
                Some((tr("Try again").to_owned(), true)),
            ),
            State::Watch | State::NextLine => (
                Some((tr("Hint (H)").to_owned(), false)),
                Some((tr("Skip (Enter)").to_owned(), false)),
            ),
            _ => (Some((tr("Hint (H)").to_owned(), self.can_hint())), None),
        };
        let clicked = side_panel(
            ui,
            |ui| self.state_card(ui, state),
            &opening,
            |ui| {
                if *state == State::Summary {
                    self.summary_body(ui);
                } else {
                    self.comment(ui, state);
                }
            },
            left,
            right,
            *state == State::Summary || matches!(state, State::SaveError(_)),
        );
        match clicked {
            Some(Clicked::Left) => match state {
                State::Summary => actions.push(SessionAction::MoreNew),
                State::SaveError(_) => self.request_exit(actions),
                _ => {
                    let id = unique_id(self.session.id());
                    self.request_hint_with_id_at(id, Instant::now(), layout::now());
                }
            },
            Some(Clicked::Right) => self.main_action(state, actions),
            Some(Clicked::Flip) => self.orientation = self.orientation.other(),
            Some(Clicked::Finish) => self.request_exit(actions),
            None => {}
        }
    }

    pub fn show(&mut self, ui: &mut Ui, settings: &Settings, name: &str) -> Vec<SessionAction> {
        let animation = Duration::from_secs_f32(settings.animation.seconds());
        self.tick_at(Instant::now(), animation, layout::now());
        if self.commit_in_flight.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        } else if let Some(deadline) = self.playback_deadline() {
            ui.ctx()
                .request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        } else if self.needs_prepare || self.pending_automatic.is_some() {
            ui.ctx().request_repaint();
        }
        if let Some(state) = self.state() {
            self.shown_state = state;
        }
        if self.shown_state == State::Summary {
            self.compute_summary();
        }
        let mut actions = Vec::new();
        self.keyboard(ui, &mut actions);
        let state = self.shown_state.clone();
        let subtitle = self.kind.subtitle(self.session.stats_scope.side);
        let back = if ui.available_width() < 600.0 {
            "⬅"
        } else if self.back_to_lines {
            tr("⬅ Lines")
        } else {
            tr("⬅ Repertoires")
        };
        let mut header = (false, false);
        layout::screen(
            ui,
            self,
            |_, ui| header = crate::widgets::section_header_sub(ui, back, name, &subtitle),
            |this, ui| this.side(ui, &state, &mut actions),
            |this, ui| this.board_ui(ui, settings),
        );
        if header.0 {
            self.request_exit(&mut actions);
        }
        if header.1 {
            actions.push(SessionAction::OpenSettings);
        }
        if std::mem::take(&mut self.leave) {
            actions.push(SessionAction::Back);
        }
        actions.extend(self.pending_sounds.drain(..).map(SessionAction::Sound));
        actions
    }

    pub fn debug_settle(&mut self) {
        // Only this explicit QA hook waits. Normal frames poll workers without blocking.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.commit_in_flight.is_some() {
                self.poll_commit(Instant::now());
                if Instant::now() >= deadline {
                    return;
                }
                if self.commit_in_flight.is_some() {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
            }
            if self.has_unsaved() {
                return;
            }
            if self.pending_automatic.is_some() {
                self.board_shown = true;
            }
            let at = self
                .playback_deadline()
                .map_or_else(Instant::now, |at| at.max(Instant::now()));
            self.tick_at(at, Duration::ZERO, layout::now());
            if self.commit_in_flight.is_none()
                && self.pending_automatic.is_none()
                && !self.needs_prepare
                && self.playback_deadline().is_none()
            {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
        }
    }

    pub fn debug_hint(&mut self) {
        if self.needs_prepare {
            self.debug_settle();
        }
        let id = unique_id(self.session.id());
        self.request_hint_with_id_at(id, Instant::now(), layout::now());
    }

    /// QA (`CP_REPERTOIRE_MOVES`): UCI moves as from the board; `hint`, `settle`,
    /// `skip`, `stop` (Esc), `answer` (the expected move), `wrong` (a move not in the
    /// repertoire).
    pub fn debug_moves(&mut self, moves: &str) {
        if self.needs_prepare {
            self.debug_settle();
        }
        for text in moves.split_whitespace() {
            match text {
                "hint" => self.debug_hint(),
                "settle" | "wait" => self.debug_settle(),
                "skip" => {
                    self.skip_context(Instant::now());
                }
                "stop" => {
                    let mut actions = Vec::new();
                    self.request_exit(&mut actions);
                }
                "answer" => {
                    if let Some(uci) = self.session.current_expected_uci().map(str::to_owned) {
                        let id = unique_id(self.session.id());
                        self.play_uci_with_id_at(&uci, id, Instant::now(), layout::now());
                    }
                }
                "wrong" => {
                    if let Some(uci) = self.debug_wrong_move() {
                        let id = unique_id(self.session.id());
                        self.play_uci_with_id_at(&uci, id, Instant::now(), layout::now());
                    }
                }
                _ => {
                    let id = unique_id(self.session.id());
                    self.play_uci_with_id_at(text, id, Instant::now(), layout::now());
                }
            }
        }
    }

    /// A legal move that is no answer of the repertoire here.
    fn debug_wrong_move(&self) -> Option<String> {
        use chess_core::shakmaty::Position;
        let position = fen4(&self.board_position);
        let user = self.session.current_target()?.key.user_id.clone();
        self.board_position
            .legal_moves()
            .into_iter()
            .find_map(|mv| {
                let uci = mv.to_uci(CastlingMode::Standard).to_string();
                let key = LearningItemKey::new(user.clone(), position.clone(), &uci);
                (!self.repertoire_answers.contains(&key)).then_some(uci)
            })
    }
}

/// «Preparing…» (S0): the session's screen before the session exists, so the board
/// and the panel do not move when it comes.
pub fn show_preparing(
    ui: &mut Ui,
    settings: &Settings,
    board: &mut BoardState,
    view: &PreparingView<'_>,
) -> Vec<SessionAction> {
    let mut actions = Vec::new();
    if !ui.ctx().egui_wants_keyboard_input()
        && !crate::app::overlay_open(ui.ctx())
        && ui.input_mut(|input| {
            input.consume_key(Modifiers::NONE, Key::Escape)
                || input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W))
        })
    {
        actions.push(SessionAction::Back);
    }
    let back = if ui.available_width() < 600.0 {
        "⬅"
    } else {
        view.back
    };
    let mut header = (false, false);
    layout::screen(
        ui,
        board,
        |_, ui| header = crate::widgets::section_header_sub(ui, back, view.name, view.subtitle),
        |_, ui| {
            let clicked = side_panel(
                ui,
                |ui| {
                    show_card(
                        ui,
                        &CardView {
                            title: tr("Preparing…").to_owned(),
                            instruction: String::new(),
                            title_color: None,
                            active: false,
                            alert: false,
                            count: "—".to_owned(),
                            label: if view.lines { tr("line") } else { tr("left") },
                        },
                    )
                },
                "",
                |_| {},
                Some((tr("Hint (H)").to_owned(), false)),
                None,
                false,
            );
            if clicked == Some(Clicked::Finish) {
                actions.push(SessionAction::Back);
            }
        },
        |board, ui| {
            layout::still_board(
                ui,
                board,
                settings,
                view.position,
                view.orientation,
                view.last_move,
                &BoardAnnotations::default(),
            )
        },
    );
    if header.0 {
        actions.push(SessionAction::Back);
    }
    if header.1 {
        actions.push(SessionAction::OpenSettings);
    }
    actions
}

pub struct PreparingView<'a> {
    pub name: &'a str,
    pub subtitle: &'a str,
    pub back: &'a str,
    pub position: &'a Chess,
    pub orientation: Color,
    pub last_move: Option<(Square, Square)>,
    /// A review: the counter counts lines.
    pub lines: bool,
}

struct CardView {
    title: String,
    instruction: String,
    title_color: Option<Color32>,
    active: bool,
    alert: bool,
    count: String,
    label: &'static str,
}

/// Block A: two lines on the left, the counter on the right; always 64 pt.
fn show_card(ui: &mut Ui, view: &CardView) {
    layout::fixed_block(ui, CARD_H, |ui| {
        let frame = layout::card(ui, view.active, view.alert);
        let inner = vec2(ui.available_width() - 20.0, CARD_H - 12.0);
        frame.show(ui, |ui| {
            ui.set_min_size(inner);
            ui.set_max_size(inner);
            ui.horizontal(|ui| {
                let counter_w = 64.0;
                ui.allocate_ui_with_layout(
                    vec2((ui.available_width() - counter_w).max(0.0), inner.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let mut title = RichText::new(&view.title).strong().size(15.0);
                        if let Some(color) = view.title_color {
                            title = title.color(color);
                        }
                        layout::line(ui, title);
                        layout::line(ui, RichText::new(&view.instruction).size(13.0));
                    },
                );
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(&view.count).monospace().size(24.0));
                    ui.label(RichText::new(view.label).weak().size(11.0));
                });
            });
        });
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clicked {
    Left,
    Right,
    Flip,
    Finish,
}

/// The session's panel (spec §4.1): A card · B opening · D comment or result · E
/// actions · F tool row; every block keeps its height in every state.
#[allow(clippy::too_many_arguments)]
fn side_panel(
    ui: &mut Ui,
    card: impl FnOnce(&mut Ui),
    opening: &str,
    body: impl FnOnce(&mut Ui),
    left: Option<(String, bool)>,
    right: Option<(String, bool)>,
    right_primary: bool,
) -> Option<Clicked> {
    let mut clicked = None;
    let rect = ui.available_rect_before_wrap();
    let footer_h = TOOL_H + 12.0;
    let footer = egui::Rect::from_min_max(
        egui::pos2(rect.left(), (rect.bottom() - footer_h).max(rect.top())),
        rect.max,
    );
    let body_rect = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), footer.top()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(body_rect), |ui| {
        ui.set_clip_rect(body_rect.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing.y = GAP;
        ui.add_space(4.0);
        card(ui);
        layout::fixed_block(ui, LINE_H, |ui| {
            layout::line(ui, RichText::new(opening).weak());
        });
        let rest = (ui.available_height() - ROW_H - GAP).max(0.0);
        layout::fixed_block(ui, rest, body);
        layout::fixed_block(ui, ROW_H, |ui| {
            ui.horizontal(|ui| {
                if let Some((label, enabled)) = left {
                    let response = ui.add_enabled(
                        enabled,
                        egui::Button::new(label)
                            .truncate()
                            .min_size(vec2(LEFT_W.min(ui.available_width() / 2.0), ROW_H)),
                    );
                    if response.clicked() {
                        clicked = Some(Clicked::Left);
                    }
                }
                if let Some((label, enabled)) = right {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let width = MAIN_W.min(ui.available_width());
                        let response = if right_primary {
                            ui.add_enabled_ui(enabled, |ui| {
                                crate::widgets::primary_wide(ui, &label, width)
                            })
                            .inner
                        } else {
                            ui.add_enabled(
                                enabled,
                                egui::Button::new(label)
                                    .truncate()
                                    .min_size(vec2(width, ROW_H)),
                            )
                        };
                        if response.clicked() {
                            clicked = Some(Clicked::Right);
                        }
                    });
                }
            });
        });
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
        ui.separator();
        ui.horizontal(|ui| {
            let icon = |s: &str| {
                egui::Button::new(RichText::new(s).size(18.0)).min_size(vec2(0.0, TOOL_H))
            };
            if ui
                .add(icon("🔃"))
                .on_hover_text(tr("Flip board (F)"))
                .clicked()
            {
                clicked = Some(Clicked::Flip);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new(tr("× Finish")).size(14.0))
                            .min_size(vec2(0.0, TOOL_H)),
                    )
                    .on_hover_text(tr("Finish the session (Esc)"))
                    .clicked()
                {
                    clicked = Some(Clicked::Finish);
                }
            });
        });
    });
    clicked
}

impl VisualMove {
    fn from_context(step: &LearningContextStep) -> Result<Self, String> {
        let before = parse_position(&step.from)?;
        let mv = step
            .uci
            .parse::<UciMove>()
            .ok()
            .and_then(|uci| uci.to_move(&before).ok())
            .ok_or_else(|| format!("automatic context move {} is not legal", step.uci))?;
        Ok(Self::new(step.uci.clone(), before, mv))
    }

    fn new(uci: String, before: Chess, mv: Move) -> Self {
        use chess_core::shakmaty::Position;
        let from = mv.from().unwrap_or_else(|| mv.to());
        let to = crate::board::ui_target(&mv);
        let mut after = before.clone();
        after.play_unchecked(mv);
        Self {
            uci,
            before,
            after,
            from,
            to,
        }
    }
}

fn parse_position(position: &trainer::repertoire::LearningPosition) -> Result<Chess, String> {
    position
        .fen
        .parse::<Fen>()
        .map_err(|error| error.to_string())?
        .into_position(CastlingMode::Standard)
        .map_err(|error| error.to_string())
}

fn unique_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}-{}-{sequence:x}", std::process::id())
}

#[cfg(test)]
#[path = "opening_session_tests.rs"]
mod tests;
