use std::collections::BTreeSet;

use chess_core::shakmaty::{CastlingMode, Chess, Move, Position, uci::UciMove};
use chess_core::{Game, NodeId};

use super::model::{Repertoire, RepertoireId};
use super::scope::TrainingScope;
use super::stats::{MoveKey, PathKey, QueueMode, RunRecord, SrsPolicy, StatsData, TrainingMode};
use super::tree::{Candidate, RepertoireTree, TrainingPath, TrainingTarget, fen4};

pub trait MoveChooser: Send {
    fn choose(&mut self, candidates: &[Candidate]) -> Option<usize>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FirstMoveChooser;

impl MoveChooser for FirstMoveChooser {
    fn choose(&mut self, candidates: &[Candidate]) -> Option<usize> {
        (!candidates.is_empty()).then_some(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayedMove {
    pub uci: String,
    pub san: String,
    pub automatic: bool,
    pub position_before: String,
    pub position_after: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionAttempt {
    pub position_fen4: String,
    pub expected_uci: String,
    pub actual_uci: String,
    pub correct: bool,
    pub hinted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrainingVerdict {
    Right,
    Finished,
    Wrong,
    TryAgain,
    Done,
}

#[derive(Debug)]
pub enum SessionError {
    InvalidUci(String),
    IllegalMove(String),
    Chooser,
    MissingTargetPath,
    InvalidScope(String),
    InvalidSrsPolicy,
    InvalidQueueMode(&'static str),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUci(uci) => write!(f, "invalid UCI move: {uci}"),
            Self::IllegalMove(uci) => write!(f, "illegal move in the current position: {uci}"),
            Self::Chooser => f.write_str("the continuation chooser returned no valid candidate"),
            Self::MissingTargetPath => {
                f.write_str("the target path is no longer present in the repertoire")
            }
            Self::InvalidScope(message) => write!(f, "invalid training scope: {message}"),
            Self::InvalidSrsPolicy => {
                f.write_str("SRS policies apply only to White/Black spaced repetition")
            }
            Self::InvalidQueueMode(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SessionError {}

/// A live tree-training state machine. Statistics are owned by the session so callers
/// can persist them after each accepted event or at a lifecycle boundary they choose.
pub struct TrainingSession {
    repertoire_id: RepertoireId,
    mode: TrainingMode,
    queue_mode: QueueMode,
    scope: TrainingScope,
    srs_policy: SrsPolicy,
    tree: RepertoireTree,
    stats: StatsData,
    chooser: Box<dyn MoveChooser>,
    current: NodeId,
    position: Chess,
    target: Option<TrainingTarget>,
    suggested_path: Option<TrainingPath>,
    seen_positions: BTreeSet<String>,
    actual_path: Vec<String>,
    done: bool,
    completion_recorded: bool,
    current_wrong: u32,
    current_hinted: bool,
    hint_visible: bool,
    run_hinted: bool,
    mistakes: u32,
    hints: u32,
    path: Vec<PlayedMove>,
    attempts: Vec<SessionAttempt>,
    completed_targets: BTreeSet<MoveKey>,
    deferred_targets: BTreeSet<MoveKey>,
}

impl TrainingSession {
    pub fn new(
        repertoire: &Repertoire,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        now: i64,
    ) -> Result<Self, SessionError> {
        Self::with_chooser(
            repertoire,
            mode,
            queue_mode,
            stats,
            now,
            Box::<FirstMoveChooser>::default(),
        )
    }

    pub fn with_chooser(
        repertoire: &Repertoire,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        now: i64,
        chooser: Box<dyn MoveChooser>,
    ) -> Result<Self, SessionError> {
        Self::scoped_with_chooser(
            repertoire,
            TrainingScope::whole(),
            mode,
            queue_mode,
            stats,
            now,
            chooser,
        )
    }

    pub fn scoped(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        now: i64,
    ) -> Result<Self, SessionError> {
        Self::scoped_with_chooser(
            repertoire,
            scope,
            mode,
            queue_mode,
            stats,
            now,
            Box::<FirstMoveChooser>::default(),
        )
    }

    pub fn new_srs(
        repertoire: &Repertoire,
        mode: TrainingMode,
        stats: StatsData,
        now: i64,
        policy: SrsPolicy,
    ) -> Result<Self, SessionError> {
        Self::scoped_srs(repertoire, TrainingScope::whole(), mode, stats, now, policy)
    }

    pub fn scoped_srs(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        stats: StatsData,
        now: i64,
        policy: SrsPolicy,
    ) -> Result<Self, SessionError> {
        Self::scoped_srs_with_chooser(
            repertoire,
            scope,
            mode,
            stats,
            now,
            policy,
            Box::<FirstMoveChooser>::default(),
        )
    }

    pub fn scoped_with_chooser(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        now: i64,
        chooser: Box<dyn MoveChooser>,
    ) -> Result<Self, SessionError> {
        Self::build(
            repertoire,
            scope,
            mode,
            queue_mode,
            stats,
            now,
            (None, chooser),
        )
    }

    pub fn scoped_srs_with_chooser(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        stats: StatsData,
        now: i64,
        policy: SrsPolicy,
        chooser: Box<dyn MoveChooser>,
    ) -> Result<Self, SessionError> {
        Self::build(
            repertoire,
            scope,
            mode,
            QueueMode::SpacedRepetition,
            stats,
            now,
            (Some(policy), chooser),
        )
    }

    fn build(
        repertoire: &Repertoire,
        scope: TrainingScope,
        mode: TrainingMode,
        queue_mode: QueueMode,
        stats: StatsData,
        now: i64,
        options: (Option<SrsPolicy>, Box<dyn MoveChooser>),
    ) -> Result<Self, SessionError> {
        let (policy, chooser) = options;
        if policy.is_some()
            && (mode == TrainingMode::Both || queue_mode != QueueMode::SpacedRepetition)
        {
            return Err(SessionError::InvalidSrsPolicy);
        }
        let tree = RepertoireTree::new(repertoire);
        let current = tree
            .scope_anchor(&scope)
            .map_err(SessionError::InvalidScope)?;
        let srs_policy = policy.unwrap_or(SrsPolicy::Due);
        let mut session = Self {
            repertoire_id: repertoire.id.clone(),
            mode,
            queue_mode,
            scope,
            srs_policy,
            stats,
            chooser,
            position: tree
                .position(current)
                .cloned()
                .unwrap_or_else(|| tree.game().start().position().clone()),
            current,
            tree,
            target: None,
            suggested_path: None,
            seen_positions: BTreeSet::new(),
            actual_path: Vec::new(),
            done: false,
            completion_recorded: false,
            current_wrong: 0,
            current_hinted: false,
            hint_visible: false,
            run_hinted: false,
            mistakes: 0,
            hints: 0,
            path: Vec::new(),
            attempts: Vec::new(),
            completed_targets: BTreeSet::new(),
            deferred_targets: BTreeSet::new(),
        };
        session.initialize(now, true)?;
        Ok(session)
    }

    pub fn game(&self) -> &Game {
        self.tree.game()
    }

    pub fn stats(&self) -> &StatsData {
        &self.stats
    }

    pub fn stats_mut(&mut self) -> &mut StatsData {
        &mut self.stats
    }

    pub fn into_stats(self) -> StatsData {
        self.stats
    }

    pub fn mode(&self) -> TrainingMode {
        self.mode
    }

    pub fn queue_mode(&self) -> QueueMode {
        self.queue_mode
    }

    pub fn scope(&self) -> &TrainingScope {
        &self.scope
    }

    pub fn srs_policy(&self) -> SrsPolicy {
        self.srs_policy
    }

    pub fn suggested_path(&self) -> Option<&[String]> {
        self.suggested_path
            .as_ref()
            .map(|path| path.moves.as_slice())
    }

    pub fn current_node(&self) -> NodeId {
        self.current
    }

    pub fn position(&self) -> &Chess {
        &self.position
    }

    /// Full position at the scope anchor, before any automatic opponent move.
    /// Unlike FEN4 this retains the counters needed for faithful UI replay.
    pub fn scope_start_position(&self) -> &Chess {
        self.tree
            .scope_anchor(&self.scope)
            .ok()
            .and_then(|anchor| self.tree.position(anchor))
            .unwrap_or_else(|| self.tree.game().start().position())
    }

    pub fn position_key(&self) -> String {
        fen4(self.position())
    }

    pub fn target(&self) -> Option<&TrainingTarget> {
        self.target.as_ref()
    }

    pub fn candidates(&self) -> Vec<Candidate> {
        if self.done || self.at_depth_boundary() {
            Vec::new()
        } else {
            self.tree.scoped_candidates(self.current, &self.scope)
        }
    }

    pub fn path(&self) -> &[PlayedMove] {
        &self.path
    }

    pub fn attempts(&self) -> &[SessionAttempt] {
        &self.attempts
    }

    pub fn errors(&self) -> impl Iterator<Item = &SessionAttempt> {
        self.attempts.iter().filter(|attempt| !attempt.correct)
    }

    pub fn total_mistakes(&self) -> u32 {
        self.mistakes
    }

    pub fn hinted(&self) -> bool {
        self.run_hinted
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn may_hint(&self) -> bool {
        !self.done && (self.queue_mode == QueueMode::Practice || self.current_wrong >= 3)
    }

    /// Practice offers a hint immediately; scheduled repetition unlocks it after three
    /// wrong attempts. Practice follows its proposed branch while the actual path
    /// matches it, then falls back to the first enabled continuation. Every enabled
    /// move remains a valid answer.
    pub fn hint(&mut self, now: i64) -> Option<Candidate> {
        if !self.may_hint() {
            return None;
        }
        let candidate = match &self.target {
            Some(target) => {
                self.tree
                    .scoped_candidate(self.current, &target.expected_uci, &self.scope)
            }
            None => {
                let candidates = self.tree.scoped_candidates(self.current, &self.scope);
                let proposed = self
                    .suggested_path
                    .as_ref()
                    .filter(|path| {
                        self.queue_mode == QueueMode::Practice
                            && path.moves.starts_with(&self.actual_path)
                    })
                    .and_then(|path| path.moves.get(self.actual_path.len()));
                proposed
                    .and_then(|uci| candidates.iter().find(|candidate| &candidate.uci == uci))
                    .cloned()
                    .or_else(|| candidates.into_iter().next())
            }
        }?;
        if self.hint_visible {
            return Some(candidate);
        }
        let key = self.key_for(&candidate.uci);
        self.stats.record_hint(key, now);
        self.hints = self.hints.saturating_add(1);
        self.current_hinted = true;
        self.run_hinted = true;
        self.hint_visible = true;
        Some(candidate)
    }

    pub fn play_uci(&mut self, uci: &str, now: i64) -> Result<TrainingVerdict, SessionError> {
        if self.done {
            return Ok(TrainingVerdict::Done);
        }
        let parsed = uci
            .parse::<UciMove>()
            .map_err(|_| SessionError::InvalidUci(uci.to_owned()))?;
        let mv = parsed
            .to_move(self.position())
            .map_err(|_| SessionError::IllegalMove(uci.to_owned()))?;
        self.play(mv, now)
    }

    pub fn play(&mut self, mv: Move, now: i64) -> Result<TrainingVerdict, SessionError> {
        if self.done {
            return Ok(TrainingVerdict::Done);
        }
        if !self.position().is_legal(mv) {
            return Err(SessionError::IllegalMove(
                mv.to_uci(CastlingMode::Standard).to_string(),
            ));
        }
        let actual_uci = mv.to_uci(CastlingMode::Standard).to_string();
        let candidates = self.candidates();
        if self.queue_mode == QueueMode::Practice {
            if let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.mv == mv)
                .cloned()
            {
                return self.accept_practice(candidate, now);
            }
            if let Some(expected) = candidates.first() {
                self.reject(expected, &actual_uci, now);
            }
            return Ok(TrainingVerdict::Wrong);
        }
        if self.mode == TrainingMode::Both {
            if let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.mv == mv)
                .cloned()
            {
                return Ok(self.accept_both(candidate, now));
            }
            if let Some(expected) = candidates.first() {
                self.reject(expected, &actual_uci, now);
            }
            return Ok(TrainingVerdict::Wrong);
        }

        let Some(target) = self.target.clone() else {
            return Ok(TrainingVerdict::Done);
        };
        if target.expected_uci == actual_uci {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.uci == actual_uci)
                .cloned()
                .ok_or(SessionError::MissingTargetPath)?;
            return self.accept_one_side(candidate, now);
        }
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.mv == mv)
            .cloned()
        {
            return self.accept_one_side(candidate, now);
        }
        let expected = candidates
            .iter()
            .find(|candidate| candidate.uci == target.expected_uci)
            .cloned()
            .ok_or(SessionError::MissingTargetPath)?;
        self.reject(&expected, &actual_uci, now);
        Ok(TrainingVerdict::Wrong)
    }

    /// Repeats the same proposed path (or restarts the current one-side queue). It does
    /// not advance the Both path cursor and never counts an unfinished run as complete.
    pub fn restart(&mut self, now: i64) -> Result<(), SessionError> {
        if self.queue_mode == QueueMode::Practice && !self.actual_path.is_empty() {
            self.suggested_path = Some(TrainingPath {
                moves: self.actual_path.clone(),
            });
        }
        self.reset_run()?;
        self.initialize(now, false)
    }

    /// Selects the next proposed root/scope-to-boundary path. Enabled deviations remain
    /// free; the actual path, rather than this proposal, receives completion statistics.
    pub fn next_path(&mut self, now: i64) -> Result<(), SessionError> {
        if self.mode != TrainingMode::Both && self.queue_mode != QueueMode::Practice {
            return Err(SessionError::InvalidQueueMode(
                "next_path is available in branch practice or Both mode",
            ));
        }
        self.reset_run()?;
        self.suggested_path = None;
        self.initialize(now, true)
    }

    pub fn restart_with_policy(&mut self, now: i64, policy: SrsPolicy) -> Result<(), SessionError> {
        if self.mode == TrainingMode::Both {
            return Err(SessionError::InvalidSrsPolicy);
        }
        self.queue_mode = QueueMode::SpacedRepetition;
        self.srs_policy = policy;
        self.reset_run()?;
        self.initialize(now, true)
    }

    fn reset_run(&mut self) -> Result<(), SessionError> {
        self.current = self
            .tree
            .scope_anchor(&self.scope)
            .map_err(SessionError::InvalidScope)?;
        self.target = None;
        self.done = false;
        self.completion_recorded = false;
        self.current_wrong = 0;
        self.current_hinted = false;
        self.hint_visible = false;
        self.run_hinted = false;
        self.mistakes = 0;
        self.hints = 0;
        self.path.clear();
        self.attempts.clear();
        self.actual_path.clear();
        self.seen_positions.clear();
        self.completed_targets.clear();
        self.deferred_targets.clear();
        Ok(())
    }

    fn initialize(&mut self, now: i64, select_path: bool) -> Result<(), SessionError> {
        if self.queue_mode == QueueMode::Practice {
            if select_path {
                self.suggested_path = self
                    .stats
                    .next_practice_path(&self.tree, &self.repertoire_id, self.mode, &self.scope)
                    .map_err(SessionError::InvalidScope)?;
            }
            self.prepare_scope_start()?;
            if self.suggested_path.is_none() {
                self.done = true;
                return Ok(());
            }
            self.advance_practice_opponent(now)?;
            return Ok(());
        }
        if self.mode == TrainingMode::Both {
            if select_path {
                self.suggested_path = self
                    .stats
                    .next_training_path(
                        &self.tree,
                        &self.repertoire_id,
                        &self.scope,
                        self.queue_mode,
                    )
                    .map_err(SessionError::InvalidScope)?;
                if self.suggested_path.is_some() {
                    self.stats
                        .path_presented(&self.repertoire_id, &self.scope, self.queue_mode);
                }
            }
            self.prepare_scope_start()?;
            if self.suggested_path.is_none()
                || self
                    .tree
                    .scoped_candidates(self.current, &self.scope)
                    .is_empty()
            {
                self.done = true;
            }
            Ok(())
        } else {
            self.prepare_next_target(now).map(|_| ())
        }
    }

    fn prepare_scope_start(&mut self) -> Result<(), SessionError> {
        let anchor = self
            .tree
            .scope_anchor(&self.scope)
            .map_err(SessionError::InvalidScope)?;
        if self.scope.include_context {
            let context = self
                .tree
                .path(anchor)
                .map(<[String]>::to_vec)
                .ok_or(SessionError::MissingTargetPath)?;
            self.current = self.tree.root();
            self.position = self.tree.game().start().position().clone();
            for uci in context {
                let candidate = self
                    .tree
                    .candidate(self.current, &uci)
                    .ok_or(SessionError::MissingTargetPath)?;
                self.advance(candidate, true);
            }
        } else {
            self.current = anchor;
            self.position = self
                .tree
                .position(anchor)
                .cloned()
                .ok_or(SessionError::MissingTargetPath)?;
        }
        self.seen_positions.insert(self.position_key());
        Ok(())
    }

    fn at_depth_boundary(&self) -> bool {
        if self.mode != TrainingMode::Both && self.queue_mode != QueueMode::Practice {
            return false;
        }
        let start = self.scope.parent_prefix().len();
        start.saturating_add(self.actual_path.len()) >= self.tree.max_plies() as usize
    }

    fn practice_boundary(&self) -> bool {
        self.at_depth_boundary()
            || self
                .tree
                .scoped_candidates(self.current, &self.scope)
                .is_empty()
    }

    fn advance_practice_opponent(&mut self, now: i64) -> Result<(), SessionError> {
        if self.practice_boundary() {
            self.finish(now);
            return Ok(());
        }
        while self
            .mode
            .user_color()
            .is_some_and(|color| self.position.turn() != color)
        {
            let candidates = self.candidates();
            let suggested = self
                .suggested_path
                .as_ref()
                .filter(|path| path.moves.starts_with(&self.actual_path))
                .and_then(|path| path.moves.get(self.actual_path.len()));
            let selected = suggested
                .and_then(|uci| candidates.iter().find(|c| &c.uci == uci))
                .cloned();
            let selected = match selected {
                Some(selected) => selected,
                None => {
                    let index = self
                        .chooser
                        .choose(&candidates)
                        .filter(|index| *index < candidates.len())
                        .ok_or(SessionError::Chooser)?;
                    candidates[index].clone()
                }
            };
            self.actual_path.push(selected.uci.clone());
            self.advance(selected, true);
            let repeated = !self.seen_positions.insert(self.position_key());
            if repeated || self.practice_boundary() {
                self.finish(now);
                return Ok(());
            }
        }
        Ok(())
    }

    fn accept_practice(
        &mut self,
        candidate: Candidate,
        now: i64,
    ) -> Result<TrainingVerdict, SessionError> {
        let key = self.key_for(&candidate.uci);
        self.stats
            .record_correct(key, now, &candidate.uci, self.current_hinted, false);
        self.push_attempt(candidate.uci.clone(), candidate.uci.clone(), true);
        self.actual_path.push(candidate.uci.clone());
        self.advance(candidate, false);
        self.reset_target_state();
        if !self.seen_positions.insert(self.position_key()) || self.practice_boundary() {
            self.finish(now);
        } else {
            self.advance_practice_opponent(now)?;
        }
        Ok(if self.done {
            TrainingVerdict::Finished
        } else {
            TrainingVerdict::Right
        })
    }

    fn accept_both(&mut self, candidate: Candidate, now: i64) -> TrainingVerdict {
        let hinted = self.current_hinted;
        let key = self.key_for(&candidate.uci);
        self.stats
            .record_correct(key, now, &candidate.uci, hinted, false);
        self.push_attempt(candidate.uci.clone(), candidate.uci.clone(), true);
        self.actual_path.push(candidate.uci.clone());
        self.advance(candidate, false);
        self.reset_target_state();
        let repeated = !self.seen_positions.insert(self.position_key());
        if repeated
            || self.at_depth_boundary()
            || self
                .tree
                .scoped_candidates(self.current, &self.scope)
                .is_empty()
        {
            self.finish(now);
            TrainingVerdict::Finished
        } else {
            TrainingVerdict::Right
        }
    }

    fn accept_one_side(
        &mut self,
        candidate: Candidate,
        now: i64,
    ) -> Result<TrainingVerdict, SessionError> {
        let hinted = self.current_hinted;
        let key = self.key_for(&candidate.uci);
        if let Some(target) = &self.target
            && target.expected_uci != candidate.uci
        {
            self.deferred_targets.insert(self.target_key(target));
        }
        self.stats.record_correct(
            key.clone(),
            now,
            &candidate.uci,
            hinted,
            self.queue_mode == QueueMode::SpacedRepetition
                && self.srs_policy != SrsPolicy::EarlyReview,
        );
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.expected_uci == candidate.uci)
        {
            self.stats.target_completed(
                &self.repertoire_id,
                self.mode,
                self.queue_mode,
                &self.scope,
            );
        }
        self.completed_targets.insert(key);
        self.push_attempt(candidate.uci.clone(), candidate.uci.clone(), true);
        self.advance(candidate, false);

        let user_color = self.mode.user_color();
        if self.tree.position(self.current).map(|p| p.turn()) != user_color {
            let candidates = self.tree.scoped_candidates(self.current, &self.scope);
            if !candidates.is_empty() {
                let index = self
                    .chooser
                    .choose(&candidates)
                    .filter(|index| *index < candidates.len())
                    .ok_or(SessionError::Chooser)?;
                let selected = candidates[index].clone();
                self.advance(selected, true);
            }
        }

        if self.prepare_next_target(now)? {
            Ok(TrainingVerdict::Right)
        } else {
            self.finish(now);
            Ok(TrainingVerdict::Finished)
        }
    }

    fn reject(&mut self, expected: &Candidate, actual_uci: &str, now: i64) {
        let key = self.key_for(&expected.uci);
        self.stats.record_wrong(
            key,
            now,
            actual_uci,
            self.mode != TrainingMode::Both
                && self.queue_mode == QueueMode::SpacedRepetition
                && self.srs_policy != SrsPolicy::EarlyReview,
        );
        self.mistakes = self.mistakes.saturating_add(1);
        self.current_wrong = self.current_wrong.saturating_add(1);
        self.push_attempt(expected.uci.clone(), actual_uci.to_owned(), false);
    }

    fn key_for(&self, expected_uci: &str) -> MoveKey {
        MoveKey {
            repertoire_id: self.repertoire_id.clone(),
            mode: self.mode,
            position_fen4: self.position_key(),
            expected_uci: if self.mode == TrainingMode::Both {
                String::new()
            } else {
                expected_uci.to_owned()
            },
        }
    }

    fn push_attempt(&mut self, expected_uci: String, actual_uci: String, correct: bool) {
        self.attempts.push(SessionAttempt {
            position_fen4: self.position_key(),
            expected_uci: if self.mode == TrainingMode::Both {
                String::new()
            } else {
                expected_uci
            },
            actual_uci,
            correct,
            hinted: self.current_hinted,
        });
    }

    fn advance(&mut self, candidate: Candidate, automatic: bool) {
        let before = self.position_key();
        self.position.play_unchecked(candidate.mv);
        self.current = candidate.node;
        let after = self.position_key();
        self.path.push(PlayedMove {
            uci: candidate.uci,
            san: candidate.san,
            automatic,
            position_before: before,
            position_after: after,
        });
    }

    fn prepare_next_target(&mut self, now: i64) -> Result<bool, SessionError> {
        if self.queue_mode == QueueMode::SpacedRepetition
            && self.srs_policy == SrsPolicy::Introduce
            && self.completed_targets.len() >= super::stats::INTRODUCTION_BATCH_SIZE
        {
            self.done = true;
            self.target = None;
            return Ok(false);
        }
        let queued = if self.queue_mode == QueueMode::SpacedRepetition {
            self.stats.srs_queue(
                &self.tree,
                &self.repertoire_id,
                self.mode,
                now,
                &self.scope,
                self.srs_policy,
            )
        } else {
            self.stats.scoped_queue(
                &self.tree,
                &self.repertoire_id,
                self.mode,
                self.queue_mode,
                now,
                &self.scope,
            )
        };
        let next = queued.into_iter().find(|target| {
            !self.completed_targets.contains(&self.target_key(target))
                && !self.deferred_targets.contains(&self.target_key(target))
        });
        let Some(target) = next else {
            self.done = true;
            self.target = None;
            return Ok(false);
        };
        if self.scope.include_context {
            self.current = self.tree.root();
            self.position = self.tree.game().start().position().clone();
            for uci in &target.path {
                let candidate = self
                    .tree
                    .candidate(self.current, uci)
                    .ok_or(SessionError::MissingTargetPath)?;
                self.advance(candidate, true);
            }
        } else {
            self.current = self
                .tree
                .node_at_path(&target.path)
                .ok_or(SessionError::MissingTargetPath)?;
            self.position = self
                .tree
                .position(self.current)
                .cloned()
                .ok_or(SessionError::MissingTargetPath)?;
        }
        self.target = Some(target);
        self.reset_target_state();
        Ok(true)
    }

    fn target_key(&self, target: &TrainingTarget) -> MoveKey {
        MoveKey {
            repertoire_id: self.repertoire_id.clone(),
            mode: self.mode,
            position_fen4: target.position_fen4.clone(),
            expected_uci: target.expected_uci.clone(),
        }
    }

    fn reset_target_state(&mut self) {
        self.current_wrong = 0;
        self.current_hinted = false;
        self.hint_visible = false;
    }

    fn finish(&mut self, now: i64) {
        self.done = true;
        self.target = None;
        self.hint_visible = false;
        if self.completion_recorded || self.attempts.is_empty() {
            return;
        }
        self.completion_recorded = true;
        let path = if self.mode == TrainingMode::Both || self.queue_mode == QueueMode::Practice {
            self.actual_path.clone()
        } else {
            self.path.iter().map(|mv| mv.uci.clone()).collect()
        };
        let record = RunRecord {
            at: now,
            mistakes: self.mistakes,
            hints: self.hints,
            hinted: self.run_hinted,
            path,
            user_attempts: u32::try_from(self.attempts.len()).unwrap_or(u32::MAX),
            automatic_moves: u32::try_from(self.path.iter().filter(|mv| mv.automatic).count())
                .unwrap_or(u32::MAX),
            scope: self.scope.key(),
        };
        self.stats
            .record_run(&self.repertoire_id, self.mode, record.clone());
        if self.mode == TrainingMode::Both && !self.actual_path.is_empty() {
            self.stats.record_path_run(
                PathKey {
                    repertoire_id: self.repertoire_id.clone(),
                    scope: self.scope.key(),
                    path: self.actual_path.clone(),
                },
                record,
            );
        }
    }
}
