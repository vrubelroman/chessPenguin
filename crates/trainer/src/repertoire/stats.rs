use chess_core::shakmaty::{Color, Position};
use serde::{Deserialize, Serialize};

use super::model::RepertoireId;
use super::scope::{TrainingScope, TrainingScopeKey};
use super::tree::{RepertoireTree, TrainingPath, TrainingTarget};

pub const STATS_VERSION: u32 = 2;
pub const INITIAL_INTERVAL_SECONDS: i64 = 24 * 60 * 60;
pub const MISTAKE_INTERVAL_SECONDS: i64 = 10 * 60;
pub const MAX_INTERVAL_SECONDS: i64 = 365 * 24 * 60 * 60;
const RECENT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TrainingMode {
    White,
    Black,
    Both,
}

impl TrainingMode {
    pub fn user_color(self) -> Option<Color> {
        match self {
            Self::White => Some(Color::White),
            Self::Black => Some(Color::Black),
            Self::Both => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueMode {
    Practice,
    SpacedRepetition,
    ReviewInOrder,
    LeastRecent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SrsPolicy {
    Due,
    Introduce,
    EarlyReview,
}

pub const INTRODUCTION_BATCH_SIZE: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MoveKey {
    pub repertoire_id: RepertoireId,
    pub mode: TrainingMode,
    pub position_fen4: String,
    pub expected_uci: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub at: i64,
    pub correct: bool,
    pub hinted: bool,
    /// The actual legal input, including a wrong move outside the repertoire.
    pub actual_uci: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoveStats {
    pub key: MoveKey,
    pub attempts: u32,
    pub correct: u32,
    pub wrong: u32,
    pub hints: u32,
    /// Positive: clean correct answers in a row. Negative: wrong answers in a row.
    pub streak: i32,
    pub recent: Vec<AttemptRecord>,
    pub last_attempt: Option<i64>,
    pub last_seen: Option<i64>,
    pub due: Option<i64>,
    /// Scheduled interval, independent of observations during non-scheduling reviews.
    pub interval_seconds: Option<i64>,
}

impl Default for MoveStats {
    fn default() -> Self {
        Self {
            key: MoveKey {
                repertoire_id: RepertoireId::legacy(""),
                mode: TrainingMode::Both,
                position_fen4: String::new(),
                expected_uci: String::new(),
            },
            attempts: 0,
            correct: 0,
            wrong: 0,
            hints: 0,
            streak: 0,
            recent: Vec::new(),
            last_attempt: None,
            last_seen: None,
            due: None,
            interval_seconds: None,
        }
    }
}

impl MoveStats {
    pub fn new(key: MoveKey) -> Self {
        Self {
            key,
            ..Self::default()
        }
    }

    pub fn unseen(&self) -> bool {
        self.attempts == 0
    }

    pub fn is_due(&self, now: i64) -> bool {
        self.due.is_some_and(|due| due <= now)
    }

    fn push_recent(&mut self, attempt: AttemptRecord) {
        self.recent.push(attempt);
        if self.recent.len() > RECENT {
            self.recent.remove(0);
        }
    }

    fn wrong(&mut self, now: i64, actual_uci: &str, schedule: bool) {
        self.attempts = self.attempts.saturating_add(1);
        self.wrong = self.wrong.saturating_add(1);
        self.streak = if self.streak < 0 {
            self.streak.saturating_sub(1)
        } else {
            -1
        };
        self.last_attempt = Some(now);
        self.last_seen = Some(now);
        if schedule {
            self.due = Some(now.saturating_add(MISTAKE_INTERVAL_SECONDS));
            self.interval_seconds = Some(MISTAKE_INTERVAL_SECONDS);
        }
        self.push_recent(AttemptRecord {
            at: now,
            correct: false,
            hinted: false,
            actual_uci: actual_uci.to_owned(),
        });
    }

    fn correct(&mut self, now: i64, actual_uci: &str, hinted: bool, schedule: bool) {
        let previous_interval =
            self.interval_seconds
                .unwrap_or_else(|| match (self.due, self.last_seen) {
                    (Some(due), Some(last)) if due > last => due.saturating_sub(last),
                    _ => INITIAL_INTERVAL_SECONDS,
                });
        let clean_streak = self.streak;
        let retry = hinted || clean_streak < 0;
        self.attempts = self.attempts.saturating_add(1);
        self.correct = self.correct.saturating_add(1);
        self.streak = if retry {
            0
        } else {
            clean_streak.saturating_add(1).max(1)
        };
        if schedule {
            let interval = if retry {
                MISTAKE_INTERVAL_SECONDS
            } else if clean_streak > 0 {
                previous_interval
                    .saturating_mul(2)
                    .clamp(INITIAL_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS)
            } else {
                INITIAL_INTERVAL_SECONDS
            };
            self.due = Some(now.saturating_add(interval));
            self.interval_seconds = Some(interval);
        }
        self.last_attempt = Some(now);
        self.last_seen = Some(now);
        self.push_recent(AttemptRecord {
            at: now,
            correct: true,
            hinted,
            actual_uci: actual_uci.to_owned(),
        });
    }

    fn hint(&mut self, now: i64) {
        self.streak = 0;
        self.hints = self.hints.saturating_add(1);
        self.last_seen = Some(now);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub at: i64,
    pub mistakes: u32,
    pub hints: u32,
    pub hinted: bool,
    pub path: Vec<String>,
    pub user_attempts: u32,
    pub automatic_moves: u32,
    #[serde(default)]
    pub scope: TrainingScopeKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunStats {
    pub repertoire_id: RepertoireId,
    pub mode: TrainingMode,
    pub completions: u32,
    pub last: Option<i64>,
    pub recent: Vec<RunRecord>,
}

impl Default for RunStats {
    fn default() -> Self {
        Self {
            repertoire_id: RepertoireId::legacy(""),
            mode: TrainingMode::Both,
            completions: 0,
            last: None,
            recent: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PathKey {
    pub repertoire_id: RepertoireId,
    pub scope: TrainingScopeKey,
    pub path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathStats {
    pub key: PathKey,
    pub completions: u32,
    pub mistakes: u32,
    pub hints: u32,
    pub last_seen: Option<i64>,
    pub last_completion: Option<i64>,
    pub recent: Vec<RunRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReviewCursor {
    repertoire_id: RepertoireId,
    mode: TrainingMode,
    #[serde(default)]
    scope: TrainingScopeKey,
    next: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PathReviewCursor {
    repertoire_id: RepertoireId,
    scope: TrainingScopeKey,
    next: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RandomPracticeCursor {
    repertoire_id: RepertoireId,
    mode: TrainingMode,
    scope: TrainingScopeKey,
    seed: u64,
    position: usize,
    count: usize,
    #[serde(default)]
    fingerprint: Option<u64>,
}

pub(crate) struct RandomPracticePath {
    pub path: TrainingPath,
    pub start: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsData {
    pub version: u32,
    pub revision: u64,
    /// Original legacy values, retained verbatim. They are not per-move attempts.
    pub legacy_lines: serde_json::Map<String, serde_json::Value>,
    pub move_stats: Vec<MoveStats>,
    pub runs: Vec<RunStats>,
    pub path_stats: Vec<PathStats>,
    review_cursors: Vec<ReviewCursor>,
    path_review_cursors: Vec<PathReviewCursor>,
    practice_path_cursors: Vec<ReviewCursor>,
    random_practice_cursors: Vec<RandomPracticeCursor>,
}

impl Default for StatsData {
    fn default() -> Self {
        Self {
            version: STATS_VERSION,
            revision: 0,
            legacy_lines: serde_json::Map::new(),
            move_stats: Vec::new(),
            runs: Vec::new(),
            path_stats: Vec::new(),
            review_cursors: Vec::new(),
            path_review_cursors: Vec::new(),
            practice_path_cursors: Vec::new(),
            random_practice_cursors: Vec::new(),
        }
    }
}

impl StatsData {
    /// Removes modern statistics and queue positions for one repertoire. Legacy line
    /// aggregates have independent identities and remain untouched.
    pub fn remove_repertoire(&mut self, repertoire_id: &RepertoireId) {
        self.move_stats
            .retain(|entry| &entry.key.repertoire_id != repertoire_id);
        self.runs
            .retain(|entry| &entry.repertoire_id != repertoire_id);
        self.path_stats
            .retain(|entry| &entry.key.repertoire_id != repertoire_id);
        self.review_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id);
        self.path_review_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id);
        self.practice_path_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id);
        self.random_practice_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id);
    }

    pub fn remove_repertoire_mode(&mut self, repertoire_id: &RepertoireId, mode: TrainingMode) {
        self.move_stats
            .retain(|entry| &entry.key.repertoire_id != repertoire_id || entry.key.mode != mode);
        self.runs
            .retain(|entry| &entry.repertoire_id != repertoire_id || entry.mode != mode);
        self.review_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id || entry.mode != mode);
        self.practice_path_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id || entry.mode != mode);
        self.random_practice_cursors
            .retain(|entry| &entry.repertoire_id != repertoire_id || entry.mode != mode);
        if mode == TrainingMode::Both {
            self.path_stats
                .retain(|entry| &entry.key.repertoire_id != repertoire_id);
            self.path_review_cursors
                .retain(|entry| &entry.repertoire_id != repertoire_id);
        }
    }

    pub fn get(&self, key: &MoveKey) -> Option<&MoveStats> {
        self.move_stats.iter().find(|stats| &stats.key == key)
    }

    pub fn runs(&self, repertoire_id: &RepertoireId, mode: TrainingMode) -> Option<&RunStats> {
        self.runs
            .iter()
            .find(|runs| &runs.repertoire_id == repertoire_id && runs.mode == mode)
    }

    /// Compatibility queue used by the existing desktop summary. SRS sessions use the
    /// more specific `srs_queue` policies below and never automatically exhaust this list.
    pub fn queue(
        &self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        queue_mode: QueueMode,
        now: i64,
    ) -> Vec<TrainingTarget> {
        self.scoped_queue(
            tree,
            repertoire_id,
            mode,
            queue_mode,
            now,
            &TrainingScope::whole(),
        )
    }

    pub fn scoped_queue(
        &self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        queue_mode: QueueMode,
        now: i64,
        scope: &TrainingScope,
    ) -> Vec<TrainingTarget> {
        let Some(color) = mode.user_color() else {
            return Vec::new();
        };
        let targets = tree.scoped_targets(color, scope);
        let key = |target: &TrainingTarget| MoveKey {
            repertoire_id: repertoire_id.clone(),
            mode,
            position_fen4: target.position_fen4.clone(),
            expected_uci: target.expected_uci.clone(),
        };
        let mut indexed: Vec<(usize, TrainingTarget)> = targets.into_iter().enumerate().collect();
        match queue_mode {
            QueueMode::SpacedRepetition => {
                let mut due = Vec::new();
                let mut unseen = Vec::new();
                let mut unscheduled = Vec::new();
                for (order, target) in indexed {
                    match self.get(&key(&target)) {
                        Some(stats) if stats.is_due(now) => due.push((stats.due, order, target)),
                        Some(stats) if stats.due.is_none() => {
                            unscheduled.push((order, target));
                        }
                        Some(_) => {}
                        None => unseen.push((order, target)),
                    }
                }
                due.sort_by_key(|(due, order, _)| (*due, *order));
                due.into_iter()
                    .map(|(_, _, target)| target)
                    .chain(unseen.into_iter().map(|(_, target)| target))
                    .chain(unscheduled.into_iter().map(|(_, target)| target))
                    .collect()
            }
            QueueMode::LeastRecent => {
                indexed.sort_by_key(|(order, target)| {
                    let stats = self.get(&key(target));
                    (
                        !stats.is_none_or(MoveStats::unseen),
                        stats.and_then(|stats| stats.last_seen).unwrap_or(i64::MIN),
                        *order,
                    )
                });
                indexed.into_iter().map(|(_, target)| target).collect()
            }
            QueueMode::Practice | QueueMode::ReviewInOrder => {
                let scope_key = scope.key();
                let cursor = self
                    .review_cursors
                    .iter()
                    .find(|cursor| {
                        &cursor.repertoire_id == repertoire_id
                            && cursor.mode == mode
                            && cursor.scope == scope_key
                    })
                    .map_or(0, |cursor| cursor.next);
                if indexed.is_empty() {
                    return Vec::new();
                }
                let offset = cursor % indexed.len();
                indexed.rotate_left(offset);
                indexed.into_iter().map(|(_, target)| target).collect()
            }
        }
    }

    pub fn srs_queue(
        &self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        now: i64,
        scope: &TrainingScope,
        policy: SrsPolicy,
    ) -> Vec<TrainingTarget> {
        let Some(color) = mode.user_color() else {
            return Vec::new();
        };
        let key = |target: &TrainingTarget| MoveKey {
            repertoire_id: repertoire_id.clone(),
            mode,
            position_fen4: target.position_fen4.clone(),
            expected_uci: target.expected_uci.clone(),
        };
        let mut due = Vec::new();
        let mut future = Vec::new();
        let mut unseen = Vec::new();
        let mut unscheduled = Vec::new();
        for (order, target) in tree.scoped_targets(color, scope).into_iter().enumerate() {
            match self.get(&key(&target)) {
                Some(stats) if stats.is_due(now) => due.push((stats.due, order, target)),
                Some(stats) if stats.due.is_some() => future.push((stats.due, order, target)),
                Some(stats) if stats.unseen() => unseen.push((order, target)),
                Some(_) => unscheduled.push((order, target)),
                None => unseen.push((order, target)),
            }
        }
        due.sort_by_key(|(due, order, _)| (*due, *order));
        future.sort_by_key(|(due, order, _)| (*due, *order));
        match policy {
            SrsPolicy::Due => due.into_iter().map(|(_, _, target)| target).collect(),
            SrsPolicy::Introduce => unseen
                .into_iter()
                .chain(unscheduled)
                .take(INTRODUCTION_BATCH_SIZE)
                .map(|(_, target)| target)
                .collect(),
            SrsPolicy::EarlyReview => future.into_iter().map(|(_, _, target)| target).collect(),
        }
    }

    pub fn path_stats(&self, key: &PathKey) -> Option<&PathStats> {
        self.path_stats.iter().find(|stats| &stats.key == key)
    }

    pub fn path_queue(
        &self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        scope: &TrainingScope,
        queue_mode: QueueMode,
    ) -> Result<Vec<TrainingPath>, String> {
        let scope_key = scope.key();
        let mut paths = tree.training_paths(scope)?;
        if queue_mode == QueueMode::LeastRecent {
            paths.sort_by_key(|path| {
                let key = PathKey {
                    repertoire_id: repertoire_id.clone(),
                    scope: scope_key.clone(),
                    path: path.moves.clone(),
                };
                let stats = self.path_stats(&key);
                (
                    stats.is_some_and(|stats| stats.completions > 0),
                    stats
                        .and_then(|stats| stats.last_completion)
                        .unwrap_or(i64::MIN),
                )
            });
            return Ok(paths);
        }
        if paths.is_empty() {
            return Ok(paths);
        }
        let cursor = self
            .path_review_cursors
            .iter()
            .find(|cursor| &cursor.repertoire_id == repertoire_id && cursor.scope == scope_key)
            .map_or(0, |cursor| cursor.next);
        let offset = cursor % paths.len();
        paths.rotate_left(offset);
        Ok(paths)
    }

    pub(crate) fn next_practice_path(
        &mut self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        scope: &TrainingScope,
    ) -> Result<Option<TrainingPath>, String> {
        use std::ops::ControlFlow;
        let scope_key = scope.key();
        let index = match self.practice_path_cursors.iter().position(|c| {
            &c.repertoire_id == repertoire_id && c.mode == mode && c.scope == scope_key
        }) {
            Some(index) => index,
            None => {
                self.practice_path_cursors.push(ReviewCursor {
                    repertoire_id: repertoire_id.clone(),
                    mode,
                    scope: scope_key,
                    next: 0,
                });
                self.practice_path_cursors.len() - 1
            }
        };
        let cursor = self.practice_path_cursors[index].next;
        let mut count = 0usize;
        let mut chosen = None;
        tree.visit_training_paths(scope, |moves| {
            if count == cursor {
                chosen = Some(TrainingPath {
                    moves: moves.to_vec(),
                });
                return ControlFlow::Break(());
            }
            count += 1;
            ControlFlow::Continue(())
        })?;
        if chosen.is_none() && count > 0 {
            let offset = cursor % count;
            let mut order = 0;
            tree.visit_training_paths(scope, |moves| {
                if order == offset {
                    chosen = Some(TrainingPath {
                        moves: moves.to_vec(),
                    });
                    return ControlFlow::Break(());
                }
                order += 1;
                ControlFlow::Continue(())
            })?;
            self.practice_path_cursors[index].next = offset;
        }
        if chosen.is_some() {
            self.practice_path_cursors[index].next =
                self.practice_path_cursors[index].next.saturating_add(1);
        }
        Ok(chosen)
    }

    pub(crate) fn next_random_practice_path(
        &mut self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        scope: &TrainingScope,
        repertoire_fingerprint: u64,
        initial_seed: u64,
    ) -> Result<Option<RandomPracticePath>, String> {
        use std::ops::ControlFlow;

        let anchor = tree.scope_anchor(scope)?;
        let start_turn = tree
            .position(anchor)
            .map(|position| position.turn())
            .ok_or_else(|| "scope start position is missing".to_owned())?;
        let mut count = Some(0usize);
        tree.visit_source_paths(scope, |moves| {
            if eligible_target_count(mode, start_turn, moves.len()) == 0 {
                return ControlFlow::Continue(());
            }
            count = count.and_then(|count| count.checked_add(1));
            if count.is_none() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })?;
        let count =
            count.ok_or_else(|| "training path count exceeds the platform limit".to_owned())?;
        let scope_key = scope.key();
        let existing = self.random_practice_cursors.iter().position(|cursor| {
            &cursor.repertoire_id == repertoire_id
                && cursor.mode == mode
                && cursor.scope == scope_key
        });
        let is_new = existing.is_none();
        let index = existing.unwrap_or_else(|| {
            self.random_practice_cursors.push(RandomPracticeCursor {
                repertoire_id: repertoire_id.clone(),
                mode,
                scope: scope_key,
                seed: initial_seed,
                position: 0,
                count: 0,
                fingerprint: Some(repertoire_fingerprint),
            });
            self.random_practice_cursors.len() - 1
        });
        let cursor = &mut self.random_practice_cursors[index];
        if !is_new
            && (cursor.fingerprint != Some(repertoire_fingerprint)
                || cursor.count != count
                || cursor.position >= count)
        {
            cursor.seed = next_cycle_seed(cursor.seed);
            cursor.position = 0;
        }
        cursor.fingerprint = Some(repertoire_fingerprint);
        cursor.count = count;
        if count == 0 {
            cursor.position = 0;
            return Ok(None);
        }
        if cursor.position >= count {
            cursor.position = 0;
        }

        let position = cursor.position;
        let seed = cursor.seed;
        let step = coprime_step(seed ^ 0xa076_1d64_78bd_642f, count);
        let offset = (mix64(seed ^ 0xe703_7ed1_a0b4_28db) as u128 % count as u128) as usize;
        let selected_index =
            ((step as u128 * position as u128 + offset as u128) % count as u128) as usize;
        let start_seed = mix64(seed ^ (position as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut order = 0usize;
        let mut chosen = None;
        tree.visit_source_paths(scope, |moves| {
            let eligible = eligible_target_count(mode, start_turn, moves.len());
            if eligible == 0 {
                return ControlFlow::Continue(());
            }
            if order == selected_index {
                let start_order = (start_seed as usize) % eligible;
                chosen = eligible_target_index(mode, start_turn, moves.len(), start_order).map(
                    |start| RandomPracticePath {
                        path: TrainingPath {
                            moves: moves.to_vec(),
                        },
                        start,
                    },
                );
                return ControlFlow::Break(());
            }
            order += 1;
            ControlFlow::Continue(())
        })?;
        if chosen.is_some() {
            cursor.position = cursor.position.saturating_add(1);
            Ok(chosen)
        } else {
            Err("random training path selection is inconsistent".to_owned())
        }
    }

    /// Selects one route without materializing the complete path queue. Ordered
    /// review stops as soon as its cursor is reached; only wrapping needs a second pass.
    pub fn next_training_path(
        &self,
        tree: &RepertoireTree,
        repertoire_id: &RepertoireId,
        scope: &TrainingScope,
        queue_mode: QueueMode,
    ) -> Result<Option<TrainingPath>, String> {
        use std::ops::ControlFlow;
        let scope_key = scope.key();
        if queue_mode == QueueMode::LeastRecent {
            let mut best = None;
            let mut best_rank = None;
            tree.visit_training_paths(scope, |moves| {
                let stats = self.path_stats.iter().find(|stats| {
                    &stats.key.repertoire_id == repertoire_id
                        && stats.key.scope == scope_key
                        && stats.key.path == moves
                });
                let rank = (
                    stats.is_some_and(|stats| stats.completions > 0),
                    stats
                        .and_then(|stats| stats.last_completion)
                        .unwrap_or(i64::MIN),
                );
                if best_rank.is_none_or(|old| rank < old) {
                    best_rank = Some(rank);
                    best = Some(TrainingPath {
                        moves: moves.to_vec(),
                    });
                }
                // This is the minimum possible rank. Keeping its first occurrence
                // preserves the stable PGN order used by path_queue's sort.
                if rank == (false, i64::MIN) {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })?;
            return Ok(best);
        }
        let cursor = self
            .path_review_cursors
            .iter()
            .find(|cursor| &cursor.repertoire_id == repertoire_id && cursor.scope == scope_key)
            .map_or(0, |cursor| cursor.next);
        let mut count = 0usize;
        let mut selected = None;
        tree.visit_training_paths(scope, |moves| {
            if count == cursor {
                selected = Some(TrainingPath {
                    moves: moves.to_vec(),
                });
                return ControlFlow::Break(());
            }
            count += 1;
            ControlFlow::Continue(())
        })?;
        if selected.is_some() || count == 0 {
            return Ok(selected);
        }
        let offset = cursor % count;
        let mut index = 0;
        tree.visit_training_paths(scope, |moves| {
            if index == offset {
                selected = Some(TrainingPath {
                    moves: moves.to_vec(),
                });
                return ControlFlow::Break(());
            }
            index += 1;
            ControlFlow::Continue(())
        })?;
        Ok(selected)
    }

    pub(crate) fn record_wrong(
        &mut self,
        key: MoveKey,
        now: i64,
        actual_uci: &str,
        schedule: bool,
    ) {
        self.entry(key).wrong(now, actual_uci, schedule);
    }

    pub(crate) fn record_correct(
        &mut self,
        key: MoveKey,
        now: i64,
        actual_uci: &str,
        hinted: bool,
        schedule: bool,
    ) {
        self.entry(key).correct(now, actual_uci, hinted, schedule);
    }

    pub(crate) fn record_hint(&mut self, key: MoveKey, now: i64) {
        self.entry(key).hint(now);
    }

    pub(crate) fn target_completed(
        &mut self,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        queue_mode: QueueMode,
        scope: &TrainingScope,
    ) {
        if queue_mode != QueueMode::ReviewInOrder {
            return;
        }
        let scope = scope.key();
        let index = match self.review_cursors.iter().position(|cursor| {
            &cursor.repertoire_id == repertoire_id && cursor.mode == mode && cursor.scope == scope
        }) {
            Some(index) => index,
            None => {
                self.review_cursors.push(ReviewCursor {
                    repertoire_id: repertoire_id.clone(),
                    mode,
                    scope,
                    next: 0,
                });
                self.review_cursors.len() - 1
            }
        };
        self.review_cursors[index].next = self.review_cursors[index].next.saturating_add(1);
    }

    pub(crate) fn path_presented(
        &mut self,
        repertoire_id: &RepertoireId,
        scope: &TrainingScope,
        queue_mode: QueueMode,
    ) {
        if queue_mode == QueueMode::LeastRecent {
            return;
        }
        let scope = scope.key();
        let index = match self
            .path_review_cursors
            .iter()
            .position(|cursor| &cursor.repertoire_id == repertoire_id && cursor.scope == scope)
        {
            Some(index) => index,
            None => {
                self.path_review_cursors.push(PathReviewCursor {
                    repertoire_id: repertoire_id.clone(),
                    scope,
                    next: 0,
                });
                self.path_review_cursors.len() - 1
            }
        };
        self.path_review_cursors[index].next =
            self.path_review_cursors[index].next.saturating_add(1);
    }

    pub(crate) fn record_run(
        &mut self,
        repertoire_id: &RepertoireId,
        mode: TrainingMode,
        record: RunRecord,
    ) {
        let index = match self
            .runs
            .iter()
            .position(|runs| &runs.repertoire_id == repertoire_id && runs.mode == mode)
        {
            Some(index) => index,
            None => {
                self.runs.push(RunStats {
                    repertoire_id: repertoire_id.clone(),
                    mode,
                    ..RunStats::default()
                });
                self.runs.len() - 1
            }
        };
        let runs = &mut self.runs[index];
        runs.completions = runs.completions.saturating_add(1);
        runs.last = Some(record.at);
        runs.recent.push(record);
        if runs.recent.len() > RECENT {
            runs.recent.remove(0);
        }
    }

    pub(crate) fn record_path_run(&mut self, key: PathKey, record: RunRecord) {
        let index = match self.path_stats.iter().position(|stats| stats.key == key) {
            Some(index) => index,
            None => {
                self.path_stats.push(PathStats {
                    key,
                    completions: 0,
                    mistakes: 0,
                    hints: 0,
                    last_seen: None,
                    last_completion: None,
                    recent: Vec::new(),
                });
                self.path_stats.len() - 1
            }
        };
        let stats = &mut self.path_stats[index];
        stats.completions = stats.completions.saturating_add(1);
        stats.mistakes = stats.mistakes.saturating_add(record.mistakes);
        stats.hints = stats.hints.saturating_add(record.hints);
        stats.last_seen = Some(record.at);
        stats.last_completion = Some(record.at);
        stats.recent.push(record);
        if stats.recent.len() > RECENT {
            stats.recent.remove(0);
        }
    }

    pub(crate) fn migrate_position_stats(&mut self) {
        let mut merged: Vec<MoveStats> = Vec::new();
        for mut entry in self.move_stats.drain(..) {
            if entry.key.mode != TrainingMode::Both {
                if entry.interval_seconds.is_none() {
                    entry.interval_seconds = match (entry.due, entry.last_seen) {
                        (Some(due), Some(last)) if due > last => Some(due.saturating_sub(last)),
                        _ => None,
                    };
                }
                merged.push(entry);
                continue;
            }
            entry.key.expected_uci.clear();
            entry.due = None;
            entry.interval_seconds = None;
            if let Some(old) = merged.iter_mut().find(|old| old.key == entry.key) {
                old.attempts = old.attempts.saturating_add(entry.attempts);
                old.correct = old.correct.saturating_add(entry.correct);
                old.wrong = old.wrong.saturating_add(entry.wrong);
                old.hints = old.hints.saturating_add(entry.hints);
                if entry.last_attempt > old.last_attempt {
                    old.streak = entry.streak;
                }
                old.last_attempt = old.last_attempt.max(entry.last_attempt);
                old.last_seen = old.last_seen.max(entry.last_seen);
                old.recent.extend(entry.recent);
                old.recent.sort_by_key(|attempt| attempt.at);
                if old.recent.len() > RECENT {
                    old.recent.drain(..old.recent.len() - RECENT);
                }
            } else {
                merged.push(entry);
            }
        }
        self.move_stats = merged;
    }

    pub(crate) fn normalize(&mut self) {
        self.move_stats.sort_by(|a, b| a.key.cmp(&b.key));
        self.move_stats.dedup_by(|a, b| a.key == b.key);
        self.runs
            .sort_by(|a, b| (&a.repertoire_id, &a.mode).cmp(&(&b.repertoire_id, &b.mode)));
        self.path_stats.sort_by(|a, b| a.key.cmp(&b.key));
        self.review_cursors.sort_by(|a, b| {
            (&a.repertoire_id, &a.mode, &a.scope).cmp(&(&b.repertoire_id, &b.mode, &b.scope))
        });
        self.practice_path_cursors.sort_by(|a, b| {
            (&a.repertoire_id, &a.mode, &a.scope).cmp(&(&b.repertoire_id, &b.mode, &b.scope))
        });
        self.random_practice_cursors.sort_by(|a, b| {
            (&a.repertoire_id, &a.mode, &a.scope).cmp(&(&b.repertoire_id, &b.mode, &b.scope))
        });
        self.path_review_cursors
            .sort_by(|a, b| (&a.repertoire_id, &a.scope).cmp(&(&b.repertoire_id, &b.scope)));
    }

    fn entry(&mut self, key: MoveKey) -> &mut MoveStats {
        let index = match self.move_stats.iter().position(|stats| stats.key == key) {
            Some(index) => index,
            None => {
                self.move_stats.push(MoveStats::new(key));
                self.move_stats.len() - 1
            }
        };
        &mut self.move_stats[index]
    }
}

pub(crate) const MIN_RANDOM_CONTINUATION_PLIES: usize = 7;

fn random_max_start(plies: usize) -> usize {
    plies.saturating_sub(MIN_RANDOM_CONTINUATION_PLIES)
}

fn eligible_target_count(mode: TrainingMode, start_turn: Color, plies: usize) -> usize {
    if plies == 0 {
        return 0;
    }
    let max_start = random_max_start(plies);
    let Some(user_color) = mode.user_color() else {
        return usize::from(plies > 0) * (max_start + 1);
    };
    let first = usize::from(start_turn != user_color);
    if max_start < first {
        0
    } else {
        (max_start - first) / 2 + 1
    }
}

fn eligible_target_index(
    mode: TrainingMode,
    start_turn: Color,
    plies: usize,
    order: usize,
) -> Option<usize> {
    let mut remaining = order;
    for index in 0..=random_max_start(plies) {
        let turn = if index % 2 == 0 {
            start_turn
        } else {
            start_turn.other()
        };
        if mode.user_color().is_none_or(|color| color == turn) {
            if remaining == 0 {
                return Some(index);
            }
            remaining -= 1;
        }
    }
    None
}

fn coprime_step(seed: u64, count: usize) -> usize {
    if count == 1 {
        return 0;
    }
    let mut step = (mix64(seed) % count as u64) as usize;
    while gcd(step, count) != 1 {
        step = (step + 1) % count;
    }
    step
}

fn gcd(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn next_cycle_seed(seed: u64) -> u64 {
    mix64(seed ^ 0x8ebc_6af0_9c88_c6e3)
}

fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
