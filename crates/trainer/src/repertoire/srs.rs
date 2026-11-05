use serde::{Deserialize, Serialize};

use super::learning::{
    AttemptApplication, IndependentResult, LearningData, LearningError, LearningItem,
    LearningItemKey, TrainingAttempt, TrainingEventType, WrongMoveStats,
};

pub const DEFAULT_GROWTH_FACTOR: f64 = 2.0;
pub const FIRST_CORRECT_DELAY_SECONDS: i64 = 24 * 60 * 60;
pub const MISTAKE_DELAY_SECONDS: i64 = 30;
pub const PREVIEW_DELAY_SECONDS: i64 = 60;
pub const SRS_MAX_INTERVAL_SECONDS: i64 = 1095 * 24 * 60 * 60;
pub const MIN_PRACTICAL_INTERVAL_SECONDS: i64 = 1;
pub const UNCERTAIN_MIN_GRACE_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const UNCERTAIN_INTERVAL_DIVISOR: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrsSettings {
    pub growth_factor: f64,
    pub first_correct_delay_seconds: i64,
    pub mistake_delay_seconds: i64,
    pub preview_delay_seconds: i64,
    pub max_interval_seconds: i64,
    /// Project-specific lower bound for elapsed-time growth, not a ChessTempo default.
    pub min_practical_interval_seconds: i64,
    pub uncertain_min_grace_seconds: i64,
    pub uncertain_interval_divisor: u32,
}

impl Default for SrsSettings {
    fn default() -> Self {
        Self {
            growth_factor: DEFAULT_GROWTH_FACTOR,
            first_correct_delay_seconds: FIRST_CORRECT_DELAY_SECONDS,
            mistake_delay_seconds: MISTAKE_DELAY_SECONDS,
            preview_delay_seconds: PREVIEW_DELAY_SECONDS,
            max_interval_seconds: SRS_MAX_INTERVAL_SECONDS,
            min_practical_interval_seconds: MIN_PRACTICAL_INTERVAL_SECONDS,
            uncertain_min_grace_seconds: UNCERTAIN_MIN_GRACE_SECONDS,
            uncertain_interval_divisor: UNCERTAIN_INTERVAL_DIVISOR,
        }
    }
}

pub fn apply_independent_correct(item: &mut LearningItem, now: i64, settings: &SrsSettings) {
    let delay = match (item.correct_streak, item.last_correct_at) {
        (0, _) | (_, None) => settings.first_correct_delay_seconds.max(0),
        (_, Some(last_correct_at)) => growth_delay(now.saturating_sub(last_correct_at), settings),
    };

    item.introduced = true;
    item.correct_streak = item.correct_streak.saturating_add(1);
    item.wrong_streak = 0;
    item.correct_count = item.correct_count.saturating_add(1);
    item.last_correct_at = Some(now);
    item.last_review_at = Some(now);
    item.previous_interval_seconds = Some(delay);
    item.next_review_at = Some(now.saturating_add(delay));
    item.record_independent_outcome(IndependentResult::Correct, now);
}

pub fn apply_independent_wrong(
    item: &mut LearningItem,
    played_move: &str,
    now: i64,
    settings: &SrsSettings,
) {
    item.introduced = true;
    item.correct_streak = 0;
    item.wrong_streak = item.wrong_streak.saturating_add(1);
    item.wrong_count = item.wrong_count.saturating_add(1);
    item.last_correct_at = None;
    item.last_review_at = Some(now);
    item.next_review_at = Some(now.saturating_add(settings.mistake_delay_seconds.max(0)));
    item.last_wrong_move_uci = Some(played_move.to_owned());
    item.last_wrong_at = Some(now);
    let wrong = item
        .wrong_move_histogram
        .entry(played_move.to_owned())
        .or_insert(WrongMoveStats {
            count: 0,
            last_at: now,
        });
    wrong.count = wrong.count.saturating_add(1);
    wrong.last_at = now;
    item.record_independent_outcome(IndependentResult::Wrong, now);
}

/// Preview preserves streaks, independent counters and the last correct answer.
pub fn apply_preview(item: &mut LearningItem, now: i64, settings: &SrsSettings) {
    item.introduced = true;
    item.last_review_at = Some(now);
    item.next_review_at = Some(now.saturating_add(settings.preview_delay_seconds.max(0)));
}

pub fn apply_attempt(
    item: &mut LearningItem,
    attempt: &TrainingAttempt,
    settings: &SrsSettings,
) -> Result<(), LearningError> {
    validate_attempt(&item.key, attempt)?;
    apply_validated_attempt(item, attempt, settings);
    Ok(())
}

impl LearningData {
    /// Applies and journals an event once per `(user_id, client_attempt_id)`.
    pub fn apply_attempt(
        &mut self,
        attempt: TrainingAttempt,
        settings: &SrsSettings,
    ) -> Result<AttemptApplication, LearningError> {
        if attempt.client_attempt_id.is_empty() {
            return Err(LearningError::EmptyClientAttemptId);
        }
        if self.attempts.iter().any(|existing| {
            existing.user_id == attempt.user_id
                && existing.client_attempt_id == attempt.client_attempt_id
        }) {
            return Ok(AttemptApplication::Duplicate);
        }

        let key = attempt.item_key();
        validate_attempt(&key, &attempt)?;
        let item = self.ensure_item(key);
        apply_validated_attempt(item, &attempt, settings);
        self.attempts.push(attempt);
        Ok(AttemptApplication::Applied)
    }
}

fn growth_delay(elapsed_seconds: i64, settings: &SrsSettings) -> i64 {
    let min = settings.min_practical_interval_seconds.max(0);
    let max = settings.max_interval_seconds.max(min);
    let growth = if settings.growth_factor.is_finite() {
        settings.growth_factor.max(0.0)
    } else {
        0.0
    };
    let scaled = elapsed_seconds.max(0) as f64 * growth;
    if scaled >= max as f64 {
        max
    } else if scaled <= min as f64 {
        min
    } else {
        scaled as i64
    }
}

fn validate_attempt(key: &LearningItemKey, attempt: &TrainingAttempt) -> Result<(), LearningError> {
    if attempt.client_attempt_id.is_empty() {
        return Err(LearningError::EmptyClientAttemptId);
    }
    if key != &attempt.item_key() {
        return Err(LearningError::WrongLearningItem);
    }
    if attempt.event_type == TrainingEventType::IncorrectIndependent
        && attempt
            .actual_uci
            .as_deref()
            .is_none_or(|actual| actual.is_empty())
    {
        return Err(LearningError::MissingActualMove);
    }
    Ok(())
}

fn apply_validated_attempt(
    item: &mut LearningItem,
    attempt: &TrainingAttempt,
    settings: &SrsSettings,
) {
    match attempt.event_type {
        TrainingEventType::CorrectIndependent => {
            apply_independent_correct(item, attempt.happened_at, settings);
        }
        TrainingEventType::IncorrectIndependent => {
            apply_independent_wrong(
                item,
                attempt.actual_uci.as_deref().unwrap_or_default(),
                attempt.happened_at,
                settings,
            );
        }
        TrainingEventType::Previewed => {
            apply_preview(item, attempt.happened_at, settings);
        }
        TrainingEventType::ReplayAfterError
        | TrainingEventType::AutoContext
        | TrainingEventType::IllegalMove => return,
    }

    if attempt.event_type.is_independent()
        && let Some(response_time_ms) = attempt.response_time_ms
    {
        item.independent_response_times_ms.push(response_time_ms);
    }
}
