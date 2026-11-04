use std::{
    collections::{BTreeSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use chess_core::shakmaty::{
    CastlingMode, Chess, Color, Move, Position, Square, fen::Fen, uci::UciMove,
};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText, Ui, vec2};
use trainer::repertoire::{
    AttemptApplication, LearningContextPolicy, LearningContextStep, LearningData,
    LearningPresentation, LearningSessionPhase, LearningSessionSnapshot, LearningSide,
    LearningStore, Library, QueueMode, SrsSettings, StatsScope, TrainingAttempt, TrainingEventType,
    TrainingMode, fen4,
};
use web_time::{Instant, SystemTime, UNIX_EPOCH};

use crate::assets::Sound;
use crate::board::{Arrow, BoardState, BoardView};
use crate::i18n::{tr, trf};
use crate::settings::Settings;

const WRONG_SHOWN: Duration = Duration::from_millis(400);
const DEFAULT_ANIMATION: Duration = Duration::from_millis(250);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub enum LearningTrainingAction {
    Back,
    Sound(Sound),
    Toast(String),
}

#[derive(Clone)]
struct VisualMove {
    #[allow(dead_code)]
    uci: String,
    before: Chess,
    after: Chess,
    from: Square,
    to: Square,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlayOutcome {
    CorrectIndependent,
    WrongIndependent,
    WrongReplay,
    ReplayAfterError,
    PreviewedReplay,
}

#[derive(Clone)]
enum CommitKind {
    Context,
    Play {
        visual: Box<VisualMove>,
        outcome: PlayOutcome,
    },
    Preview,
}

#[derive(Clone)]
struct PendingCommit {
    attempt: TrainingAttempt,
    kind: CommitKind,
}

enum Playback {
    Automatic {
        current: Box<VisualMove>,
        remaining: VecDeque<LearningContextStep>,
        started_at: Instant,
        duration: Duration,
        completed: usize,
        total: usize,
    },
    Paused {
        remaining: VecDeque<LearningContextStep>,
        completed: usize,
    },
    Wrong {
        visual: Box<VisualMove>,
        until: Instant,
    },
    Accepted {
        started_at: Instant,
        duration: Duration,
        independent_success: bool,
    },
}

pub struct LearningTrainingUi {
    library: Library,
    store: LearningStore,
    session: LearningSessionSnapshot,
    learning: LearningData,
    presentation: Option<LearningPresentation>,
    completed_attempt_ids: BTreeSet<String>,
    pending_commit: Option<PendingCommit>,
    save_error: Option<String>,
    error: Option<String>,
    board: BoardState,
    board_position: Chess,
    board_shown: bool,
    pending_automatic: Option<VecDeque<LearningContextStep>>,
    playback: Option<Playback>,
    pause_requested: bool,
    stopped: bool,
    needs_prepare: bool,
    animation: Duration,
    ready_since: Option<Instant>,
    last_move: Option<(Square, Square)>,
    flipped: bool,
    message: Option<String>,
    confirm_discard: bool,
    discard_requested: bool,
    pending_sounds: Vec<Sound>,
    #[cfg(test)]
    visual_log: Vec<String>,
}

impl LearningTrainingUi {
    #[allow(dead_code)]
    pub fn new(
        library: Library,
        store: LearningStore,
        scope: StatsScope,
        settings: SrsSettings,
        introduce_new: bool,
        flipped: bool,
    ) -> Result<Self, String> {
        Self::new_with_policy(
            library,
            store,
            scope,
            settings,
            introduce_new,
            flipped,
            QueueMode::SpacedRepetition,
            LearningContextPolicy::default(),
        )
    }

    #[allow(dead_code)]
    pub fn new_with_mode(
        library: Library,
        store: LearningStore,
        scope: StatsScope,
        settings: SrsSettings,
        introduce_new: bool,
        flipped: bool,
        mode: QueueMode,
    ) -> Result<Self, String> {
        Self::new_with_policy(
            library,
            store,
            scope,
            settings,
            introduce_new,
            flipped,
            mode,
            LearningContextPolicy::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_policy(
        library: Library,
        store: LearningStore,
        scope: StatsScope,
        settings: SrsSettings,
        introduce_new: bool,
        flipped: bool,
        mode: QueueMode,
        context_policy: LearningContextPolicy,
    ) -> Result<Self, String> {
        if introduce_new && mode != QueueMode::SpacedRepetition {
            return Err(format!("new learning items require SRS, not {mode:?}"));
        }
        let data = store.load().map_err(|error| error.to_string())?;
        let existing = if introduce_new {
            None
        } else {
            data.session_snapshots
                .iter()
                .filter(|session| {
                    (mode != QueueMode::SpacedRepetition
                        || session.phase != LearningSessionPhase::Finished)
                        && compatible(session, &scope, mode)
                })
                .max_by_key(|session| session.revision)
                .cloned()
        };
        let mut session = match existing {
            Some(session) => {
                let mut session = session
                    .resume(&library)
                    .map_err(|error| error.to_string())?;
                if session.phase == LearningSessionPhase::Finished
                    && matches!(mode, QueueMode::ReviewInOrder | QueueMode::LeastRecent)
                {
                    session
                        .start_next_pass(&library, &data.learning)
                        .map_err(|error| error.to_string())?;
                }
                session
            }
            None => {
                let mut session = LearningSessionSnapshot::new_with_mode(
                    unique_id("learning-session"),
                    &library,
                    &data.learning,
                    scope,
                    settings,
                    mode,
                )
                .map_err(|error| error.to_string())?;
                session.context_policy = context_policy;
                if introduce_new {
                    session
                        .introduce_new(&library, &data.learning)
                        .map_err(|error| error.to_string())?;
                }
                session
            }
        };
        store
            .save_session(&mut session)
            .map_err(|error| error.to_string())?;
        let completed_attempt_ids = data
            .learning
            .attempts
            .iter()
            .filter(|attempt| attempt.session_id == session.id())
            .map(|attempt| attempt.client_attempt_id.clone())
            .collect();
        let flipped = match session.stats_scope.side {
            LearningSide::White => flipped,
            LearningSide::Black => !flipped,
        };
        Ok(Self {
            library,
            store,
            session,
            learning: data.learning,
            presentation: None,
            completed_attempt_ids,
            pending_commit: None,
            save_error: None,
            error: None,
            board: BoardState::default(),
            board_position: Chess::default(),
            board_shown: false,
            pending_automatic: None,
            playback: None,
            pause_requested: false,
            stopped: false,
            needs_prepare: true,
            animation: DEFAULT_ANIMATION,
            ready_since: None,
            last_move: None,
            flipped,
            message: None,
            confirm_discard: false,
            discard_requested: false,
            pending_sounds: Vec::new(),
            #[cfg(test)]
            visual_log: Vec::new(),
        })
    }

    #[allow(dead_code)]
    pub fn learning_data(&self) -> &LearningData {
        &self.learning
    }

    pub fn has_unsaved(&self) -> bool {
        self.pending_commit.is_some() || self.save_error.is_some()
    }

    pub fn target_available(&self) -> bool {
        self.session.current_target().is_some()
    }

    pub fn is_finished(&self) -> bool {
        self.session.phase == LearningSessionPhase::Finished
            && self.playback.is_none()
            && !self.has_unsaved()
    }

    #[allow(dead_code)]
    pub fn session_id(&self) -> &str {
        self.session.id()
    }

    fn can_input(&self) -> bool {
        !self.has_unsaved()
            && !self.stopped
            && self.error.is_none()
            && !self.needs_prepare
            && self.playback.is_none()
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
        self.can_input() && self.session.phase == LearningSessionPhase::AwaitingIndependent
    }

    fn tick_at(&mut self, now: Instant, animation: Duration, happened_at: i64) {
        self.animation = animation;
        if self.stopped || self.has_unsaved() {
            return;
        }
        if self.board_shown
            && let Some(steps) = self.pending_automatic.take()
        {
            self.start_next_automatic(steps, now, 0);
        }
        if self.needs_prepare {
            self.prepare_presentation(now);
        }
        loop {
            if self.stopped || self.has_unsaved() || self.error.is_some() {
                return;
            }
            let Some(deadline) = self.playback_deadline() else {
                return;
            };
            if now < deadline {
                return;
            }
            match self.playback {
                Some(Playback::Wrong { .. }) => self.finish_wrong(now),
                Some(Playback::Accepted { .. }) => self.finish_accepted(now),
                Some(Playback::Automatic { .. }) => {
                    let id = unique_id(self.session.id());
                    self.advance_context_with_id_at(id, now, happened_at);
                }
                Some(Playback::Paused { .. }) | None => return,
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
            Some(Playback::Wrong { until, .. }) => Some(*until),
            Some(Playback::Paused { .. }) | None => None,
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
        let Some(current) = presentation.current_position else {
            self.ready_since = None;
            self.pending_automatic = None;
            if self.session.queue.is_empty() && self.message.is_none() {
                self.message =
                    Some(tr("No due moves. New items are introduced only on request.").to_owned());
            }
            return;
        };
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
            if !self.board_shown {
                self.board_position = match parse_position(&current) {
                    Ok(position) => position,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
                self.last_move = None;
                self.playback = None;
                self.pending_automatic = Some(steps);
                return;
            }
            self.pending_automatic = None;
            self.start_next_automatic(steps, now, 0);
            return;
        }
        self.pending_automatic = None;
        let position = match parse_position(&current) {
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
        self.playback = None;
        match self.session.phase {
            LearningSessionPhase::AwaitingIndependent => {
                self.ready_since = Some(now);
                self.message = Some(tr("Your turn: play the main repertoire move.").to_owned());
            }
            LearningSessionPhase::AwaitingContextIndependent => {
                self.ready_since = Some(now);
                self.message = Some(format!(
                    "{} · {}",
                    tr("Automatic move/context"),
                    tr("Your turn: play the main repertoire move.")
                ));
            }
            LearningSessionPhase::AwaitingReplayAfterError => {
                self.ready_since = None;
                self.message = Some(tr("Wrong move. Replay the revealed move; it will not count as an independent success.").to_owned());
            }
            LearningSessionPhase::PreviewedReplay => {
                self.ready_since = None;
                self.message = Some(
                    tr("PREVIEWED · replay the shown move; this is not an independent success.")
                        .to_owned(),
                );
            }
            phase => {
                self.ready_since = None;
                self.error = Some(format!(
                    "learning session has no playable position in {phase:?}"
                ));
            }
        }
    }

    fn start_next_automatic(
        &mut self,
        mut remaining: VecDeque<LearningContextStep>,
        now: Instant,
        completed: usize,
    ) {
        let Some(step) = remaining.pop_front() else {
            self.needs_prepare = true;
            self.prepare_presentation(now);
            return;
        };
        let total = completed.saturating_add(remaining.len()).saturating_add(1);
        let current = match VisualMove::from_context(&step) {
            Ok(current) => current,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.board_position = current.after.clone();
        self.last_move = Some((current.from, current.to));
        self.message = None;
        self.playback = Some(Playback::Automatic {
            current: Box::new(current.clone()),
            remaining,
            started_at: now,
            duration: self.animation.div_f32(step.speed_factor.max(0.01) as f32),
            completed,
            total,
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
        let Some(Playback::Automatic {
            remaining,
            completed,
            ..
        }) = self.playback.take()
        else {
            self.error = Some("automatic context is missing its visual step".to_owned());
            return;
        };
        let completed = completed + 1;
        if self.pause_requested && !remaining.is_empty() {
            self.pause_requested = false;
            self.message = Some(tr("Paused").to_owned());
            self.playback = Some(Playback::Paused {
                remaining,
                completed,
            });
            return;
        }
        self.pause_requested = false;
        self.start_next_automatic(remaining, now, completed);
    }

    fn is_autoplay(&self) -> bool {
        matches!(
            &self.playback,
            Some(Playback::Automatic { .. } | Playback::Paused { .. })
        )
    }

    fn autoplay_paused(&self) -> bool {
        matches!(&self.playback, Some(Playback::Paused { .. }))
    }

    fn request_pause(&mut self) -> bool {
        let active = matches!(&self.playback, Some(Playback::Automatic { .. }));
        if active {
            self.pause_requested = true;
        }
        active
    }

    fn continue_autoplay(&mut self, now: Instant) -> bool {
        if self.stopped || self.has_unsaved() {
            return false;
        }
        let Some(Playback::Paused {
            remaining,
            completed,
        }) = self.playback.take()
        else {
            return false;
        };
        self.pause_requested = false;
        self.start_next_automatic(remaining, now, completed);
        true
    }

    fn stop_autoplay(&mut self, actions: &mut Vec<LearningTrainingAction>) -> bool {
        if !self.is_autoplay() {
            return false;
        }
        if self.has_unsaved() {
            self.request_back(actions);
            return true;
        }
        self.pause_requested = false;
        let playback = self.playback.take();
        if let Some(Playback::Automatic { current, .. }) = playback {
            self.board_position = current.before.clone();
        }
        self.last_move = None;
        self.ready_since = None;
        self.stopped = true;
        actions.push(LearningTrainingAction::Back);
        true
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
        if !independent && uci != expected_uci {
            self.start_playback(visual, PlayOutcome::WrongReplay, now);
            return true;
        }
        let response_time_ms = independent.then(|| {
            self.ready_since.map_or(0, |ready| {
                u64::try_from(now.saturating_duration_since(ready).as_millis()).unwrap_or(u64::MAX)
            })
        });
        let attempt = match self.session.play_uci_with_learning(
            &self.library,
            &self.learning,
            &uci,
            client_attempt_id,
            happened_at,
            response_time_ms,
        ) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.error = Some(error.to_string());
                return false;
            }
        };
        let outcome = match attempt.event_type {
            TrainingEventType::CorrectIndependent => PlayOutcome::CorrectIndependent,
            TrainingEventType::IncorrectIndependent => PlayOutcome::WrongIndependent,
            TrainingEventType::ReplayAfterError => PlayOutcome::ReplayAfterError,
            TrainingEventType::AutoContext => PlayOutcome::PreviewedReplay,
            TrainingEventType::Previewed | TrainingEventType::IllegalMove => {
                self.error = Some("legal board input produced a non-play event".to_owned());
                return false;
            }
        };
        self.begin_commit(
            attempt,
            CommitKind::Play {
                visual: Box::new(visual),
                outcome,
            },
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
            self.message = Some(tr("Illegal move in the current position").to_owned());
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
                self.message = Some(tr("Wrong move. Replay the revealed move; it will not count as an independent success.").to_owned());
                self.playback = Some(Playback::Wrong {
                    visual: Box::new(visual),
                    until: now + WRONG_SHOWN,
                });
                self.queue_sound(Sound::Wrong);
            }
            PlayOutcome::CorrectIndependent
            | PlayOutcome::ReplayAfterError
            | PlayOutcome::PreviewedReplay => {
                self.message = Some(
                    tr(match outcome {
                        PlayOutcome::CorrectIndependent => "Correct independent answer.",
                        PlayOutcome::ReplayAfterError => {
                            "REPLAY_AFTER_ERROR · not an independent success."
                        }
                        PlayOutcome::PreviewedReplay => {
                            "PREVIEWED · shown move completed; no independent success recorded."
                        }
                        PlayOutcome::WrongIndependent | PlayOutcome::WrongReplay => {
                            "Wrong move. Replay the revealed move; it will not count as an independent success."
                        }
                    })
                    .to_owned(),
                );
                self.playback = Some(Playback::Accepted {
                    started_at: now,
                    duration: self.animation,
                    independent_success: outcome == PlayOutcome::CorrectIndependent,
                });
                self.queue_sound(Sound::Move);
            }
        }
    }

    fn finish_wrong(&mut self, now: Instant) {
        let Some(Playback::Wrong { visual, .. }) = self.playback.take() else {
            return;
        };
        let visual = *visual;
        self.board_position = visual.before;
        self.last_move = None;
        self.needs_prepare = true;
        self.prepare_presentation(now);
    }

    fn finish_accepted(&mut self, now: Instant) {
        let Some(Playback::Accepted {
            independent_success,
            ..
        }) = self.playback.take()
        else {
            return;
        };
        if independent_success && self.session.phase == LearningSessionPhase::Finished {
            self.queue_sound(Sound::Success);
        }
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
        let attempt = match self.session.preview(client_attempt_id, happened_at) {
            Ok(Some(attempt)) => attempt,
            Ok(None) => return false,
            Err(error) => {
                self.error = Some(error.to_string());
                return false;
            }
        };
        self.board.clear();
        self.begin_commit(attempt, CommitKind::Preview, now);
        true
    }

    fn begin_commit(&mut self, attempt: TrainingAttempt, kind: CommitKind, now: Instant) {
        if self.pending_commit.is_some() {
            self.save_error =
                Some("another learning event is still waiting to be saved".to_owned());
            return;
        }
        self.pending_commit = Some(PendingCommit { attempt, kind });
        self.persist_pending(now);
    }

    fn persist_pending(&mut self, now: Instant) {
        let Some(pending) = self.pending_commit.clone() else {
            return;
        };
        match self
            .store
            .apply_attempt_and_session(pending.attempt.clone(), &mut self.session)
        {
            Ok(persisted) => {
                let duplicate = persisted.application == AttemptApplication::Duplicate;
                self.learning = persisted.learning;
                self.session = persisted.session;
                self.completed_attempt_ids
                    .insert(pending.attempt.client_attempt_id.clone());
                self.pending_commit = None;
                self.save_error = None;
                self.confirm_discard = false;
                if duplicate {
                    self.playback = None;
                    self.pause_requested = false;
                    self.ready_since = None;
                    self.needs_prepare = true;
                    self.prepare_presentation(now);
                    return;
                }
                match pending.kind {
                    CommitKind::Context => self.finish_context(now),
                    CommitKind::Play { visual, outcome } => {
                        self.start_playback(*visual, outcome, now);
                    }
                    CommitKind::Preview => {
                        self.needs_prepare = true;
                        self.prepare_presentation(now);
                    }
                }
            }
            Err(error) => {
                self.save_error = Some(error.to_string());
                self.ready_since = None;
            }
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

    fn request_back(&mut self, actions: &mut Vec<LearningTrainingAction>) {
        if self.has_unsaved() {
            self.confirm_discard = true;
        } else {
            actions.push(LearningTrainingAction::Back);
        }
    }

    fn keyboard(&mut self, ui: &Ui, actions: &mut Vec<LearningTrainingAction>) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let autoplay = self.is_autoplay();
        let mut command = None;
        ui.input_mut(|input| {
            if autoplay
                && (input.consume_key(Modifiers::NONE, Key::Space)
                    || input.consume_key(Modifiers::NONE, Key::P))
            {
                command = Some("toggle-pause");
            } else if autoplay && input.consume_key(Modifiers::NONE, Key::Escape) {
                command = Some("stop");
            } else if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W))
                || (!self.board.is_busy() && input.consume_key(Modifiers::NONE, Key::Escape))
            {
                command = Some("back");
            } else if input.consume_key(Modifiers::NONE, Key::F) {
                command = Some("flip");
            } else if input.consume_key(Modifiers::NONE, Key::H) {
                command = Some("hint");
            } else if input.consume_key(Modifiers::NONE, Key::L)
                || input.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::A))
            {
                command = Some("engine");
            } else if self.is_finished() && input.consume_key(Modifiers::NONE, Key::Enter) {
                command = Some("back");
            }
        });
        match command {
            Some("toggle-pause") => {
                if self.autoplay_paused() {
                    self.continue_autoplay(Instant::now());
                } else {
                    self.request_pause();
                }
            }
            Some("stop") => {
                self.stop_autoplay(actions);
            }
            Some("back") => self.request_back(actions),
            Some("flip") => self.flipped = !self.flipped,
            Some("hint") => {
                let id = unique_id(self.session.id());
                self.request_hint_with_id_at(id, Instant::now(), now());
            }
            Some("engine") => actions.push(LearningTrainingAction::Toast(
                tr("The engine is off while training").to_owned(),
            )),
            _ => {}
        }
    }

    fn board_animation_seconds(&self, settings: &Settings) -> f32 {
        match &self.playback {
            Some(Playback::Automatic { duration, .. }) => duration.as_secs_f32(),
            _ => settings.animation.seconds(),
        }
    }

    fn board_ui(&mut self, ui: &mut Ui, settings: &Settings) {
        let interactive = self.can_input();
        let mut arrows = Vec::new();
        if self.playback.is_none()
            && let Some(uci) = self
                .presentation
                .as_ref()
                .and_then(|presentation| presentation.revealed_move.as_deref())
            && let Some(mv) = uci
                .parse::<UciMove>()
                .ok()
                .and_then(|uci| uci.to_move(&self.board_position).ok())
        {
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
        let area = ui.available_rect_before_wrap();
        let side = area.width().min(area.height()).max(1.0).floor();
        let left = area.left() + ((area.width() - side) / 2.0).max(0.0).floor();
        let top = area.top() + ((area.height() - side) / 2.0).max(0.0).floor();
        let rect = egui::Rect::from_min_size(egui::pos2(left, top), vec2(side, side));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            let played = BoardView {
                pos: &self.board_position,
                orientation: if self.flipped {
                    Color::Black
                } else {
                    Color::White
                },
                last_move: self.last_move,
                arrows: &arrows,
                circles: &[],
                badge: None,
                interactive,
                show_coords: settings.show_coords,
                theme: settings.board_theme.theme(),
                pieces: settings.piece_set,
                animation: self.board_animation_seconds(settings),
                premove: None,
                auto_queen: false,
                alert,
            }
            .show(ui, &mut self.board);
            if let Some(mv) = played {
                self.play_move_at(mv, Instant::now(), now());
            }
        });
        self.board_shown = true;
    }

    fn status_card(&mut self, ui: &mut Ui) {
        let alert = matches!(self.playback, Some(Playback::Wrong { .. }));
        fixed_block(ui, 144.0, |ui| {
            card(ui, alert).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_clip_rect(ui.max_rect());
                if self.is_finished() {
                    ui.label(RichText::new(tr("Training complete")).strong().size(17.0));
                } else if let Some(Playback::Automatic {
                    completed, total, ..
                }) = &self.playback
                {
                    ui.label(
                        RichText::new(trf(
                            "Automatic context · {current} of {total}",
                            &[("current", &(completed + 1)), ("total", total)],
                        ))
                        .strong(),
                    );
                } else if let Some(Playback::Paused {
                    remaining,
                    completed,
                }) = &self.playback
                {
                    let progress = trf(
                        "Automatic context · {current} of {total}",
                        &[
                            ("current", completed),
                            ("total", &completed.saturating_add(remaining.len())),
                        ],
                    );
                    ui.label(RichText::new(format!("{} · {progress}", tr("Paused"))).strong());
                } else if self.session.phase == LearningSessionPhase::PreviewedReplay {
                    ui.label(RichText::new("PREVIEWED").strong());
                } else if self.session.phase == LearningSessionPhase::AwaitingReplayAfterError {
                    ui.label(RichText::new(tr("Wrong")).strong());
                }
                if let Some(error) = self.error.clone().or_else(|| self.save_error.clone()) {
                    let text = self.save_error.as_ref().map_or(error.clone(), |error| {
                        trf("Could not save changes: {error}", &[("error", error)])
                    });
                    ui.add(
                        egui::Label::new(RichText::new(text).color(ui.visuals().error_fg_color))
                            .truncate(),
                    );
                    if self.pending_commit.is_some() {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(tr("Retry")).clicked() {
                                self.retry_pending(Instant::now());
                            }
                            if self.confirm_discard {
                                if ui.button(tr("Leave without saving")).clicked() {
                                    self.discard_requested = true;
                                }
                            } else if ui.button(tr("Back to repertoire")).clicked() {
                                self.confirm_discard = true;
                            }
                        });
                    }
                } else if let Some(message) = &self.message {
                    ui.add(egui::Label::new(message).wrap());
                }
            });
        });
    }

    fn side(&mut self, ui: &mut Ui, actions: &mut Vec<LearningTrainingAction>) {
        ui.spacing_mut().item_spacing.y = 6.0;
        fixed_block(ui, 44.0, |ui| {
            let (done, total) = main_task_progress(&self.session);
            ui.add(
                egui::Label::new(
                    RichText::new(trf(
                        "Tasks: {done} of {total}",
                        &[("done", &done), ("total", &total)],
                    ))
                    .strong()
                    .size(15.0),
                )
                .truncate(),
            );
            let side = tr(match self.session.stats_scope.side {
                LearningSide::White => "White",
                LearningSide::Black => "Black",
            });
            ui.add(
                egui::Label::new(format!(
                    "{} · {side}",
                    queue_mode_label(self.session.session.mode)
                ))
                .truncate(),
            );
        });
        self.status_card(ui);
        ui.horizontal_wrapped(|ui| {
            if self.is_autoplay() {
                let paused = self.autoplay_paused();
                let label = if paused { tr("Continue") } else { tr("Pause") };
                if ui.button(label).on_hover_text("Space / P").clicked() {
                    if paused {
                        self.continue_autoplay(Instant::now());
                    } else {
                        self.request_pause();
                    }
                }
                if ui.button(tr("Stop")).on_hover_text("Escape").clicked() {
                    self.stop_autoplay(actions);
                }
            } else if self.is_finished() {
                if crate::widgets::primary(ui, tr("Next"))
                    .on_hover_text("Enter")
                    .clicked()
                {
                    self.request_back(actions);
                }
            } else if ui
                .add_enabled(self.can_hint(), egui::Button::new(tr("Hint")))
                .clicked()
            {
                let id = unique_id(self.session.id());
                self.request_hint_with_id_at(id, Instant::now(), now());
            }
            if ui
                .button("🔃")
                .on_hover_text(tr("Flip board (F)"))
                .clicked()
            {
                self.flipped = !self.flipped;
            }
        });
        if self.confirm_discard && self.has_unsaved() {
            ui.add(
                egui::Label::new(
                    RichText::new(tr(
                        "Only the events that could not be saved will be discarded",
                    ))
                    .weak(),
                )
                .wrap(),
            );
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        settings: &Settings,
        name: &str,
    ) -> Vec<LearningTrainingAction> {
        let animation = Duration::from_secs_f32(settings.animation.seconds());
        self.tick_at(Instant::now(), animation, now());
        if let Some(deadline) = self.playback_deadline() {
            ui.ctx()
                .request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        } else if self.needs_prepare || self.pending_automatic.is_some() {
            ui.ctx().request_repaint();
        }
        let mut actions = Vec::new();
        self.keyboard(ui, &mut actions);
        egui::Panel::top("learning_training_header").show(ui, |ui| {
            ui.horizontal(|ui| {
                let back = if ui.available_width() < 600.0 {
                    tr("Back")
                } else {
                    tr("Back to repertoire")
                };
                if ui.button(format!("← {back}")).clicked() {
                    self.request_back(&mut actions);
                }
                ui.separator();
                ui.add(egui::Label::new(RichText::new(name).heading()).truncate())
                    .on_hover_text(name);
            });
        });
        let available = ui.available_rect_before_wrap();
        let background = ui.visuals().window_fill();
        ui.painter_at(available)
            .rect_filled(available, 0.0, background);
        let width = available.width();
        let height = available.height();
        let stacked = width < 760.0 || (width < 900.0 && height > width);
        if stacked {
            let panel_height = (height * 0.52)
                .clamp(250.0, 300.0)
                .min((height - 160.0).max(120.0));
            let board_rect = egui::Rect::from_min_max(
                available.min,
                egui::pos2(available.right(), available.bottom() - panel_height),
            );
            let side_rect = egui::Rect::from_min_max(
                egui::pos2(available.left(), board_rect.bottom()),
                available.max,
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(board_rect), |ui| {
                egui::CentralPanel::default().show(ui, |ui| self.board_ui(ui, settings));
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(side_rect), |ui| {
                ui.set_clip_rect(side_rect);
                egui::Frame::side_top_panel(ui.style()).show(ui, |ui| self.side(ui, &mut actions));
            });
        } else {
            let side = egui::Panel::right("learning_training_side")
                .resizable(true)
                .default_size((width * 0.32).clamp(320.0, 420.0))
                .size_range(300.0..=(width - 240.0).max(300.0))
                .show(ui, |ui| self.side(ui, &mut actions));
            crate::widgets::check_panel("learning_training_side", &side.response);
            egui::CentralPanel::default().show(ui, |ui| self.board_ui(ui, settings));
        }
        actions.extend(
            self.pending_sounds
                .drain(..)
                .map(LearningTrainingAction::Sound),
        );
        if self.has_unsaved() && !self.discard_requested {
            actions.retain(|action| !matches!(action, LearningTrainingAction::Back));
        }
        if std::mem::take(&mut self.discard_requested) {
            actions.push(LearningTrainingAction::Back);
        }
        actions
    }

    pub fn debug_settle(&mut self) {
        let started = Instant::now();
        let at = self
            .playback_deadline()
            .map_or(started, |deadline| deadline.max(started));
        self.tick_at(at, Duration::ZERO, now());
        if matches!(self.playback, Some(Playback::Wrong { .. })) {
            let until = self.playback_deadline().unwrap_or(at);
            self.tick_at(until, Duration::ZERO, now());
        }
    }

    pub fn debug_hint(&mut self) {
        if self.needs_prepare {
            self.tick_at(Instant::now(), Duration::ZERO, now());
        }
        let id = unique_id(self.session.id());
        self.request_hint_with_id_at(id, Instant::now(), now());
    }

    pub fn debug_moves(&mut self, moves: &str) {
        if self.needs_prepare {
            self.tick_at(Instant::now(), Duration::ZERO, now());
        }
        for text in moves.split_whitespace() {
            match text {
                "hint" => self.debug_hint(),
                "settle" | "wait" => self.debug_settle(),
                _ => {
                    let id = unique_id(self.session.id());
                    self.play_uci_with_id_at(text, id, Instant::now(), now());
                }
            }
        }
    }
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

fn main_task_progress(session: &LearningSessionSnapshot) -> (usize, usize) {
    let total = session.queue.len();
    (total.saturating_sub(session.remaining()), total)
}

pub fn queue_mode_label(mode: QueueMode) -> &'static str {
    tr(match mode {
        QueueMode::SpacedRepetition | QueueMode::Practice => "Review individual moves",
        QueueMode::ReviewInOrder => "Review in order",
        QueueMode::LeastRecent => "Least recent and unseen first",
    })
}

fn compatible(session: &LearningSessionSnapshot, scope: &StatsScope, mode: QueueMode) -> bool {
    let expected_side = match scope.side {
        LearningSide::White => TrainingMode::White,
        LearningSide::Black => TrainingMode::Black,
    };
    session.session.mode == mode
        && session.session.side == expected_side
        && session.stats_scope.repertoire_ids == scope.repertoire_ids
        && session.stats_scope.side == scope.side
        && session.stats_scope.max_depth == scope.max_depth
        && session.stats_scope.only_active == scope.only_active
        && session.stats_scope.branch_root == scope.branch_root
        && session.stats_scope.learning_priority == scope.learning_priority
}

fn unique_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}-{}-{sequence:x}", std::process::id())
}

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

