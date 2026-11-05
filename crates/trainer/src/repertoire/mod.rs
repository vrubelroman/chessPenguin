//! Versioned opening repertoires, tree training and their persistent stores.
//!
//! This module is independent from the legacy line trainer in the crate root.

mod aggregate;
mod branch_analytics;
mod learning;
mod learning_analytics;
mod learning_graph;
mod learning_modes;
mod learning_session;
mod line_analytics;
mod model;
mod scope;
mod session;
mod srs;
mod stats;
mod store;
mod tree;

pub use aggregate::{
    LearningAggregate, RepertoireStatus, aggregate, global_total_invariant_violation, is_uncertain,
    log_global_total_invariant,
};
pub use branch_analytics::{BranchLearningAnalytics, branch_learning_analytics};
pub use learning::{
    AttemptApplication, IndependentOutcome, IndependentResult, LOCAL_PROFILE_ID, LearningData,
    LearningError, LearningItem, LearningItemKey, RECENT_INDEPENDENT_OUTCOMES_LIMIT,
    TrainingAttempt, TrainingEventType, WrongMoveStats, is_resistant,
};
pub use learning_analytics::{
    AttemptsHistogram, CorrectStreakHistogram, LearningSnapshot, LocalDatePolicy,
    MoveLearningAnalytics, MoveLearningStatus, SCHEDULED_MONTH_SECONDS, SCHEDULED_WEEK_SECONDS,
    SCHEDULED_YEAR_SECONDS, ScheduledBuckets, SequenceHistograms, WrongStreakHistogram,
    attempts_histogram, correct_streak_histogram, move_learning_analytics, scheduled_buckets,
    scoped_move_learning_analytics, sequence_histograms, wrong_streak_histogram,
};
pub use learning_graph::{
    LearningGraphError, LearningItemMembership, ResetImpactProjection, ResetRepertoireImpact,
    ScopedLearningAggregate, StatsScope, active_learning_keys, active_learning_memberships,
    aggregate_scope, reset_impact,
};
pub use learning_modes::LearningPriority;
pub use learning_session::{
    LearningContextPolicy, LearningContextStep, LearningPosition, LearningPresentation,
    LearningSessionError, LearningSessionPhase, LearningSessionSnapshot, LearningSessionTarget,
};
pub use line_analytics::{LineLearningAnalytics, line_learning_analytics};
pub use model::{
    DEFAULT_MAX_PLIES, LIBRARY_VERSION, LearningSide, LegacyMigrationReport, Library, MergeReport,
    Repertoire, RepertoireError, RepertoireId,
};
pub use scope::{TrainingScope, TrainingScopeKey};
pub use session::{
    FirstMoveChooser, MoveChooser, PlayedMove, PreparedNextPath, SessionAttempt, SessionError,
    TrainingSession, TrainingVerdict,
};
pub use srs::{
    DEFAULT_GROWTH_FACTOR, FIRST_CORRECT_DELAY_SECONDS, MIN_PRACTICAL_INTERVAL_SECONDS,
    MISTAKE_DELAY_SECONDS, PREVIEW_DELAY_SECONDS, SRS_MAX_INTERVAL_SECONDS, SrsSettings,
    UNCERTAIN_INTERVAL_DIVISOR, UNCERTAIN_MIN_GRACE_SECONDS, apply_attempt,
    apply_independent_correct, apply_independent_wrong, apply_preview,
};
pub use stats::{
    AttemptRecord, INITIAL_INTERVAL_SECONDS, INTRODUCTION_BATCH_SIZE, MAX_INTERVAL_SECONDS,
    MISTAKE_INTERVAL_SECONDS, MoveKey, MoveStats, PathKey, PathStats, QueueMode, RunRecord,
    RunStats, STATS_VERSION, SrsPolicy, StatsData, TrainingMode,
};
pub use store::{
    LEARNING_STORE_VERSION, LearningSession, LearningStore, LearningStoreData, LibraryStore,
    MigrationReport, PersistedAttempt, PersistedLearningReset, PersistedLearningSnapshot,
    PersistedSessionAttempt, StatsStore, StoreError,
};
pub use tree::{Candidate, RepertoireTree, TrainingPath, TrainingTarget, fen4};

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod advanced_tests;
#[cfg(test)]
mod analytics_tests;
#[cfg(test)]
mod branch_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod line_tests;
#[cfg(test)]
mod mode_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;
