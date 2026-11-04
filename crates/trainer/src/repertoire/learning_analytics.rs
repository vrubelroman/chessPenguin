use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::aggregate::is_uncertain;
use super::learning::{
    IndependentResult, LearningData, LearningItem, LearningItemKey, WrongMoveStats,
};
use super::learning_graph::{
    LearningGraphError, LearningItemMembership, StatsScope, active_learning_memberships,
    aggregate_scope,
};
use super::model::Library;
use super::srs::SrsSettings;

pub const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
pub const SCHEDULED_WEEK_SECONDS: i64 = 7 * SECONDS_PER_DAY;
pub const SCHEDULED_MONTH_SECONDS: i64 = 30 * SECONDS_PER_DAY;
pub const SCHEDULED_YEAR_SECONDS: i64 = 365 * SECONDS_PER_DAY;

/// Fixed UTC offset used for local calendar dates. The caller resolves named zones and DST.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocalDatePolicy {
    pub utc_offset_seconds: i32,
}

impl LocalDatePolicy {
    pub const UTC: Self = Self {
        utc_offset_seconds: 0,
    };

    pub const fn fixed(utc_offset_seconds: i32) -> Self {
        Self { utc_offset_seconds }
    }

    pub fn date(self, as_of: i64) -> String {
        let local = i128::from(as_of) + i128::from(self.utc_offset_seconds);
        civil_date(local.div_euclid(i128::from(SECONDS_PER_DAY)))
    }

    /// Exclusive UTC boundary of the current local day: the next local midnight.
    pub fn end_of_local_day_exclusive(self, as_of: i64) -> i64 {
        let offset = i128::from(self.utc_offset_seconds);
        let local = i128::from(as_of) + offset;
        let next_midnight =
            (local.div_euclid(i128::from(SECONDS_PER_DAY)) + 1) * i128::from(SECONDS_PER_DAY);
        clamp_i64(next_midnight - offset)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledBuckets {
    pub overdue: usize,
    pub today: usize,
    pub week: usize,
    pub month: usize,
    pub year: usize,
    pub later: usize,
}

impl ScheduledBuckets {
    pub fn introduced(self) -> usize {
        self.overdue
            .saturating_add(self.today)
            .saturating_add(self.week)
            .saturating_add(self.month)
            .saturating_add(self.year)
            .saturating_add(self.later)
    }
}

/// Today ends at the explicit local midnight; all other ranges use fixed durations.
/// An introduced item without a schedule is overdue so the buckets remain a partition.
pub fn scheduled_buckets<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
    now: i64,
    local_date: LocalDatePolicy,
) -> ScheduledBuckets {
    let end_today = local_date.end_of_local_day_exclusive(now);
    let end_week = now.saturating_add(SCHEDULED_WEEK_SECONDS);
    let end_month = now.saturating_add(SCHEDULED_MONTH_SECONDS);
    let end_year = now.saturating_add(SCHEDULED_YEAR_SECONDS);
    let mut buckets = ScheduledBuckets::default();
    for item in unique_items(items).into_values() {
        if item.is_unseen() {
            continue;
        }
        let bucket = match item.next_review_at {
            None => &mut buckets.overdue,
            Some(next) if next <= now => &mut buckets.overdue,
            Some(next) if next < end_today => &mut buckets.today,
            Some(next) if next < end_week => &mut buckets.week,
            Some(next) if next < end_month => &mut buckets.month,
            Some(next) if next < end_year => &mut buckets.year,
            Some(_) => &mut buckets.later,
        };
        *bucket = bucket.saturating_add(1);
    }
    buckets
}

pub type CorrectStreakHistogram = BTreeMap<u32, usize>;
pub type WrongStreakHistogram = BTreeMap<u32, usize>;
pub type AttemptsHistogram = BTreeMap<u64, usize>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceHistograms {
    pub correct_streak: CorrectStreakHistogram,
    pub wrong_streak: WrongStreakHistogram,
    pub attempts: AttemptsHistogram,
}

pub fn sequence_histograms<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
) -> SequenceHistograms {
    let mut histograms = SequenceHistograms::default();
    for item in unique_items(items).into_values() {
        increment(&mut histograms.correct_streak, item.correct_streak);
        increment(&mut histograms.wrong_streak, item.wrong_streak);
        increment(&mut histograms.attempts, item.independent_attempt_count());
    }
    histograms
}

pub fn correct_streak_histogram<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
) -> CorrectStreakHistogram {
    sequence_histograms(items).correct_streak
}

pub fn wrong_streak_histogram<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
) -> WrongStreakHistogram {
    sequence_histograms(items).wrong_streak
}