fn fixed_block(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        add(ui);
    });
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use trainer::repertoire::{
        LearningItemKey, LearningPriority, LearningStoreData, active_learning_memberships,
    };

    const NOW: i64 = 1_800_000_000;
    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cp-learning-training-{}-{}-{name}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
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

    struct Fixture {
        _dir: TestDir,
        library: Library,
        scope: StatsScope,
        store: LearningStore,
        ui: LearningTrainingUi,
    }

    impl Fixture {
        fn new(pgn: &str, side: LearningSide, due: &[&str], name: &str) -> Self {
            Self::with_introduction(pgn, side, due, name, false)
        }

        fn with_introduction(
            pgn: &str,
            side: LearningSide,
            due: &[&str],
            name: &str,
            introduce_new: bool,
        ) -> Self {
            let dir = TestDir::new(name);
            let mut library = Library::default();
            let id = library
                .create("Test", &chess_core::parse_str(pgn).game)
                .unwrap();
            let scope = StatsScope::for_repertoire(id, side, NOW);
            let mut learning = LearningData::default();
            for expected in due {
                let key = key_for(&library, &scope, expected);
                let item = learning.ensure_item(key);
                item.introduced = true;
                item.next_review_at = Some(NOW);
            }
            let bytes = serde_json::to_vec_pretty(&LearningStoreData {
                learning,
                ..LearningStoreData::default()
            })
            .unwrap();
            std::fs::write(dir.0.join("learning.json"), bytes).unwrap();
            let store = dir.store();
            let mut ui = LearningTrainingUi::new(
                library.clone(),
                store.clone(),
                scope.clone(),
                SrsSettings::default(),
                introduce_new,
                false,
            )
            .unwrap();
            ui.board_shown = true;
            Self {
                _dir: dir,
                library,
                scope,
                store,
                ui,
            }
        }

        fn with_mode(
            pgn: &str,
            side: LearningSide,
            history: &[(&str, i64)],
            name: &str,
            mode: QueueMode,
            priority: LearningPriority,
        ) -> Self {
            let dir = TestDir::new(name);
            let mut library = Library::default();
            let id = library
                .create("Test", &chess_core::parse_str(pgn).game)
                .unwrap();
            let scope = StatsScope::for_repertoire(id, side, NOW).with_learning_priority(priority);
            let mut learning = LearningData::default();
            for (expected, reviewed_at) in history {
                let key = key_for(&library, &scope, expected);
                let item = learning.ensure_item(key);
                item.introduced = true;
                item.correct_streak = 1;
                item.correct_count = 1;
                item.last_review_at = Some(*reviewed_at);
                item.last_correct_at = Some(*reviewed_at);
                item.next_review_at = Some(NOW + 10 * 24 * 60 * 60);
                item.previous_interval_seconds = Some(24 * 60 * 60);
            }
            let bytes = serde_json::to_vec_pretty(&LearningStoreData {
                learning,
                ..LearningStoreData::default()
            })
            .unwrap();
            std::fs::write(dir.0.join("learning.json"), bytes).unwrap();
            let store = dir.store();
            let mut ui = LearningTrainingUi::new_with_mode(
                library.clone(),
                store.clone(),
                scope.clone(),
                SrsSettings::default(),
                false,
                false,
                mode,
            )
            .unwrap();
            ui.board_shown = true;
            Self {
                _dir: dir,
                library,
                scope,
                store,
                ui,
            }
        }

        fn attempts(&self) -> Vec<TrainingAttempt> {
            self.store.load().unwrap().learning.attempts
        }
    }

    fn key_for(library: &Library, scope: &StatsScope, expected: &str) -> LearningItemKey {
        active_learning_memberships(library, scope)
            .unwrap()
            .into_iter()
            .find(|membership| membership.key.expected_uci == expected)
            .unwrap()
            .key
    }

    fn normal() -> Duration {
        Duration::from_millis(250)
    }

    fn answer_current(f: &mut Fixture, at: Instant, happened_at: i64, id: &str) {
        let expected = f.ui.session.current_expected_uci().unwrap().to_owned();
        assert!(f.ui.play_uci_with_id_at(&expected, id.to_owned(), at, happened_at,));
        f.ui.tick_at(at, Duration::ZERO, happened_at);
    }

    #[test]
    fn new_session_saves_custom_policy_and_resume_keeps_persisted_policy() {
        let f = Fixture::new(
            "1. e4 *",
            LearningSide::White,
            &[],
            "context-policy-persistence",
        );
        assert_eq!(
            f.ui.session.context_policy,
            LearningContextPolicy::default()
        );
        let policy = LearningContextPolicy {
            reinforce_minimum_correct: 6,
            reinforce_minimum_days: Some(21),
            dont_show_start_moves_threshold: 9,
            use_fixed_piece_speed: false,
            move_speed_adjustment: 1.5,
            consider_leadup_moves: true,
        };
        let started = LearningTrainingUi::new_with_policy(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            true,
            false,
            QueueMode::SpacedRepetition,
            policy,
        )
        .unwrap();
        assert_eq!(started.session.context_policy, policy);
        let read_store = || {
            let bytes = std::fs::read(f.store.path()).unwrap();
            serde_json::from_slice::<LearningStoreData>(&bytes).unwrap()
        };
        assert_eq!(
            read_store()
                .session_snapshot(started.session_id())
                .unwrap()
                .context_policy,
            policy
        );

        let resumed = LearningTrainingUi::new_with_policy(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            false,
            false,
            QueueMode::SpacedRepetition,
            LearningContextPolicy {
                reinforce_minimum_correct: 99,
                ..LearningContextPolicy::default()
            },
        )
        .unwrap();
        assert_eq!(resumed.session_id(), started.session_id());
        assert_eq!(resumed.session.context_policy, policy);
        assert_eq!(
            read_store()
                .session_snapshot(started.session_id())
                .unwrap()
                .context_policy,
            policy
        );
    }

    fn board_frame(f: &mut Fixture, ctx: &egui::Context, time: f64, settings: &Settings) {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 800.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ui| f.ui.board_ui(ui, settings),
        )
        .drop_without_applying_deltas();
    }

    fn automatic_duration(f: &Fixture) -> Duration {
        match &f.ui.playback {
            Some(Playback::Automatic { duration, .. }) => *duration,
            _ => panic!("automatic playback expected"),
        }
    }

    fn assert_current_slide(
        f: &Fixture,
        time: f64,
        expected_duration: Duration,
        from: Square,
        to: Square,
    ) {
        let (duration, slides, remaining) =
            f.ui.board
                .animation_at(time)
                .expect("the current BoardView transition");
        assert_eq!(remaining, 1.0, "the first visual frame must be at from");
        assert!(
            (duration - f64::from(expected_duration.as_secs_f32())).abs() < 1e-6,
            "animation {duration} does not match scheduler {}",
            expected_duration.as_secs_f32()
        );
        assert!(
            slides
                .iter()
                .any(|(_, slide_from, slide_to)| *slide_from == from && *slide_to == to),
            "missing {from}{to} in {slides:?}"
        );
    }

    fn assert_adaptive_board_slides(f: &mut Fixture, first_ms: u128, last_ms: u128) {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        let start = Instant::now();
        f.ui.board_shown = false;
        f.ui.tick_at(start, normal(), NOW);
        board_frame(f, &ctx, 0.0, &settings);
        assert!(f.ui.board.animation_at(0.0).is_none());

        f.ui.tick_at(start, normal(), NOW);
        let first = automatic_duration(f);
        assert_eq!(first.as_millis(), first_ms);
        assert_eq!(f.ui.playback_deadline(), Some(start + first));
        assert_eq!(f.ui.session.context_index, 0);
        board_frame(f, &ctx, 0.01, &settings);
        assert_current_slide(f, 0.01, first, Square::E2, Square::E4);
        assert!(f.ui.message.is_none());

        f.ui.tick_at(start + first, normal(), NOW);
        let last = automatic_duration(f);
        assert_eq!(last.as_millis(), last_ms);
        assert_eq!(f.ui.session.context_index, 1);
        board_frame(f, &ctx, 0.02, &settings);
        assert_current_slide(f, 0.02, last, Square::E7, Square::E5);
        assert!(f.ui.message.is_none());

        f.ui.tick_at(start + first + last, normal(), NOW);
        assert_eq!(f.ui.session.context_index, 2);
        assert!(f.ui.can_input());
        board_frame(f, &ctx, 10.0, &settings);
        assert!(
            f.ui.board.animation_at(10.0).is_none(),
            "the final position must not start a settings-duration slide"
        );
        assert_eq!(f.attempts().len(), 2);
    }

    #[test]
    fn adaptive_board_animation_matches_actual_slides_and_starts_at_from() {
        let mut fast = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::White,
            &["g1f3"],
            "adaptive-fast",
        );
        let e4 = key_for(&fast.library, &fast.scope, "e2e4");
        fast.ui.learning.ensure_item(e4).correct_streak = 3;
        fast.ui.session.context_policy.use_fixed_piece_speed = false;
        fast.ui.session.context_policy.move_speed_adjustment = 1.5;
        assert_adaptive_board_slides(&mut fast, 83, 166);

        let mut slow = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::White,
            &["g1f3"],
            "adaptive-slow",
        );
        slow.ui.session.context_policy.use_fixed_piece_speed = false;
        slow.ui.session.context_policy.move_speed_adjustment = 0.0;
        assert_adaptive_board_slides(&mut slow, 1000, 1000);
    }

    #[test]
    fn reinforcement_does_not_count_as_completed_main_target() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::White,
            &["g1f3"],
            "reinforcement-progress",
        );
        f.ui.session.context_policy.consider_leadup_moves = true;
        let start = Instant::now();
        f.ui.tick_at(start, Duration::ZERO, NOW);
        assert_eq!(
            f.ui.session.phase,
            LearningSessionPhase::AwaitingContextIndependent
        );
        answer_current(&mut f, start, NOW, "weak-e4-reinforcement");
        let e4 = key_for(&f.library, &f.scope, "e2e4");
        assert!(f.ui.session.completed_keys.contains(&e4));
        assert_eq!(
            f.ui.session.current_target().unwrap().key.expected_uci,
            "g1f3"
        );
        assert_eq!(main_task_progress(&f.ui.session), (0, 1));

        answer_current(&mut f, start, NOW + 1, "main-nf3-target");
        assert_eq!(main_task_progress(&f.ui.session), (1, 1));
    }

    #[test]
    fn automatic_context_has_three_separate_normal_timed_steps() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "context-normal",
        );
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        assert_eq!(f.ui.visual_log, ["e2e4"]);
        assert_eq!(f.ui.session.context_index, 0);
        assert!(!f.ui.can_input());

        f.ui.tick_at(start + Duration::from_millis(249), normal(), NOW);
        assert_eq!(f.ui.session.context_index, 0);
        assert!(f.attempts().is_empty());

        f.ui.tick_at(start + normal(), normal(), NOW);
        assert_eq!(f.ui.session.context_index, 1);
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5"]);
        f.ui.tick_at(start + normal(), normal(), NOW);
        assert_eq!(f.ui.session.context_index, 1);

        f.ui.tick_at(start + normal() * 2, normal(), NOW);
        assert_eq!(f.ui.session.context_index, 2);
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5", "g1f3"]);
        f.ui.tick_at(start + normal() * 3, normal(), NOW);
        assert_eq!(f.ui.session.context_index, 3);
        assert_eq!(
            f.ui.session.phase,
            LearningSessionPhase::AwaitingIndependent
        );
        assert!(f.ui.can_input());
        assert_eq!(f.attempts().len(), 3);
        assert_eq!(
            trainer::repertoire::fen4(&f.ui.board_position),
            key_for(&f.library, &f.scope, "b8c6").position_fen4
        );
    }

    #[test]
    fn pause_after_second_step_resumes_with_third_without_duplicate() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "pause-after-second",
        );
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        f.ui.tick_at(start + normal(), normal(), NOW);
        assert_eq!(f.ui.session.context_index, 1);
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5"]);

        assert!(f.ui.request_pause());
        assert!(f.ui.pause_requested);
        assert!(matches!(f.ui.playback, Some(Playback::Automatic { .. })));
        assert_eq!(f.attempts().len(), 1);
        f.ui.tick_at(start + normal() * 2, normal(), NOW);
        assert!(f.ui.autoplay_paused());
        assert_eq!(f.ui.session.context_index, 2);
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5"]);
        assert_eq!(f.attempts().len(), 2);
        assert!(!f.ui.can_input());

        f.ui.tick_at(start + normal() * 10, normal(), NOW);
        assert_eq!(f.ui.session.context_index, 2);
        assert_eq!(f.attempts().len(), 2);
        assert!(f.ui.continue_autoplay(start + normal() * 20));
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5", "g1f3"]);
        assert_eq!(f.ui.session.context_index, 2);
        assert_eq!(f.attempts().len(), 2);
        f.ui.tick_at(start + normal() * 20, normal(), NOW);
        assert_eq!(f.attempts().len(), 2);

        f.ui.tick_at(start + normal() * 21, normal(), NOW);
        assert_eq!(f.ui.session.context_index, 3);
        assert_eq!(f.attempts().len(), 3);
        assert!(f.ui.can_input());
        assert!(f.ui.playback.is_none());
    }

    #[test]
    fn autoplay_keyboard_controls_stop_without_new_attempts() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "autoplay-keyboard-stop",
        );
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        let ctx = egui::Context::default();
        let press = |key: Key,
                     training: &mut LearningTrainingUi,
                     actions: &mut Vec<LearningTrainingAction>| {
            ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::NONE,
                    }],
                    time: Some(0.0),
                    ..Default::default()
                },
                |ui| training.keyboard(ui, actions),
            )
            .drop_without_applying_deltas();
        };
        let mut actions = Vec::new();
        press(Key::P, &mut f.ui, &mut actions);
        assert!(f.ui.pause_requested);
        assert!(actions.is_empty());

        f.ui.tick_at(start + normal(), normal(), NOW);
        assert!(f.ui.autoplay_paused());
        assert_eq!(f.attempts().len(), 1);
        press(Key::Space, &mut f.ui, &mut actions);
        assert!(matches!(f.ui.playback, Some(Playback::Automatic { .. })));
        assert_eq!(f.ui.session.context_index, 1);
        assert_eq!(f.attempts().len(), 1);

        press(Key::Escape, &mut f.ui, &mut actions);
        assert!(matches!(actions.as_slice(), [LearningTrainingAction::Back]));
        assert!(f.ui.stopped);
        assert!(f.ui.playback.is_none());
        assert!(!f.ui.can_input());
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5"]);
        f.ui.tick_at(start + normal() * 20, normal(), NOW);
        assert_eq!(f.attempts().len(), 1);
        let data = f.store.load().unwrap();
        assert_eq!(data.learning.attempts.len(), 1);
        assert_eq!(
            data.session_snapshot(f.ui.session_id())
                .unwrap()
                .context_index,
            1
        );
    }

    #[test]
    fn animation_off_commits_every_context_step_without_skipping_store_calls() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "context-off",
        );
        f.ui.tick_at(Instant::now(), Duration::ZERO, NOW);
        assert_eq!(f.ui.visual_log, ["e2e4", "e7e5", "g1f3"]);
        assert_eq!(f.ui.session.context_index, 3);
        assert!(f.ui.can_input());
        let attempts = f.attempts();
        assert_eq!(attempts.len(), 3);
        assert_eq!(
            attempts
                .iter()
                .map(|attempt| attempt.actual_uci.as_deref())
                .collect::<Vec<_>>(),
            [Some("e2e4"), Some("e7e5"), Some("g1f3")]
        );
        assert!(attempts.iter().all(|attempt| {
            attempt.event_type == TrainingEventType::AutoContext
                && attempt.response_time_ms.is_none()
        }));
    }

    #[test]
    fn legal_input_is_ignored_during_autoplay() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "autoplay-input",
        );
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        assert!(!f.ui.play_uci_with_id_at(
            "e7e5",
            "blocked-input".to_owned(),
            start + Duration::from_millis(20),
            NOW,
        ));
        assert_eq!(f.ui.session.context_index, 0);
        assert!(f.attempts().is_empty());
        assert_eq!(f.ui.visual_log, ["e2e4"]);
    }

    #[test]
    fn wrong_answer_replays_without_a_second_independent_error() {
        let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "wrong-replay");
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        assert!(f.ui.play_uci_with_id_at(
            "d2d4",
            "wrong-d4".to_owned(),
            start + Duration::from_millis(1500),
            NOW,
        ));
        assert!(matches!(f.ui.playback, Some(Playback::Wrong { .. })));
        f.ui.tick_at(
            start + Duration::from_millis(1500) + WRONG_SHOWN,
            normal(),
            NOW,
        );
        assert_eq!(
            f.ui.session.phase,
            LearningSessionPhase::AwaitingReplayAfterError
        );
        assert_eq!(
            f.ui.presentation.as_ref().unwrap().revealed_move.as_deref(),
            Some("e2e4")
        );

        assert!(f.ui.play_uci_with_id_at(
            "d2d4",
            "wrong-replay-d4".to_owned(),
            start + Duration::from_secs(3),
            NOW,
        ));
        assert_eq!(f.attempts().len(), 1);
        f.ui.tick_at(start + Duration::from_secs(3) + WRONG_SHOWN, normal(), NOW);
        assert!(f.ui.play_uci_with_id_at(
            "e2e4",
            "replay-e4".to_owned(),
            start + Duration::from_secs(4),
            NOW,
        ));
        f.ui.tick_at(start + Duration::from_secs(4) + normal(), normal(), NOW);
        assert!(f.ui.is_finished());
        let attempts = f.attempts();
        assert_eq!(attempts.len(), 2);
        assert_eq!(
            attempts[0].event_type,
            TrainingEventType::IncorrectIndependent
        );
        assert_eq!(attempts[0].response_time_ms, Some(1500));
        assert_eq!(attempts[1].event_type, TrainingEventType::ReplayAfterError);
        assert_eq!(attempts[1].response_time_ms, None);
        let item = f
            .store
            .load()
            .unwrap()
            .learning
            .item(&key_for(&f.library, &f.scope, "e2e4"))
            .unwrap()
            .clone();
        assert_eq!(item.wrong_count, 1);
        assert_eq!(item.correct_count, 0);
    }

    #[test]
    fn preview_is_idempotent_and_shown_move_is_not_success() {
        let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "preview");
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        assert!(f.ui.request_hint_with_id_at(
            "preview-e4".to_owned(),
            start + Duration::from_millis(100),
            NOW,
        ));
        assert_eq!(f.ui.session.phase, LearningSessionPhase::PreviewedReplay);
        assert!(!f.ui.request_hint_with_id_at(
            "preview-e4-again".to_owned(),
            start + Duration::from_millis(200),
            NOW,
        ));
        assert_eq!(f.attempts().len(), 1);
        assert!(f.ui.play_uci_with_id_at(
            "e2e4",
            "play-preview-e4".to_owned(),
            start + Duration::from_secs(1),
            NOW,
        ));
        f.ui.tick_at(start + Duration::from_secs(1) + normal(), normal(), NOW);
        assert!(f.ui.is_finished());
        assert!(
            f.ui.message
                .as_deref()
                .is_some_and(|text| text.contains("PREVIEWED"))
        );
        let data = f.store.load().unwrap();
        assert_eq!(data.learning.attempts.len(), 2);
        assert_eq!(
            data.learning.attempts[0].event_type,
            TrainingEventType::Previewed
        );
        assert_eq!(
            data.learning.attempts[1].event_type,
            TrainingEventType::AutoContext
        );
        let item = data
            .learning
            .item(&key_for(&f.library, &f.scope, "e2e4"))
            .unwrap();
        assert_eq!(item.correct_count, 0);
        assert_eq!(item.wrong_count, 0);
        assert!(item.independent_response_times_ms.is_empty());
    }

    #[test]
    fn duplicate_store_response_does_not_advance_the_ui_twice() {
        let mut f = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "duplicate",
        );
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        assert!(f.ui.advance_context_with_id_at(
            "duplicate-context".to_owned(),
            start + normal(),
            NOW,
        ));
        assert_eq!(f.ui.session.context_index, 1);
        assert_eq!(f.attempts().len(), 1);

        f.ui.completed_attempt_ids.remove("duplicate-context");
        assert!(f.ui.advance_context_with_id_at(
            "duplicate-context".to_owned(),
            start + normal() * 2,
            NOW,
        ));
        assert_eq!(f.ui.session.context_index, 1);
        assert_eq!(f.attempts().len(), 1);
        assert!(!f.ui.visual_log.contains(&"g1f3".to_owned()));
    }

    #[test]
    fn store_error_is_visible_blocking_and_retryable() {
        let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "store-error");
        let start = Instant::now();
        f.ui.tick_at(start, normal(), NOW);
        let valid_store = f.ui.store.clone();
        f.ui.store = LearningStore::with_legacy(valid_store.path(), valid_store.path());
        assert!(f.ui.play_uci_with_id_at(
            "e2e4",
            "save-error-e4".to_owned(),
            start + Duration::from_millis(100),
            NOW,
        ));
        assert!(f.ui.has_unsaved());
        assert!(f.ui.save_error.is_some());
        assert!(!f.ui.can_input());
        assert!(!f.ui.is_finished());
        assert!(f.attempts().is_empty());
        assert!(!f.ui.play_uci_with_id_at(
            "d2d4",
            "blocked-after-save-error".to_owned(),
            start + Duration::from_millis(200),
            NOW,
        ));

        f.ui.store = valid_store;
        f.ui.retry_pending(start + Duration::from_millis(300));
        assert!(!f.ui.has_unsaved());
        assert!(matches!(f.ui.playback, Some(Playback::Accepted { .. })));
        f.ui.tick_at(start + Duration::from_millis(300) + normal(), normal(), NOW);
        assert!(f.ui.is_finished());
        assert_eq!(f.attempts().len(), 1);
    }

    #[test]
    fn reload_resumes_preview_and_partially_presented_context() {
        let mut preview = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "resume-preview");
        let start = Instant::now();
        preview.ui.tick_at(start, normal(), NOW);
        preview
            .ui
            .request_hint_with_id_at("resume-preview-event".to_owned(), start, NOW);
        let id = preview.ui.session_id().to_owned();
        let resumed = LearningTrainingUi::new(
            preview.library.clone(),
            preview.store.clone(),
            preview.scope.clone(),
            SrsSettings::default(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(resumed.session_id(), id);
        assert_eq!(resumed.session.phase, LearningSessionPhase::PreviewedReplay);
        assert_eq!(preview.store.load().unwrap().learning.attempts.len(), 1);

        let mut context = Fixture::new(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::Black,
            &["b8c6"],
            "resume-context",
        );
        context.ui.tick_at(start, normal(), NOW);
        context.ui.tick_at(start + normal(), normal(), NOW);
        assert_eq!(context.ui.session.context_index, 1);
        let id = context.ui.session_id().to_owned();
        drop(context.ui);
        let mut resumed = LearningTrainingUi::new(
            context.library.clone(),
            context.store.clone(),
            context.scope.clone(),
            SrsSettings::default(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(resumed.session_id(), id);
        assert_eq!(resumed.session.context_index, 1);
        resumed.board_shown = true;
        resumed.tick_at(start + normal() * 2, Duration::ZERO, NOW);
        assert_eq!(resumed.session.context_index, 3);
        assert_eq!(context.store.load().unwrap().learning.attempts.len(), 3);
    }

    #[test]
    fn new_items_require_the_explicit_introduction_flag() {
        let due = Fixture::new("1. e4 *", LearningSide::White, &[], "due-empty");
        assert!(due.ui.session.queue.is_empty());
        assert!(due.ui.is_finished());
        assert!(due.store.load().unwrap().learning.items.is_empty());

        let introduced = Fixture::with_introduction(
            "1. e4 *",
            LearningSide::White,
            &[],
            "introduce-explicit",
            true,
        );
        assert_eq!(introduced.ui.session.queue.len(), 1);
        assert!(introduced.store.load().unwrap().learning.items.is_empty());
    }

    #[test]
    fn introduce_new_starts_separate_session_and_preserves_previewed_snapshot() {
        let pgn = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 *";
        let mut f = Fixture::with_introduction(
            pgn,
            LearningSide::White,
            &[],
            "introduce-preserves-preview",
            true,
        );
        let start = Instant::now();
        f.ui.tick_at(start, Duration::ZERO, NOW);
        assert!(f.ui.request_hint_with_id_at(
            "preview-before-new-introduction".to_owned(),
            start,
            NOW,
        ));
        let old_id = f.ui.session_id().to_owned();

        let introduced = LearningTrainingUi::new(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            true,
            false,
        )
        .unwrap();

        assert_ne!(introduced.session_id(), old_id);
        assert_eq!(introduced.session.queue.len(), 6);
        assert!(
            introduced
                .session
                .queue
                .iter()
                .all(|target| target.key.expected_uci != "e2e4")
        );
        let data = f.store.load().unwrap();
        assert_eq!(data.session_snapshots.len(), 2);
        let old = data.session_snapshot(&old_id).unwrap();
        assert_eq!(old.phase, LearningSessionPhase::PreviewedReplay);
        assert_eq!(old.queue.len(), 7);
        assert!(data.session_snapshot(introduced.session_id()).is_some());
    }

    #[test]
    fn review_in_order_second_cycle_restarts_progress_and_updates_srs() {
        let mut f = Fixture::with_mode(
            "1. e4 e5 2. Nf3 Nc6 *",
            LearningSide::White,
            &[],
            "review-second-cycle",
            QueueMode::ReviewInOrder,
            LearningPriority::DepthFirst,
        );
        let start = Instant::now();
        f.ui.tick_at(start, Duration::ZERO, NOW);
        answer_current(
            &mut f,
            start + Duration::from_millis(10),
            NOW,
            "review-cycle0-e4",
        );
        assert_eq!(f.ui.session.cycle, 0);
        assert_eq!(
            f.ui.session.current_target().unwrap().key.expected_uci,
            "g1f3"
        );
        answer_current(
            &mut f,
            start + Duration::from_millis(20),
            NOW + 1,
            "review-cycle0-nf3",
        );
        assert_eq!(f.ui.session.cycle, 1);
        assert_eq!(
            f.ui.session.current_target().unwrap().key.expected_uci,
            "e2e4"
        );
        assert!(f.ui.session.completed_keys.is_empty());
        assert!(!f.ui.is_finished());

        answer_current(
            &mut f,
            start + Duration::from_millis(30),
            NOW + 24 * 60 * 60,
            "review-cycle1-e4",
        );
        assert_eq!(f.ui.session.cycle, 1);
        assert_eq!(
            f.ui.session.current_target().unwrap().key.expected_uci,
            "g1f3"
        );
        assert_eq!(f.ui.session.completed_keys.len(), 1);
        let e4 = key_for(&f.library, &f.scope, "e2e4");
        let item = f.store.load().unwrap().learning.item(&e4).unwrap().clone();
        assert_eq!(item.correct_count, 2);
        assert!(item.next_review_at > Some(NOW + 24 * 60 * 60));
    }

    #[test]
    fn least_recent_context_is_independent_once_and_new_pass_resumes() {
        let mut f = Fixture::with_mode(
            "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *",
            LearningSide::White,
            &[("e2e4", NOW - 10), ("g1f3", NOW - 100), ("f1b5", NOW - 50)],
            "least-context-once",
            QueueMode::LeastRecent,
            LearningPriority::DepthFirst,
        );
        assert_eq!(
            f.ui.session.phase,
            LearningSessionPhase::AwaitingContextIndependent
        );
        assert_eq!(f.ui.session.current_expected_uci(), Some("e2e4"));
        assert_eq!(
            f.ui.session.current_target().unwrap().key.expected_uci,
            "g1f3"
        );
        let start = Instant::now();
        f.ui.tick_at(start, Duration::ZERO, NOW);
        assert!(f.ui.can_input());
        assert!(f.ui.playback.is_none());
        assert!(f.ui.visual_log.is_empty());
        assert!(
            f.ui.message
                .as_deref()
                .unwrap()
                .contains(tr("Automatic move/context"))
        );

        answer_current(
            &mut f,
            start + Duration::from_millis(10),
            NOW,
            "least-context-e4",
        );
        assert_eq!(
            f.ui.session.phase,
            LearningSessionPhase::AwaitingIndependent
        );
        assert_eq!(f.ui.session.current_expected_uci(), Some("g1f3"));
        answer_current(
            &mut f,
            start + Duration::from_millis(20),
            NOW + 1,
            "least-target-nf3",
        );
        assert_eq!(
            f.ui.session.current_target().unwrap().key.expected_uci,
            "f1b5"
        );
        for expected in ["e2e4", "g1f3"] {
            let key = key_for(&f.library, &f.scope, expected);
            assert_eq!(
                f.attempts()
                    .iter()
                    .filter(|attempt| {
                        attempt.item_key() == key && attempt.event_type.is_independent()
                    })
                    .count(),
                1,
                "context was checked twice for {expected}"
            );
        }
        answer_current(
            &mut f,
            start + Duration::from_millis(30),
            NOW + 2,
            "least-target-bb5",
        );
        assert!(f.ui.is_finished());
        let attempts = f.attempts();
        for expected in ["e2e4", "g1f3", "f1b5"] {
            let key = key_for(&f.library, &f.scope, expected);
            assert_eq!(
                attempts
                    .iter()
                    .filter(|attempt| {
                        attempt.item_key() == key && attempt.event_type.is_independent()
                    })
                    .count(),
                1,
                "duplicate independent review for {expected}"
            );
            let item = f.store.load().unwrap().learning.item(&key).unwrap().clone();
            assert_eq!(item.correct_count, 2);
        }

        let id = f.ui.session_id().to_owned();
        drop(f.ui);
        let resumed = LearningTrainingUi::new_with_mode(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            false,
            false,
            QueueMode::LeastRecent,
        )
        .unwrap();
        assert_eq!(resumed.session_id(), id);
        assert_eq!(resumed.session.cycle, 1);
        assert_eq!(resumed.session.session.mode, QueueMode::LeastRecent);
        assert!(resumed.target_available());
    }

    #[test]
    fn resume_is_scoped_by_mode_and_learning_priority() {
        let mut f = Fixture::with_mode(
            "1. e4 (1. d4 d5) e5 2. Nf3 *",
            LearningSide::White,
            &[],
            "resume-mode-priority",
            QueueMode::ReviewInOrder,
            LearningPriority::MainLinesFirst,
        );
        let start = Instant::now();
        f.ui.tick_at(start, Duration::ZERO, NOW);
        answer_current(&mut f, start, NOW, "priority-review-first");
        let review_id = f.ui.session_id().to_owned();
        assert_eq!(f.ui.session.current_index, 1);

        let resumed = LearningTrainingUi::new_with_mode(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            false,
            false,
            QueueMode::ReviewInOrder,
        )
        .unwrap();
        assert_eq!(resumed.session_id(), review_id);
        assert_eq!(resumed.session.current_index, 1);
        assert_eq!(
            resumed.session.stats_scope.learning_priority,
            LearningPriority::MainLinesFirst
        );

        let other_priority = LearningTrainingUi::new_with_mode(
            f.library.clone(),
            f.store.clone(),
            f.scope
                .clone()
                .with_learning_priority(LearningPriority::DepthFirst),
            SrsSettings::default(),
            false,
            false,
            QueueMode::ReviewInOrder,
        )
        .unwrap();
        assert_ne!(other_priority.session_id(), review_id);
        assert_eq!(
            other_priority.session.stats_scope.learning_priority,
            LearningPriority::DepthFirst
        );

        let other_mode = LearningTrainingUi::new_with_mode(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            false,
            false,
            QueueMode::LeastRecent,
        )
        .unwrap();
        assert_ne!(other_mode.session_id(), review_id);
        assert_eq!(other_mode.session.session.mode, QueueMode::LeastRecent);

        let srs = LearningTrainingUi::new(
            f.library.clone(),
            f.store.clone(),
            f.scope.clone(),
            SrsSettings::default(),
            false,
            false,
        )
        .unwrap();
        assert_ne!(srs.session_id(), review_id);
        assert_eq!(srs.session.session.mode, QueueMode::SpacedRepetition);
        assert!(
            LearningTrainingUi::new_with_mode(
                f.library.clone(),
                f.store.clone(),
                f.scope.clone(),
                SrsSettings::default(),
                true,
                false,
                QueueMode::ReviewInOrder,
            )
            .is_err()
        );
        assert_eq!(
            queue_mode_label(QueueMode::SpacedRepetition),
            tr("Review individual moves")
        );
        assert_eq!(
            queue_mode_label(QueueMode::ReviewInOrder),
            tr("Review in order")
        );
        assert_eq!(
            queue_mode_label(QueueMode::LeastRecent),
            tr("Least recent and unseen first")
        );
    }

    #[test]
    fn show_runs_at_mvp_window_sizes() {
        let mut f = Fixture::new("1. e4 *", LearningSide::White, &["e2e4"], "mvp-sizes");
        let settings = Settings::default();
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        for (width, height) in [(1400.0, 900.0), (640.0, 480.0)] {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, height),
                    )),
                    time: Some(0.0),
                    ..Default::default()
                },
                |ui| {
                    f.ui.show(ui, &settings, "Test repertoire");
                },
            );
            if width == 640.0 {
                let background = egui::Visuals::light().window_fill();
                assert!(
                    output.shapes.iter().any(|clipped| {
                        let egui::Shape::Rect(shape) = &clipped.shape else {
                            return false;
                        };
                        shape.fill == background
                            && shape.rect.min.x <= 0.0
                            && shape.rect.max.x >= width
                            && shape.rect.min.y > 0.0
                            && shape.rect.min.y <= height - 58.0
                            && shape.rect.max.y >= height - 0.01
                    }),
                    "the stacked background must cover the bottom of the viewport"
                );
            }
            output.drop_without_applying_deltas();
        }
        assert!(f.ui.target_available());
        assert!(!f.ui.has_unsaved());
    }
}
