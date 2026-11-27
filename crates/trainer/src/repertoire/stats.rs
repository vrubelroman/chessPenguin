use chess_core::shakmaty::Color;
use serde::{Deserialize, Serialize};

use super::model::RepertoireId;
use super::scope::TrainingScopeKey;

pub const STATS_VERSION: u32 = 2;
/// Recent attempts kept per move in a v2 file.
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

#[cfg(test)]
impl MoveStats {
    pub fn new(key: MoveKey) -> Self {
        Self {
            key,
            ..Self::default()
        }
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
}