pub fn attempts_histogram<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
) -> AttemptsHistogram {
    sequence_histograms(items).attempts
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MoveLearningStatus {
    Unseen,
    Learning,
    LearntFresh,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveLearningAnalytics {
    pub key: LearningItemKey,
    pub status: MoveLearningStatus,
    pub resistant: bool,
    pub accuracy_percent: Option<f64>,
    pub correct_count: u64,
    pub wrong_count: u64,
    pub independent_attempts: u64,
    pub correct_streak: u32,
    pub wrong_streak: u32,
    pub last_review_at: Option<i64>,
    pub last_independent_at: Option<i64>,
    pub last_independent_result: Option<IndependentResult>,
    pub last_independent_correct_at: Option<i64>,
    pub last_independent_wrong_at: Option<i64>,
    pub next_review_at: Option<i64>,
    pub review_in_seconds: Option<i64>,
    pub overdue_seconds: Option<i64>,
    pub uncertain_grace_seconds: Option<i64>,
    pub wrong_move_histogram: BTreeMap<String, WrongMoveStats>,
    pub response_time_median_ms: Option<f64>,
    pub memberships: Vec<LearningItemMembership>,
}

pub fn move_learning_analytics(
    item: &LearningItem,
    now: i64,
    settings: &SrsSettings,
) -> MoveLearningAnalytics {
    let uncertain = is_uncertain(item, now, settings);
    let status = if item.is_unseen() {
        MoveLearningStatus::Unseen
    } else if !item.is_learnt() {
        MoveLearningStatus::Learning
    } else if uncertain {
        MoveLearningStatus::Uncertain
    } else {
        MoveLearningStatus::LearntFresh
    };
    let uncertain_grace_seconds = (item.is_learnt() && item.next_review_at.is_some())
        .then(|| {
            item.previous_interval_seconds.unwrap_or(0).max(0)
                / i64::from(settings.uncertain_interval_divisor.max(1))
        })
        .map(|interval_grace| {
            settings
                .uncertain_min_grace_seconds
                .max(0)
                .max(interval_grace)
        });
    MoveLearningAnalytics {
        key: item.key.clone(),
        status,
        resistant: item.is_resistant(),
        accuracy_percent: item.accuracy_percent(),
        correct_count: item.correct_count,
        wrong_count: item.wrong_count,
        independent_attempts: item.independent_attempt_count(),
        correct_streak: item.correct_streak,
        wrong_streak: item.wrong_streak,
        last_review_at: item.last_review_at,
        last_independent_at: item.last_independent_at,
        last_independent_result: item.last_independent_result,
        last_independent_correct_at: item.last_independent_correct_at,
        last_independent_wrong_at: item.last_independent_wrong_at,
        next_review_at: item.next_review_at,
        review_in_seconds: item
            .next_review_at
            .filter(|next| *next > now)
            .map(|next| next.saturating_sub(now)),
        overdue_seconds: item
            .next_review_at
            .map(|next| now.saturating_sub(next).max(0)),
        uncertain_grace_seconds,
        wrong_move_histogram: item.wrong_move_histogram.clone(),
        response_time_median_ms: item.response_time_median_ms(),
        memberships: Vec::new(),
    }
}

pub fn scoped_move_learning_analytics(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    settings: &SrsSettings,
) -> Result<Vec<MoveLearningAnalytics>, LearningGraphError> {
    let scoped = aggregate_scope(library, learning, scope, settings)?;
    let mut memberships_by_key: BTreeMap<LearningItemKey, Vec<LearningItemMembership>> =
        BTreeMap::new();
    for membership in active_learning_memberships(library, scope)? {
        memberships_by_key
            .entry(membership.key.clone())
            .or_default()
            .push(membership);
    }
    Ok(scoped
        .items
        .iter()
        .map(|item| {
            let mut analytics = move_learning_analytics(item, scope.as_of, settings);
            analytics.memberships = memberships_by_key.remove(&item.key).unwrap_or_default();
            analytics
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningSnapshot {
    pub date: String,
    pub scope: StatsScope,
    pub as_of: i64,
    pub timezone: LocalDatePolicy,
    pub total: usize,
    pub unseen: usize,
    pub learning: usize,
    pub learnt: usize,
    pub uncertain: usize,
    pub due: usize,
}

impl LearningSnapshot {
    pub fn capture(
        library: &Library,
        learning: &LearningData,
        scope: &StatsScope,
        settings: &SrsSettings,
        timezone: LocalDatePolicy,
    ) -> Result<Self, LearningGraphError> {
        let scoped = aggregate_scope(library, learning, scope, settings)?;
        Ok(Self {
            date: timezone.date(scope.as_of),
            scope: scope.clone(),
            as_of: scope.as_of,
            timezone,
            total: scoped.summary.total,
            unseen: scoped.summary.unseen,
            learning: scoped.summary.learning,
            learnt: scoped.summary.learnt,
            uncertain: scoped.summary.uncertain,
            due: scoped.summary.due,
        })
    }
}

pub(crate) fn same_snapshot_slot(left: &LearningSnapshot, right: &LearningSnapshot) -> bool {
    if left.date != right.date {
        return false;
    }
    let mut left_scope = left.scope.clone();
    let mut right_scope = right.scope.clone();
    left_scope.as_of = 0;
    right_scope.as_of = 0;
    left_scope == right_scope
}

fn unique_items<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
) -> BTreeMap<&'a LearningItemKey, &'a LearningItem> {
    let mut unique = BTreeMap::new();
    for item in items {
        unique.entry(&item.key).or_insert(item);
    }
    unique
}

fn increment<K: Ord>(histogram: &mut BTreeMap<K, usize>, key: K) {
    let count = histogram.entry(key).or_default();
    *count = count.saturating_add(1);
}

fn clamp_i64(value: i128) -> i64 {
    if value < i128::from(i64::MIN) {
        i64::MIN
    } else if value > i128::from(i64::MAX) {
        i64::MAX
    } else {
        value as i64
    }
}

fn civil_date(days_since_unix_epoch: i128) -> String {
    let shifted = days_since_unix_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
}
