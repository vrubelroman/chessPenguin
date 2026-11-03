//! Versioned opening repertoires, tree training and their persistent stores.
//!
//! This module is independent from the legacy line trainer in the crate root.

mod model;
mod scope;
mod session;
mod stats;
mod store;
mod tree;

pub use model::{
    DEFAULT_MAX_PLIES, LIBRARY_VERSION, LegacyMigrationReport, Library, MergeReport, Repertoire,
    RepertoireError, RepertoireId,
};
pub use scope::{TrainingScope, TrainingScopeKey};
pub use session::{
    FirstMoveChooser, MoveChooser, PlayedMove, PreparedNextPath, SessionAttempt, SessionError,
    TrainingSession, TrainingVerdict,
};
pub use stats::{
    AttemptRecord, INITIAL_INTERVAL_SECONDS, INTRODUCTION_BATCH_SIZE, MAX_INTERVAL_SECONDS,
    MISTAKE_INTERVAL_SECONDS, MoveKey, MoveStats, PathKey, PathStats, QueueMode, RunRecord,
    RunStats, STATS_VERSION, SrsPolicy, StatsData, TrainingMode,
};
pub use store::{LibraryStore, StatsStore, StoreError};
pub use tree::{Candidate, RepertoireTree, TrainingPath, TrainingTarget, fen4};

#[cfg(test)]
mod advanced_tests;
#[cfg(test)]
mod tests;
