use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use chess_core::shakmaty::fen::Fen;
use chess_core::shakmaty::uci::UciMove;
use chess_core::shakmaty::{CastlingMode, Chess, EnPassantMode, Position};
use serde::{Deserialize, Serialize};

use super::learning::{LearningData, LearningItemKey, TrainingAttempt, TrainingEventType};
use super::learning_graph::{
    LearningGraphError, LearningItemMembership, StatsScope, active_learning_memberships,
};
use super::learning_modes::{LearningPriority, sort_by_priority, sort_least_recent};
use super::model::{LearningSide, Library, RepertoireId};
use super::scope::TrainingScopeKey;
use super::srs::SrsSettings;
use super::stats::{QueueMode, TrainingMode};
use super::store::LearningSession;
use super::tree::fen4;

#[path = "learning_context.rs"]
mod learning_context;
pub use learning_context::LearningContextPolicy;
use learning_context::{
    LearningContextAction, LearningContextMove, LearningContextPlan, plan_context, speed_factor,
};

#[cfg(test)]
#[path = "context_tests.rs"]
mod context_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LearningSessionPhase {
    AwaitingIndependent,
    AwaitingContextIndependent,
    AwaitingReplayAfterError,
    PreviewedReplay,
    AdvancingContext,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningSessionTarget {
    pub key: LearningItemKey,
    pub repertoire_id: RepertoireId,
    pub min_depth: u32,
    /// Includes the expected target move as its last element.
    pub path: Vec<String>,
    #[serde(default)]
    pub pgn_order: usize,
    #[serde(default)]
    pub variation_depth: u32,
}

impl From<LearningItemMembership> for LearningSessionTarget {
    fn from(membership: LearningItemMembership) -> Self {
        Self {
            key: membership.key,
            repertoire_id: membership.repertoire_id,
            min_depth: membership.min_depth,
            path: membership.path,
            pgn_order: membership.pgn_order,
            variation_depth: membership.variation_depth,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearningSessionSnapshot {
    pub session: LearningSession,
    #[serde(default)]
    pub context_policy: LearningContextPolicy,
    pub stats_scope: StatsScope,
    pub queue: Vec<LearningSessionTarget>,
    pub current_index: usize,
    pub phase: LearningSessionPhase,
    /// Number of already presented or deliberately omitted moves in the current target path.
    pub context_index: usize,
    pub completed_keys: BTreeSet<LearningItemKey>,
    #[serde(default)]
    pub cycle: u64,
    /// Per-session optimistic revision, independent from the store revision.
    #[serde(default)]
    pub revision: u64,
    #[serde(skip)]
    runtime: SessionRuntime,
}

// Derived data only: clones share it, persistence and snapshot equality do not.
#[derive(Clone, Default, Debug)]
struct SessionRuntime(Arc<Mutex<RuntimeData>>);

impl PartialEq for SessionRuntime {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Default, Debug)]
struct RuntimeData {
    source: Option<ReplaySource>,
    #[cfg(test)]
    parses: usize,
    #[cfg(test)]
    replays: usize,
    #[cfg(test)]
    plans: usize,
}

#[derive(Debug)]
struct ReplaySource {
    repertoire_id: RepertoireId,
    pgn: String,
    start: Chess,
    target: Option<ReplayedTarget>,
}

#[derive(Debug)]
struct ReplayedTarget {
    target: LearningSessionTarget,
    side: LearningSide,
    positions: Arc<Vec<Chess>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearningPosition {
    pub fen: String,
    pub fen4: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LearningContextStep {
    pub uci: String,
    pub from: LearningPosition,
    pub to: LearningPosition,
    pub speed_factor: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LearningPresentation {
    pub phase: LearningSessionPhase,
    pub target: Option<LearningItemKey>,
    pub target_path: Vec<String>,
    pub current_position: Option<LearningPosition>,
    /// Automatic steps that still need presentation before the independent answer.
    pub automatic_context: Vec<LearningContextStep>,
    pub revealed_move: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LearningSessionError {
    EmptySessionId,
    EmptyClientAttemptId,
    UnsupportedMode(QueueMode),
    SessionSideMismatch,
    InvalidPhase {
        operation: &'static str,
        phase: LearningSessionPhase,
    },
    IntroductionRequiresFinished,
    InvalidSnapshot(&'static str),
    RepertoireNotFound(RepertoireId),
    InvalidTargetPath {
        repertoire_id: RepertoireId,
        path: Vec<String>,
    },
    TargetPositionMismatch,
    TargetSideMismatch,
    /// A known repertoire answer that is not the pending question. This is neutral
    /// input: callers must not record an attempt, play an error sound or reset SRS.
    RepertoireAlternative {
        already_completed: bool,
    },
    Graph(LearningGraphError),
}

impl std::fmt::Display for LearningSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySessionId => f.write_str("learning session id must not be empty"),
            Self::EmptyClientAttemptId => f.write_str("client attempt id must not be empty"),
            Self::UnsupportedMode(mode) => {
                write!(f, "learning session operation does not support {mode:?}")
            }
            Self::SessionSideMismatch => {
                f.write_str("learning session side does not match its statistics scope")
            }
            Self::InvalidPhase { operation, phase } => {
                write!(
                    f,
                    "cannot {operation} while learning session is in {phase:?}"
                )
            }
            Self::IntroductionRequiresFinished => {
                f.write_str("new learning items can be introduced only after the due queue")
            }
            Self::InvalidSnapshot(message) => {
                write!(f, "invalid learning session snapshot: {message}")
            }
            Self::RepertoireNotFound(id) => write!(f, "repertoire not found: {id}"),
            Self::InvalidTargetPath {
                repertoire_id,
                path,
            } => write!(
                f,
                "learning target path is no longer legal in repertoire {repertoire_id}: {}",
                path.join(" ")
            ),
            Self::TargetPositionMismatch => {
                f.write_str("learning target path does not reach its saved position")
            }
            Self::TargetSideMismatch => {
                f.write_str("learning target position belongs to the other side")
            }
            Self::RepertoireAlternative { already_completed } => {
                if *already_completed {
                    f.write_str("this repertoire answer has already been reviewed in this pass")
                } else {
                    f.write_str("this repertoire answer is not the current scheduled question")
                }
            }
            Self::Graph(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LearningSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Graph(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LearningGraphError> for LearningSessionError {
    fn from(error: LearningGraphError) -> Self {
        Self::Graph(error)
    }
}

impl LearningSessionSnapshot {
    pub fn new(
        id: impl Into<String>,
        library: &Library,
        learning: &LearningData,
        stats_scope: StatsScope,
        settings: SrsSettings,
    ) -> Result<Self, LearningSessionError> {
        Self::new_with_mode(
            id,
            library,
            learning,
            stats_scope,
            settings,
            QueueMode::SpacedRepetition,
        )
    }

    pub fn new_with_mode(
        id: impl Into<String>,
        library: &Library,
        learning: &LearningData,
        stats_scope: StatsScope,
        settings: SrsSettings,
        mode: QueueMode,
    ) -> Result<Self, LearningSessionError> {
        let id = id.into();
        if id.is_empty() {
            return Err(LearningSessionError::EmptySessionId);
        }
        let queue = select_queue(library, learning, &stats_scope, mode)?;
        let side = training_mode(stats_scope.side);
        let session = LearningSession {
            id,
            mode,
            side,
            scope: TrainingScopeKey {
                prefix: stats_scope.branch_root.clone().unwrap_or_default(),
                end_prefix: None,
            },
            settings,
        };
        let mut snapshot = Self {
            session,
            context_policy: LearningContextPolicy::default(),
            stats_scope,
            queue,
            current_index: 0,
            phase: LearningSessionPhase::Finished,
            context_index: 0,
            completed_keys: BTreeSet::new(),
            cycle: 0,
            revision: 0,
            runtime: SessionRuntime::default(),
        };
        snapshot.reset_current_target_with_library(library)?;
        Ok(snapshot)
    }

    /// Explicitly replaces a finished queue with the currently unseen items. No item is
    /// introduced into SRS until an independent answer or preview is recorded.
    pub fn introduce_new(
        &mut self,
        library: &Library,
        learning: &LearningData,
    ) -> Result<usize, LearningSessionError> {
        self.validate_configuration()?;
        if self.session.mode != QueueMode::SpacedRepetition {
            return Err(LearningSessionError::UnsupportedMode(self.session.mode));
        }
        if self.phase != LearningSessionPhase::Finished {
            return Err(LearningSessionError::IntroductionRequiresFinished);
        }
        self.queue = select_targets(
            library,
            learning,
            &self.stats_scope,
            |item| item.is_none_or(|item| !item.introduced),
            self.stats_scope.learning_priority,
        )?;
        self.current_index = 0;
        self.completed_keys.clear();
        self.reset_current_target_with_library(library)?;
        Ok(self.queue.len())
    }

    pub fn start_next_pass(
        &mut self,
        library: &Library,
        learning: &LearningData,
    ) -> Result<usize, LearningSessionError> {
        self.validate_configuration()?;
        if self.phase != LearningSessionPhase::Finished {
            return Err(LearningSessionError::InvalidPhase {
                operation: "start the next pass",
                phase: self.phase,
            });
        }
        self.queue = select_queue(library, learning, &self.stats_scope, self.session.mode)?;
        self.current_index = 0;
        self.completed_keys.clear();
        if !self.queue.is_empty() {
            self.cycle = self.cycle.saturating_add(1);
        }
        self.reset_current_target_with_library(library)?;
        Ok(self.queue.len())
    }

    pub fn reset_review_order(&mut self) -> Result<(), LearningSessionError> {
        self.validate_configuration()?;
        if self.session.mode != QueueMode::ReviewInOrder {
            return Err(LearningSessionError::UnsupportedMode(self.session.mode));
        }
        self.current_index = 0;
        self.context_index = 0;
        self.completed_keys.clear();
        self.cycle = 0;
        self.reset_current_target();
        Ok(())
    }

    pub fn resume(self, library: &Library) -> Result<Self, LearningSessionError> {
        self.validate(library)?;
        Ok(self)
    }

    pub fn id(&self) -> &str {
        &self.session.id
    }

    pub fn current_target(&self) -> Option<&LearningSessionTarget> {
        self.queue.get(self.current_index)
    }

    pub fn current_expected_uci(&self) -> Option<&str> {
        let target = self.current_target()?;
        if self.is_context_challenge(target) {
            target.path.get(self.context_index).map(String::as_str)
        } else {
            Some(&target.key.expected_uci)
        }
    }

    pub fn remaining(&self) -> usize {
        self.queue
            .iter()
            .skip(self.current_index)
            .filter(|target| !self.completed_keys.contains(&target.key))
            .count()
    }

    /// Choices still due in this fixed pass; statistics stay separate for each UCI.
    pub fn pending_answers_for_position(&self, position_fen4: &str) -> Vec<String> {
        self.queue
            .iter()
            .skip(self.current_index)
            .filter(|target| {
                target.key.position_fen4 == position_fen4
                    && !self.completed_keys.contains(&target.key)
            })
            .map(|target| target.key.expected_uci.clone())
            .collect()
    }

    /// Only active answers in this session's scope qualify, never arbitrary legal
    /// moves or a disabled/out-of-scope PGN branch.
    pub fn is_active_repertoire_answer(
        &self,
        library: &Library,
        position_fen4: &str,
        uci: &str,
    ) -> Result<bool, LearningSessionError> {
        Ok(active_learning_memberships(library, &self.stats_scope)?
            .iter()
            .any(|item| item.key.position_fen4 == position_fen4 && item.key.expected_uci == uci))
    }

    pub fn presentation(
        &self,
        library: &Library,
    ) -> Result<LearningPresentation, LearningSessionError> {
        self.presentation_impl(library, None)
    }

    pub fn presentation_with_learning(
        &mut self,
        library: &Library,
        learning: &LearningData,
    ) -> Result<LearningPresentation, LearningSessionError> {
        self.sync_context_phase(library, Some(learning))?;
        self.presentation_impl(library, Some(learning))
    }

    fn presentation_impl(
        &self,
        library: &Library,
        learning: Option<&LearningData>,
    ) -> Result<LearningPresentation, LearningSessionError> {
        self.validate_configuration()?;
        let Some(target) = self.current_target() else {
            return Ok(LearningPresentation {
                phase: self.phase,
                target: None,
                target_path: Vec::new(),
                current_position: None,
                automatic_context: Vec::new(),
                revealed_move: None,
            });
        };
        let positions = self.replay_target(library, target)?;
        if self.context_index >= positions.len() {
            return Err(LearningSessionError::InvalidSnapshot(
                "context index is outside the target path",
            ));
        }
        let prefix_len = target.path.len().saturating_sub(1);
        let plan = learning.map(|learning| self.context_plan(target, &positions, learning));
        let automatic_context = (self.context_index..prefix_len)
            .take_while(|index| {
                if *index == self.context_index && self.is_context_challenge(target) {
                    false
                } else {
                    !self.context_needs_check(target, &positions, *index, plan.as_ref())
                }
            })
            .map(|index| {
                let factor = learning.map_or(1.0, |learning| {
                    let own_move =
                        LearningSide::from(positions[index].turn()) == self.stats_scope.side;
                    let key = LearningItemKey::new(
                        target.key.user_id.clone(),
                        fen4(&positions[index]),
                        target.path[index].clone(),
                    );
                    let item = own_move.then(|| learning.item(&key)).flatten();
                    speed_factor(&self.context_policy, item)
                });
                LearningContextStep {
                    uci: target.path[index].clone(),
                    from: learning_position(&positions[index]),
                    to: learning_position(&positions[index + 1]),
                    speed_factor: factor,
                }
            })
            .collect();
        let revealed_move = match self.phase {
            LearningSessionPhase::AwaitingReplayAfterError
            | LearningSessionPhase::PreviewedReplay => {
                self.current_expected_uci().map(str::to_owned)
            }
            _ => None,
        };
        Ok(LearningPresentation {
            phase: self.phase,
            target: Some(target.key.clone()),
            target_path: target.path.clone(),
            current_position: Some(learning_position(&positions[self.context_index])),
            automatic_context,
            revealed_move,
        })
    }

    pub fn advance_context(
        &mut self,
        library: &Library,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        self.advance_context_impl(library, client_attempt_id, happened_at, None)
    }

    pub fn advance_context_with_learning(
        &mut self,
        library: &Library,
        learning: &LearningData,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        self.sync_context_phase(library, Some(learning))?;
        self.advance_context_impl(library, client_attempt_id, happened_at, Some(learning))
    }

    fn advance_context_impl(
        &mut self,
        library: &Library,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
        learning: Option<&LearningData>,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        let client_attempt_id = checked_attempt_id(client_attempt_id.into())?;
        self.require_phase(
            "advance automatic context",
            LearningSessionPhase::AdvancingContext,
        )?;
        let target =
            self.current_target()
                .cloned()
                .ok_or(LearningSessionError::InvalidSnapshot(
                    "context has no current target",
                ))?;
        let positions = self.replay_target(library, &target)?;
        let prefix_len = target.path.len() - 1;
        if self.context_index >= prefix_len {
            return Err(LearningSessionError::InvalidSnapshot(
                "automatic context is already complete",
            ));
        }
        let index = self.context_index;
        let plan = learning.map(|learning| self.context_plan(&target, &positions, learning));
        if self.context_needs_check(&target, &positions, index, plan.as_ref()) {
            return Err(LearningSessionError::InvalidSnapshot(
                "automatic context requires an independent answer",
            ));
        }
        let uci = target.path[index].clone();
        let attempt = TrainingAttempt {
            user_id: target.key.user_id,
            position_fen4: fen4(&positions[index]),
            client_attempt_id,
            session_id: self.session.id.clone(),
            event_type: TrainingEventType::AutoContext,
            expected_uci: uci.clone(),
            actual_uci: Some(uci),
            response_time_ms: None,
            happened_at,
        };
        self.context_index += 1;
        self.sync_context_phase(library, learning)?;
        Ok(attempt)
    }

    pub fn preview(
        &mut self,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
    ) -> Result<Option<TrainingAttempt>, LearningSessionError> {
        let client_attempt_id = checked_attempt_id(client_attempt_id.into())?;
        self.validate_configuration()?;
        match self.phase {
            LearningSessionPhase::AwaitingReplayAfterError
            | LearningSessionPhase::PreviewedReplay => return Ok(None),
            LearningSessionPhase::AwaitingIndependent => {}
            phase => {
                return Err(LearningSessionError::InvalidPhase {
                    operation: "preview the expected move",
                    phase,
                });
            }
        }
        let target =
            self.current_target()
                .cloned()
                .ok_or(LearningSessionError::InvalidSnapshot(
                    "preview has no current target",
                ))?;
        self.phase = LearningSessionPhase::PreviewedReplay;
        Ok(Some(self.attempt(
            &target.key,
            TrainingEventType::Previewed,
            Some(target.key.expected_uci.clone()),
            None,
            client_attempt_id,
            happened_at,
        )))
    }

    pub fn play_uci(
        &mut self,
        library: &Library,
        uci: &str,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
        response_time_ms: Option<u64>,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        self.play_uci_impl(
            library,
            uci,
            client_attempt_id,
            happened_at,
            response_time_ms,
            None,
            None,
        )
    }

    pub fn play_uci_with_learning(
        &mut self,
        library: &Library,
        learning: &LearningData,
        uci: &str,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
        response_time_ms: Option<u64>,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        self.sync_context_phase(library, Some(learning))?;
        self.play_uci_impl(
            library,
            uci,
            client_attempt_id,
            happened_at,
            response_time_ms,
            Some(learning),
            None,
        )
    }

    /// The UI prepares this scope's answers with the immutable library in its
    /// startup worker, so a wrong move never rebuilds all repertoire graphs.
    #[allow(clippy::too_many_arguments)]
    pub fn play_uci_with_learning_answers(
        &mut self,
        library: &Library,
        learning: &LearningData,
        answers: &BTreeSet<LearningItemKey>,
        uci: &str,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
        response_time_ms: Option<u64>,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        self.sync_context_phase(library, Some(learning))?;
        self.play_uci_impl(
            library,
            uci,
            client_attempt_id,
            happened_at,
            response_time_ms,
            Some(learning),
            Some(answers),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn play_uci_impl(
        &mut self,
        library: &Library,
        uci: &str,
        client_attempt_id: impl Into<String>,
        happened_at: i64,
        response_time_ms: Option<u64>,
        learning: Option<&LearningData>,
        answers: Option<&BTreeSet<LearningItemKey>>,
    ) -> Result<TrainingAttempt, LearningSessionError> {
        let client_attempt_id = checked_attempt_id(client_attempt_id.into())?;
        self.validate_configuration()?;
        if !matches!(
            self.phase,
            LearningSessionPhase::AwaitingIndependent
                | LearningSessionPhase::AwaitingContextIndependent
                | LearningSessionPhase::AwaitingReplayAfterError
                | LearningSessionPhase::PreviewedReplay
        ) {
            return Err(LearningSessionError::InvalidPhase {
                operation: "play a move",
                phase: self.phase,
            });
        }
        let target =
            self.current_target()
                .cloned()
                .ok_or(LearningSessionError::InvalidSnapshot(
                    "move input has no current target",
                ))?;
        let positions = self.replay_target(library, &target)?;
        let context = self.is_context_challenge(&target);
        let challenge_index = if context {
            self.context_index
        } else {
            target.path.len() - 1
        };
        let position = &positions[challenge_index];
        let challenge_key = if context {
            LearningItemKey::new(
                target.key.user_id.clone(),
                fen4(position),
                target.path[challenge_index].clone(),
            )
        } else {
            target.key.clone()
        };
        let legal = uci
            .parse::<UciMove>()
            .ok()
            .and_then(|parsed| parsed.to_move(position).ok());
        let Some(mv) = legal else {
            return Ok(self.attempt(
                &challenge_key,
                TrainingEventType::IllegalMove,
                Some(uci.to_owned()),
                None,
                client_attempt_id,
                happened_at,
            ));
        };
        let actual_uci = mv.to_uci(CastlingMode::Standard).to_string();
        let correct = actual_uci == challenge_key.expected_uci;
        if !correct {
            let actual_key = LearningItemKey::new(
                challenge_key.user_id.clone(),
                challenge_key.position_fen4.clone(),
                actual_uci.clone(),
            );
            if matches!(
                self.phase,
                LearningSessionPhase::AwaitingIndependent
                    | LearningSessionPhase::AwaitingContextIndependent
            ) && let Some(index) = self
                .queue
                .iter()
                .enumerate()
                .skip(self.current_index)
                .find_map(|(index, candidate)| {
                    (candidate.key == actual_key && !self.completed_keys.contains(&actual_key))
                        .then_some(index)
                })
            {
                // Consume exactly the answer played. The original target remains
                // pending, even when this question was part of its context. Never
                // advance the old path after taking a different branch.
                let attempt = self.attempt(
                    &actual_key,
                    TrainingEventType::CorrectIndependent,
                    Some(actual_uci),
                    response_time_ms,
                    client_attempt_id,
                    happened_at,
                );
                self.queue.swap(self.current_index, index);
                self.complete_current(library)?;
                return Ok(attempt);
            }
            let known = match answers {
                Some(answers) => answers.contains(&actual_key),
                None => self.is_active_repertoire_answer(
                    library,
                    &actual_key.position_fen4,
                    &actual_uci,
                )?,
            };
            if known {
                return Err(LearningSessionError::RepertoireAlternative {
                    already_completed: self.completed_keys.contains(&actual_key),
                });
            }
        }
        let event_type = match self.phase {
            LearningSessionPhase::AwaitingIndependent
            | LearningSessionPhase::AwaitingContextIndependent => {
                if correct {
                    TrainingEventType::CorrectIndependent
                } else {
                    TrainingEventType::IncorrectIndependent
                }
            }
            LearningSessionPhase::AwaitingReplayAfterError => TrainingEventType::ReplayAfterError,
            LearningSessionPhase::PreviewedReplay => TrainingEventType::AutoContext,
            phase => {
                return Err(LearningSessionError::InvalidPhase {
                    operation: "play a move",
                    phase,
                });
            }
        };
        let independent_time = if event_type.is_independent() {
            response_time_ms
        } else {
            None
        };
        let attempt = self.attempt(
            &challenge_key,
            event_type,
            Some(actual_uci),
            independent_time,
            client_attempt_id,
            happened_at,
        );
        match self.phase {
            LearningSessionPhase::AwaitingIndependent if correct => {
                self.complete_current(library)?;
            }
            LearningSessionPhase::AwaitingContextIndependent if correct => {
                self.complete_context(challenge_key, library, learning)?;
            }
            LearningSessionPhase::AwaitingIndependent
            | LearningSessionPhase::AwaitingContextIndependent => {
                self.phase = LearningSessionPhase::AwaitingReplayAfterError;
            }
            LearningSessionPhase::AwaitingReplayAfterError
            | LearningSessionPhase::PreviewedReplay
                if correct =>
            {
                if context {
                    self.complete_context(challenge_key, library, learning)?;
                } else {
                    self.complete_current(library)?;
                }
            }
            _ => {}
        }
        Ok(attempt)
    }

    pub fn validate(&self, library: &Library) -> Result<(), LearningSessionError> {
        self.validate_configuration()?;
        if self.current_index > self.queue.len() {
            return Err(LearningSessionError::InvalidSnapshot(
                "current index is outside the fixed queue",
            ));
        }
        let mut keys = BTreeSet::new();
        if self.queue.iter().any(|target| !keys.insert(&target.key)) {
            return Err(LearningSessionError::InvalidSnapshot(
                "fixed queue contains a duplicate learning item",
            ));
        }
        let Some(target) = self.current_target() else {
            if self.phase != LearningSessionPhase::Finished {
                return Err(LearningSessionError::InvalidSnapshot(
                    "a session without a current target must be finished",
                ));
            }
            return Ok(());
        };
        if self.phase == LearningSessionPhase::Finished {
            return Err(LearningSessionError::InvalidSnapshot(
                "a session with a current target cannot be finished",
            ));
        }
        if self.completed_keys.contains(&target.key) {
            return Err(LearningSessionError::InvalidSnapshot(
                "the current learning item is already completed in this pass",
            ));
        }
        let positions = self.replay_target(library, target)?;
        let prefix_len = target.path.len() - 1;
        let own_context = self.context_index < prefix_len
            && LearningSide::from(positions[self.context_index].turn()) == self.stats_scope.side;
        let legacy_context_needs_check = self.context_index < prefix_len
            && self.context_needs_check(target, &positions, self.context_index, None);
        match self.phase {
            LearningSessionPhase::AdvancingContext
                if self.context_index < prefix_len && !legacy_context_needs_check => {}
            LearningSessionPhase::AwaitingContextIndependent if own_context => {}
            LearningSessionPhase::AwaitingIndependent if self.context_index == prefix_len => {}
            LearningSessionPhase::AwaitingReplayAfterError
                if self.context_index == prefix_len || own_context => {}
            LearningSessionPhase::PreviewedReplay if self.context_index == prefix_len => {}
            _ => {
                return Err(LearningSessionError::InvalidSnapshot(
                    "phase and automatic context index disagree",
                ));
            }
        }
        if positions.len() != target.path.len() + 1 {
            return Err(LearningSessionError::InvalidSnapshot(
                "target path replay is incomplete",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_configuration(&self) -> Result<(), LearningSessionError> {
        if self.session.id.is_empty() {
            return Err(LearningSessionError::EmptySessionId);
        }
        if !matches!(
            self.session.mode,
            QueueMode::SpacedRepetition | QueueMode::ReviewInOrder | QueueMode::LeastRecent
        ) {
            return Err(LearningSessionError::UnsupportedMode(self.session.mode));
        }
        if self.session.side != training_mode(self.stats_scope.side) {
            return Err(LearningSessionError::SessionSideMismatch);
        }
        Ok(())
    }

    fn require_phase(
        &self,
        operation: &'static str,
        expected: LearningSessionPhase,
    ) -> Result<(), LearningSessionError> {
        self.validate_configuration()?;
        if self.phase == expected {
            Ok(())
        } else {
            Err(LearningSessionError::InvalidPhase {
                operation,
                phase: self.phase,
            })
        }
    }

    fn attempt(
        &self,
        key: &LearningItemKey,
        event_type: TrainingEventType,
        actual_uci: Option<String>,
        response_time_ms: Option<u64>,
        client_attempt_id: String,
        happened_at: i64,
    ) -> TrainingAttempt {
        TrainingAttempt {
            user_id: key.user_id.clone(),
            position_fen4: key.position_fen4.clone(),
            client_attempt_id,
            session_id: self.session.id.clone(),
            event_type,
            expected_uci: key.expected_uci.clone(),
            actual_uci,
            response_time_ms,
            happened_at,
        }
    }

    fn complete_current(&mut self, library: &Library) -> Result<(), LearningSessionError> {
        if let Some(target) = self.current_target() {
            self.completed_keys.insert(target.key.clone());
        }
        self.current_index += 1;
        while self
            .current_target()
            .is_some_and(|target| self.completed_keys.contains(&target.key))
        {
            self.current_index += 1;
        }
        if self.session.mode == QueueMode::ReviewInOrder
            && self.current_index >= self.queue.len()
            && !self.queue.is_empty()
        {
            self.current_index = 0;
            self.completed_keys.clear();
            self.cycle = self.cycle.saturating_add(1);
        }
        self.reset_current_target_with_library(library)
    }

    fn complete_context(
        &mut self,
        key: LearningItemKey,
        library: &Library,
        learning: Option<&LearningData>,
    ) -> Result<(), LearningSessionError> {
        self.completed_keys.insert(key);
        self.context_index += 1;
        self.phase = LearningSessionPhase::AdvancingContext;
        self.sync_context_phase(library, learning)
    }

    fn reset_current_target_with_library(
        &mut self,
        library: &Library,
    ) -> Result<(), LearningSessionError> {
        self.reset_current_target();
        self.sync_context_phase(library, None)
    }

    fn sync_context_phase(
        &mut self,
        library: &Library,
        learning: Option<&LearningData>,
    ) -> Result<(), LearningSessionError> {
        self.validate_configuration()?;
        let Some(target) = self.current_target().cloned() else {
            self.phase = LearningSessionPhase::Finished;
            return Ok(());
        };
        let positions = self.replay_target(library, &target)?;
        let prefix_len = target.path.len() - 1;
        if self.context_index > prefix_len {
            return Err(LearningSessionError::InvalidSnapshot(
                "context index is outside the target path",
            ));
        }
        if self.context_index == prefix_len {
            if !matches!(
                self.phase,
                LearningSessionPhase::AwaitingReplayAfterError
                    | LearningSessionPhase::PreviewedReplay
            ) {
                self.phase = LearningSessionPhase::AwaitingIndependent;
            }
            return Ok(());
        }
        if self.is_context_challenge(&target) {
            if LearningSide::from(positions[self.context_index].turn()) == self.stats_scope.side {
                return Ok(());
            }
            return Err(LearningSessionError::InvalidSnapshot(
                "opponent context cannot require an independent answer",
            ));
        }
        let plan = learning.map(|learning| self.context_plan(&target, &positions, learning));
        if let Some(plan) = &plan
            && self.phase == LearningSessionPhase::AdvancingContext
            && self.context_index < plan.skip_prefix
        {
            self.context_index = plan.skip_prefix;
            if self.context_index == prefix_len {
                self.phase = LearningSessionPhase::AwaitingIndependent;
                return Ok(());
            }
        }
        self.phase =
            if self.context_needs_check(&target, &positions, self.context_index, plan.as_ref()) {
                LearningSessionPhase::AwaitingContextIndependent
            } else {
                LearningSessionPhase::AdvancingContext
            };
        Ok(())
    }

    fn context_plan(
        &self,
        target: &LearningSessionTarget,
        positions: &[Chess],
        learning: &LearningData,
    ) -> LearningContextPlan {
        #[cfg(test)]
        {
            self.runtime
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .plans += 1;
        }
        let queue_keys = (self.session.mode == QueueMode::LeastRecent).then(|| {
            self.queue
                .iter()
                .map(|target| &target.key)
                .collect::<BTreeSet<_>>()
        });
        let moves = (0..target.path.len().saturating_sub(1))
            .map(|index| {
                let key = LearningItemKey::new(
                    target.key.user_id.clone(),
                    fen4(&positions[index]),
                    target.path[index].clone(),
                );
                LearningContextMove {
                    own_move: LearningSide::from(positions[index].turn()) == self.stats_scope.side,
                    least_recent_target: queue_keys
                        .as_ref()
                        .is_some_and(|keys| keys.contains(&key)),
                    key,
                }
            })
            .collect::<Vec<_>>();
        plan_context(
            &moves,
            learning,
            &self.context_policy,
            &self.completed_keys,
            self.session.mode == QueueMode::LeastRecent,
            self.stats_scope.as_of,
        )
    }

    fn context_needs_check(
        &self,
        target: &LearningSessionTarget,
        positions: &[Chess],
        index: usize,
        plan: Option<&LearningContextPlan>,
    ) -> bool {
        if index >= target.path.len().saturating_sub(1)
            || LearningSide::from(positions[index].turn()) != self.stats_scope.side
        {
            return false;
        }
        if let Some(plan) = plan {
            return plan.actions[index] == LearningContextAction::Independent;
        }
        if self.session.mode != QueueMode::LeastRecent {
            return false;
        }
        let key = LearningItemKey::new(
            target.key.user_id.clone(),
            fen4(&positions[index]),
            target.path[index].clone(),
        );
        !self.completed_keys.contains(&key) && self.queue.iter().any(|target| target.key == key)
    }

    fn is_context_challenge(&self, target: &LearningSessionTarget) -> bool {
        self.context_index < target.path.len().saturating_sub(1)
            && matches!(
                self.phase,
                LearningSessionPhase::AwaitingContextIndependent
                    | LearningSessionPhase::AwaitingReplayAfterError
                    | LearningSessionPhase::PreviewedReplay
            )
    }

    fn reset_current_target(&mut self) {
        self.context_index = 0;
        self.phase = match self.current_target() {
            None => LearningSessionPhase::Finished,
            Some(target) if target.path.len() > 1 => LearningSessionPhase::AdvancingContext,
            Some(_) => LearningSessionPhase::AwaitingIndependent,
        };
    }
}

fn checked_attempt_id(id: String) -> Result<String, LearningSessionError> {
    if id.is_empty() {
        Err(LearningSessionError::EmptyClientAttemptId)
    } else {
        Ok(id)
    }
}

fn training_mode(side: LearningSide) -> TrainingMode {
    match side {
        LearningSide::White => TrainingMode::White,
        LearningSide::Black => TrainingMode::Black,
    }
}

fn select_queue(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    mode: QueueMode,
) -> Result<Vec<LearningSessionTarget>, LearningSessionError> {
    match mode {
        QueueMode::SpacedRepetition => select_targets(
            library,
            learning,
            scope,
            |item| item.is_some_and(|item| item.is_due(scope.as_of)),
            LearningPriority::BreadthOverDepth,
        ),
        QueueMode::ReviewInOrder => {
            select_targets(library, learning, scope, |_| true, scope.learning_priority)
        }
        QueueMode::LeastRecent => {
            let mut memberships =
                select_memberships(library, learning, scope, |_| true, scope.learning_priority)?;
            sort_least_recent(&mut memberships, learning, scope.learning_priority);
            Ok(memberships
                .into_iter()
                .map(LearningSessionTarget::from)
                .collect())
        }
        QueueMode::Practice => Err(LearningSessionError::UnsupportedMode(mode)),
    }
}

fn select_targets(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    include: impl Fn(Option<&super::learning::LearningItem>) -> bool,
    priority: LearningPriority,
) -> Result<Vec<LearningSessionTarget>, LearningSessionError> {
    Ok(
        select_memberships(library, learning, scope, include, priority)?
            .into_iter()
            .map(LearningSessionTarget::from)
            .collect(),
    )
}

fn select_memberships(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    include: impl Fn(Option<&super::learning::LearningItem>) -> bool,
    priority: LearningPriority,
) -> Result<Vec<LearningItemMembership>, LearningSessionError> {
    let memberships = active_learning_memberships(library, scope)?;
    let mut by_key = BTreeMap::new();
    for membership in memberships {
        by_key
            .entry(membership.key.clone())
            .and_modify(|existing: &mut LearningItemMembership| {
                if membership_order(&membership) < membership_order(existing) {
                    *existing = membership.clone();
                }
            })
            .or_insert(membership);
    }
    let mut memberships: Vec<_> = by_key
        .into_values()
        .filter(|membership| include(learning.item(&membership.key)))
        .collect();
    sort_by_priority(&mut memberships, priority);
    Ok(memberships)
}

fn membership_order(
    membership: &LearningItemMembership,
) -> (&RepertoireId, u32, &[String], &LearningItemKey) {
    (
        &membership.repertoire_id,
        membership.min_depth,
        &membership.path,
        &membership.key,
    )
}

impl LearningSessionSnapshot {
    fn replay_target(
        &self,
        library: &Library,
        target: &LearningSessionTarget,
    ) -> Result<Arc<Vec<Chess>>, LearningSessionError> {
        let side = self.stats_scope.side;
        if target.path.is_empty() || target.path.last() != Some(&target.key.expected_uci) {
            return Err(LearningSessionError::InvalidTargetPath {
                repertoire_id: target.repertoire_id.clone(),
                path: target.path.clone(),
            });
        }
        let repertoire = library.repertoire(&target.repertoire_id).ok_or_else(|| {
            LearningSessionError::RepertoireNotFound(target.repertoire_id.clone())
        })?;
        let mut runtime = self
            .runtime
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if runtime.source.as_ref().is_none_or(|source| {
            source.repertoire_id != target.repertoire_id || source.pgn != repertoire.pgn
        }) {
            runtime.source = Some(ReplaySource {
                repertoire_id: target.repertoire_id.clone(),
                pgn: repertoire.pgn.clone(),
                start: repertoire.game().start().position().clone(),
                target: None,
            });
            #[cfg(test)]
            {
                runtime.parses += 1;
            }
        }
        // The source was installed above. Avoid an assumption on user data at this boundary.
        let Some(source) = runtime.source.as_ref() else {
            return Err(LearningSessionError::InvalidSnapshot(
                "replay source unavailable",
            ));
        };
        if let Some(cached) = &source.target
            && cached.target == *target
            && cached.side == side
        {
            return Ok(Arc::clone(&cached.positions));
        }
        let mut position = source.start.clone();
        let mut positions = vec![position.clone()];
        for uci in &target.path {
            let mv = uci
                .parse::<UciMove>()
                .ok()
                .and_then(|parsed| parsed.to_move(&position).ok())
                .ok_or_else(|| LearningSessionError::InvalidTargetPath {
                    repertoire_id: target.repertoire_id.clone(),
                    path: target.path.clone(),
                })?;
            position.play_unchecked(mv);
            positions.push(position.clone());
        }
        let parent = &positions[target.path.len() - 1];
        if fen4(parent) != target.key.position_fen4 {
            return Err(LearningSessionError::TargetPositionMismatch);
        }
        if LearningSide::from(parent.turn()) != side {
            return Err(LearningSessionError::TargetSideMismatch);
        }
        let positions = Arc::new(positions);
        if let Some(source) = runtime.source.as_mut() {
            source.target = Some(ReplayedTarget {
                target: target.clone(),
                side,
                positions: Arc::clone(&positions),
            });
        }
        #[cfg(test)]
        {
            runtime.replays += 1;
        }
        Ok(positions)
    }

    #[cfg(test)]
    pub(super) fn runtime_work_counts(&self) -> (usize, usize, usize) {
        let runtime = self
            .runtime
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (runtime.parses, runtime.replays, runtime.plans)
    }
}

fn learning_position(position: &Chess) -> LearningPosition {
    LearningPosition {
        fen: Fen::from_position(position, EnPassantMode::Legal).to_string(),
        fen4: fen4(position),
    }
}
