use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const LOCAL_PROFILE_ID: &str = "local";

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LearningItemKey {
    pub user_id: String,
    pub position_fen4: String,
    pub expected_uci: String,
}

impl LearningItemKey {
    pub fn new(
        user_id: impl Into<String>,
        position_fen4: impl Into<String>,
        expected_uci: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            position_fen4: position_fen4.into(),
            expected_uci: expected_uci.into(),
        }
    }

    pub fn local(position_fen4: impl Into<String>, expected_uci: impl Into<String>) -> Self {
        Self::new(LOCAL_PROFILE_ID, position_fen4, expected_uci)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrongMoveStats {
    pub count: u64,
    pub last_at: i64,
}

pub const RECENT_INDEPENDENT_OUTCOMES_LIMIT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IndependentResult {
    Correct,
    Wrong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndependentOutcome {
    pub result: IndependentResult,
    pub happened_at: i64,
}

/// Learning state for one user/FEN4/UCI key. Timestamps are UTC Unix seconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningItem {
    pub key: LearningItemKey,
    pub introduced: bool,
    pub correct_streak: u32,
    pub wrong_streak: u32,
    /// Only CORRECT_INDEPENDENT events increment this counter.
    pub correct_count: u64,
    /// Only INCORRECT_INDEPENDENT events increment this counter.
    pub wrong_count: u64,
    pub last_review_at: Option<i64>,
    /// SRS growth base; an independent wrong answer clears it as before.
    pub last_correct_at: Option<i64>,
    pub next_review_at: Option<i64>,
    pub previous_interval_seconds: Option<i64>,
    pub last_wrong_move_uci: Option<String>,
    pub last_wrong_at: Option<i64>,
    pub wrong_move_histogram: BTreeMap<String, WrongMoveStats>,
    /// Response times of independent attempts only; contextual events are excluded.
    pub independent_response_times_ms: Vec<u64>,
    #[serde(default)]
    pub last_independent_at: Option<i64>,
    #[serde(default)]
    pub last_independent_correct_at: Option<i64>,
    #[serde(default)]
    pub last_independent_wrong_at: Option<i64>,
    #[serde(default)]
    pub last_independent_result: Option<IndependentResult>,
    #[serde(default)]
    pub recent_independent_outcomes: Vec<IndependentOutcome>,
    #[serde(default)]
    pub resistant: bool,
}

impl LearningItem {
    pub fn new(key: LearningItemKey) -> Self {
        Self {
            key,
            introduced: false,
            correct_streak: 0,
            wrong_streak: 0,
            correct_count: 0,
            wrong_count: 0,
            last_review_at: None,
            last_correct_at: None,
            next_review_at: None,
            previous_interval_seconds: None,
            last_wrong_move_uci: None,
            last_wrong_at: None,
            wrong_move_histogram: BTreeMap::new(),
            independent_response_times_ms: Vec::new(),
            last_independent_at: None,
            last_independent_correct_at: None,
            last_independent_wrong_at: None,
            last_independent_result: None,
            recent_independent_outcomes: Vec::new(),
            resistant: false,
        }
    }

    pub fn is_unseen(&self) -> bool {
        !self.introduced
    }

    pub fn is_learning(&self) -> bool {
        self.introduced && self.correct_streak < 3
    }

    pub fn is_learnt(&self) -> bool {
        self.introduced && self.correct_streak >= 3
    }

    pub fn is_due(&self, now: i64) -> bool {
        self.introduced && self.next_review_at.is_some_and(|next| next <= now)
    }

    pub fn is_resistant(&self) -> bool {
        self.resistant
    }

    pub fn independent_attempt_count(&self) -> u64 {
        self.correct_count.saturating_add(self.wrong_count)
    }

    pub fn accuracy_percent(&self) -> Option<f64> {
        let attempts = self.independent_attempt_count();
        (attempts > 0).then(|| 100.0 * self.correct_count as f64 / attempts as f64)
    }

    pub fn response_time_median_ms(&self) -> Option<f64> {
        median(&self.independent_response_times_ms)
    }

    pub(crate) fn record_independent_outcome(&mut self, result: IndependentResult, now: i64) {
        self.last_independent_at = Some(now);
        self.last_independent_result = Some(result);
        match result {
            IndependentResult::Correct => self.last_independent_correct_at = Some(now),
            IndependentResult::Wrong => self.last_independent_wrong_at = Some(now),
        }
        self.recent_independent_outcomes.push(IndependentOutcome {
            result,
            happened_at: now,
        });
        if self.recent_independent_outcomes.len() > RECENT_INDEPENDENT_OUTCOMES_LIMIT {
            let excess = self
                .recent_independent_outcomes
                .len()
                .saturating_sub(RECENT_INDEPENDENT_OUTCOMES_LIMIT);
            self.recent_independent_outcomes.drain(..excess);
        }

        let wrong_count = self
            .recent_independent_outcomes
            .iter()
            .filter(|outcome| outcome.result == IndependentResult::Wrong)
            .count();
        let correct_since_error = self
            .recent_independent_outcomes
            .iter()
            .rev()
            .take_while(|outcome| outcome.result == IndependentResult::Correct)
            .count();
        if correct_since_error >= 3 {
            self.resistant = false;
        } else if self.recent_independent_outcomes.len() >= 5 && wrong_count >= 3 {
            self.resistant = true;
        }
    }
}

pub fn is_resistant(item: &LearningItem) -> bool {
    item.is_resistant()
}

fn median(values: &[u64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[middle - 1] as f64 + sorted[middle] as f64) / 2.0)
    } else {
        Some(sorted[middle] as f64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrainingEventType {
    CorrectIndependent,
    IncorrectIndependent,
    Previewed,
    ReplayAfterError,
    AutoContext,
    IllegalMove,
}

impl TrainingEventType {
    pub fn is_independent(self) -> bool {
        matches!(self, Self::CorrectIndependent | Self::IncorrectIndependent)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingAttempt {
    pub user_id: String,
    pub position_fen4: String,
    pub client_attempt_id: String,
    pub session_id: String,
    pub event_type: TrainingEventType,
    pub expected_uci: String,
    pub actual_uci: Option<String>,
    pub response_time_ms: Option<u64>,
    /// UTC Unix seconds assigned by the event-recording boundary.
    pub happened_at: i64,
}

impl TrainingAttempt {
    pub fn item_key(&self) -> LearningItemKey {
        LearningItemKey::new(
            self.user_id.clone(),
            self.position_fen4.clone(),
            self.expected_uci.clone(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptApplication {
    Applied,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LearningError {
    EmptyClientAttemptId,
    WrongLearningItem,
    MissingActualMove,
}

impl std::fmt::Display for LearningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyClientAttemptId => f.write_str("client attempt id must not be empty"),
            Self::WrongLearningItem => {
                f.write_str("training attempt belongs to a different learning item")
            }
            Self::MissingActualMove => {
                f.write_str("an independent wrong attempt must contain the actual UCI move")
            }
        }
    }
}

impl std::error::Error for LearningError {}

/// In-memory learning state and its idempotency journal; persistence is a separate layer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearningData {
    pub items: Vec<LearningItem>,
    pub attempts: Vec<TrainingAttempt>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl LearningData {
    pub fn item(&self, key: &LearningItemKey) -> Option<&LearningItem> {
        self.items.iter().find(|item| &item.key == key)
    }

    pub fn ensure_item(&mut self, key: LearningItemKey) -> &mut LearningItem {
        let index = match self.items.iter().position(|item| item.key == key) {
            Some(index) => index,
            None => {
                self.items.push(LearningItem::new(key));
                self.items.len() - 1
            }
        };
        &mut self.items[index]
    }
}
