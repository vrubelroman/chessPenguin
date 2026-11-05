use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::learning::LearningItem;
use super::srs::SrsSettings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RepertoireStatus {
    NotStarted,
    InProgress,
    AllLearnt,
    ConfirmedCurrent,
    #[default]
    Empty,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LearningAggregate {
    pub total: usize,
    pub unseen: usize,
    pub learning: usize,
    pub learnt: usize,
    pub uncertain: usize,
    pub learnt_confident: usize,
    pub due: usize,
    pub next_due_at: Option<i64>,
    pub coverage_percent: Option<f64>,
    pub learnt_percent: Option<f64>,
    pub confident_percent: Option<f64>,
    pub learning_percent: Option<f64>,
    pub unseen_percent: Option<f64>,
    pub uncertain_of_learnt_percent: Option<f64>,
    pub due_of_introduced_percent: Option<f64>,
    pub accuracy_correct: u64,
    pub accuracy_wrong: u64,
    pub accuracy_attempts: u64,
    pub accuracy_percent: Option<f64>,
    pub repertoire_status: RepertoireStatus,
}

/// Uncertain starts strictly after the grace boundary, never at the boundary itself.
pub fn is_uncertain(item: &LearningItem, now: i64, settings: &SrsSettings) -> bool {
    if !item.is_learnt() {
        return false;
    }
    let Some(next_review_at) = item.next_review_at else {
        return false;
    };
    let interval_grace = item.previous_interval_seconds.unwrap_or(0).max(0)
        / i64::from(settings.uncertain_interval_divisor.max(1));
    let grace = settings
        .uncertain_min_grace_seconds
        .max(0)
        .max(interval_grace);
    now > next_review_at.saturating_add(grace)
}

/// Counts each full LearningItemKey once, even if several scopes supplied the same item.
pub fn aggregate<'a>(
    items: impl IntoIterator<Item = &'a LearningItem>,
    now: i64,
    settings: &SrsSettings,
) -> LearningAggregate {
    let mut unique = BTreeMap::new();
    for item in items {
        unique.entry(&item.key).or_insert(item);
    }

    let mut summary = LearningAggregate::default();
    for item in unique.into_values() {
        summary.total = summary.total.saturating_add(1);
        if item.is_unseen() {
            summary.unseen = summary.unseen.saturating_add(1);
        } else if item.is_learnt() {
            summary.learnt = summary.learnt.saturating_add(1);
        } else {
            summary.learning = summary.learning.saturating_add(1);
        }
        if is_uncertain(item, now, settings) {
            summary.uncertain = summary.uncertain.saturating_add(1);
        }
        if item.is_due(now) {
            summary.due = summary.due.saturating_add(1);
        }
        if item.introduced
            && let Some(next) = item.next_review_at
            && next > now
        {
            summary.next_due_at = Some(summary.next_due_at.map_or(next, |old| old.min(next)));
        }
        summary.accuracy_correct = summary.accuracy_correct.saturating_add(item.correct_count);
        summary.accuracy_wrong = summary.accuracy_wrong.saturating_add(item.wrong_count);
    }

    if summary.due > 0 {
        summary.next_due_at = None;
    }
    summary.learnt_confident = summary.learnt.saturating_sub(summary.uncertain);
    summary.accuracy_attempts = summary
        .accuracy_correct
        .saturating_add(summary.accuracy_wrong);
    if summary.total > 0 {
        summary.coverage_percent = Some(percent(summary.total - summary.unseen, summary.total));
        summary.learnt_percent = Some(percent(summary.learnt, summary.total));
        summary.confident_percent = Some(percent(summary.learnt_confident, summary.total));
        summary.learning_percent = Some(percent(summary.learning, summary.total));
        summary.unseen_percent = Some(percent(summary.unseen, summary.total));
    }
    if summary.learnt > 0 {
        summary.uncertain_of_learnt_percent = Some(percent(summary.uncertain, summary.learnt));
    }
    let introduced = summary.total.saturating_sub(summary.unseen);
    if introduced > 0 {
        summary.due_of_introduced_percent = Some(percent(summary.due, introduced));
    }
    if summary.accuracy_attempts > 0 {
        summary.accuracy_percent =
            Some(100.0 * summary.accuracy_correct as f64 / summary.accuracy_attempts as f64);
    }
    summary.repertoire_status = if summary.total == 0 {
        RepertoireStatus::Empty
    } else if summary.unseen == summary.total {
        RepertoireStatus::NotStarted
    } else if summary.learnt == summary.total && summary.uncertain == 0 && summary.due == 0 {
        RepertoireStatus::ConfirmedCurrent
    } else if summary.learnt == summary.total {
        RepertoireStatus::AllLearnt
    } else {
        RepertoireStatus::InProgress
    };
    for violation in learning_invariant_violations(&summary) {
        eprintln!(
            "training: learning aggregate invariant violation: {violation}; summary={summary:?}"
        );
    }
    summary
}

pub fn learning_invariant_violations(summary: &LearningAggregate) -> Vec<&'static str> {
    let mut violations = Vec::new();
    let classified_total = summary
        .unseen
        .checked_add(summary.learning)
        .and_then(|total| total.checked_add(summary.learnt));
    if classified_total != Some(summary.total) {
        violations.push("total != unseen + learning + learnt");
    }
    if summary.uncertain > summary.learnt {
        violations.push("uncertain > learnt");
    }
    if summary.learnt.checked_sub(summary.uncertain) != Some(summary.learnt_confident) {
        violations.push("learnt_confident != learnt - uncertain");
    }
    if summary.due > summary.learning.saturating_add(summary.learnt) {
        violations.push("due > learning + learnt");
    }
    if let (Some(coverage), Some(learnt)) = (summary.coverage_percent, summary.learnt_percent)
        && is_valid_percentage(coverage)
        && is_valid_percentage(learnt)
        && coverage < learnt
    {
        violations.push("coverage_percent < learnt_percent");
    }
    if let (Some(learnt), Some(confident)) = (summary.learnt_percent, summary.confident_percent)
        && is_valid_percentage(learnt)
        && is_valid_percentage(confident)
        && learnt < confident
    {
        violations.push("learnt_percent < confident_percent");
    }
    for (percentage, violation) in [
        (
            summary.coverage_percent,
            "coverage_percent is not finite or is outside 0..=100",
        ),
        (
            summary.learnt_percent,
            "learnt_percent is not finite or is outside 0..=100",
        ),
        (
            summary.confident_percent,
            "confident_percent is not finite or is outside 0..=100",
        ),
        (
            summary.learning_percent,
            "learning_percent is not finite or is outside 0..=100",
        ),
        (
            summary.unseen_percent,
            "unseen_percent is not finite or is outside 0..=100",
        ),
        (
            summary.uncertain_of_learnt_percent,
            "uncertain_of_learnt_percent is not finite or is outside 0..=100",
        ),
        (
            summary.due_of_introduced_percent,
            "due_of_introduced_percent is not finite or is outside 0..=100",
        ),
        (
            summary.accuracy_percent,
            "accuracy_percent is not finite or is outside 0..=100",
        ),
    ] {
        if percentage.is_some_and(|value| !is_valid_percentage(value)) {
            violations.push(violation);
        }
    }
    violations
}

pub fn global_total_invariant_violation(
    global_total: usize,
    repertoire_totals: impl IntoIterator<Item = usize>,
) -> Option<&'static str> {
    let mut repertoire_total = 0usize;
    for total in repertoire_totals {
        let Some(next) = repertoire_total.checked_add(total) else {
            return Some("repertoire_totals sum overflow");
        };
        repertoire_total = next;
    }
    (global_total > repertoire_total).then_some("global_total > sum(repertoire_totals)")
}

pub fn log_global_total_invariant(
    global_total: usize,
    repertoire_totals: impl IntoIterator<Item = usize>,
) {
    if let Some(violation) = global_total_invariant_violation(global_total, repertoire_totals) {
        eprintln!(
            "training: global total invariant violation: {violation}; global_total={global_total}"
        );
    }
}

fn is_valid_percentage(value: f64) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

fn percent(numerator: usize, denominator: usize) -> f64 {
    100.0 * numerator as f64 / denominator as f64
}

#[cfg(test)]
mod tests {
    use super::super::learning::LearningItemKey;
    use super::*;

    fn valid_summary() -> LearningAggregate {
        LearningAggregate {
            total: 10,
            unseen: 2,
            learning: 3,
            learnt: 5,
            uncertain: 2,
            learnt_confident: 3,
            due: 4,
            next_due_at: None,
            coverage_percent: Some(80.0),
            learnt_percent: Some(50.0),
            confident_percent: Some(30.0),
            learning_percent: Some(30.0),
            unseen_percent: Some(20.0),
            uncertain_of_learnt_percent: Some(40.0),
            due_of_introduced_percent: Some(50.0),
            accuracy_correct: 7,
            accuracy_wrong: 3,
            accuracy_attempts: 10,
            accuracy_percent: Some(70.0),
            repertoire_status: RepertoireStatus::InProgress,
        }
    }

    fn assert_has_violation(summary: &LearningAggregate, expected: &'static str) {
        let violations = learning_invariant_violations(summary);
        assert!(
            violations.contains(&expected),
            "missing {expected:?} in {violations:?}"
        );
    }

    #[test]
    fn valid_mixed_summary_has_no_invariant_violations() {
        let mut summary = valid_summary();
        assert!(learning_invariant_violations(&summary).is_empty());

        summary.accuracy_percent = None;
        assert!(learning_invariant_violations(&summary).is_empty());
    }

    #[test]
    fn detects_count_invariant_violations() {
        let mut summary = valid_summary();
        summary.total = 11;
        assert_has_violation(&summary, "total != unseen + learning + learnt");

        let mut summary = valid_summary();
        summary.uncertain = 6;
        assert_has_violation(&summary, "uncertain > learnt");

        let mut summary = valid_summary();
        summary.learnt_confident = 4;
        assert_has_violation(&summary, "learnt_confident != learnt - uncertain");

        let mut summary = valid_summary();
        summary.due = 9;
        assert_has_violation(&summary, "due > learning + learnt");
    }

    #[test]
    fn detects_percentage_ordering_violations() {
        let mut summary = valid_summary();
        summary.coverage_percent = Some(40.0);
        assert_has_violation(&summary, "coverage_percent < learnt_percent");

        let mut summary = valid_summary();
        summary.learnt_percent = Some(20.0);
        assert_has_violation(&summary, "learnt_percent < confident_percent");
    }

    #[test]
    fn detects_invalid_values_in_every_percentage() {
        type PercentageSetter = fn(&mut LearningAggregate, f64);

        let fields: [(&str, PercentageSetter); 8] = [
            (
                "coverage_percent is not finite or is outside 0..=100",
                |summary, value| summary.coverage_percent = Some(value),
            ),
            (
                "learnt_percent is not finite or is outside 0..=100",
                |summary, value| summary.learnt_percent = Some(value),
            ),
            (
                "confident_percent is not finite or is outside 0..=100",
                |summary, value| summary.confident_percent = Some(value),
            ),
            (
                "learning_percent is not finite or is outside 0..=100",
                |summary, value| summary.learning_percent = Some(value),
            ),
            (
                "unseen_percent is not finite or is outside 0..=100",
                |summary, value| summary.unseen_percent = Some(value),
            ),
            (
                "uncertain_of_learnt_percent is not finite or is outside 0..=100",
                |summary, value| summary.uncertain_of_learnt_percent = Some(value),
            ),
            (
                "due_of_introduced_percent is not finite or is outside 0..=100",
                |summary, value| summary.due_of_introduced_percent = Some(value),
            ),
            (
                "accuracy_percent is not finite or is outside 0..=100",
                |summary, value| summary.accuracy_percent = Some(value),
            ),
        ];
        for (violation, set) in fields {
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 100.01] {
                let mut summary = valid_summary();
                set(&mut summary, value);
                assert_has_violation(&summary, violation);
            }
        }
    }

    #[test]
    fn global_total_invariant_allows_deduplicated_union() {
        assert_eq!(global_total_invariant_violation(3, [2, 2]), None);
    }

    #[test]
    fn global_total_invariant_detects_global_above_repertoire_sum() {
        assert_eq!(
            global_total_invariant_violation(5, [2, 2]),
            Some("global_total > sum(repertoire_totals)")
        );
    }

    #[test]
    fn global_total_invariant_detects_repertoire_sum_overflow() {
        assert_eq!(
            global_total_invariant_violation(0, [usize::MAX, 1]),
            Some("repertoire_totals sum overflow")
        );
    }

    #[test]
    fn global_total_invariant_handles_empty_repertoires() {
        assert_eq!(global_total_invariant_violation(0, []), None);
        assert_eq!(
            global_total_invariant_violation(1, []),
            Some("global_total > sum(repertoire_totals)")
        );
    }

    #[test]
    fn aggregate_output_is_unchanged() {
        fn item(
            index: usize,
            introduced: bool,
            correct_streak: u32,
            next_review_at: Option<i64>,
        ) -> LearningItem {
            let mut item =
                LearningItem::new(LearningItemKey::local(format!("fen-{index}"), "e2e4"));
            item.introduced = introduced;
            item.correct_streak = correct_streak;
            item.next_review_at = next_review_at;
            item.previous_interval_seconds = Some(0);
            item
        }

        let now = 1_000_000;
        let grace = 7 * 24 * 60 * 60;
        let mut items = [
            item(0, false, 0, None),
            item(1, true, 1, Some(now + 100)),
            item(2, true, 3, Some(now + 200)),
            item(3, true, 3, Some(now - grace - 1)),
        ];
        items[1].correct_count = 7;
        items[1].wrong_count = 3;

        let summary = aggregate(items.iter(), now, &SrsSettings::default());
        let expected = LearningAggregate {
            total: 4,
            unseen: 1,
            learning: 1,
            learnt: 2,
            uncertain: 1,
            learnt_confident: 1,
            due: 1,
            next_due_at: None,
            coverage_percent: Some(75.0),
            learnt_percent: Some(50.0),
            confident_percent: Some(25.0),
            learning_percent: Some(25.0),
            unseen_percent: Some(25.0),
            uncertain_of_learnt_percent: Some(50.0),
            due_of_introduced_percent: Some(100.0 / 3.0),
            accuracy_correct: 7,
            accuracy_wrong: 3,
            accuracy_attempts: 10,
            accuracy_percent: Some(70.0),
            repertoire_status: RepertoireStatus::InProgress,
        };
        assert_eq!(summary, expected);
        assert!(learning_invariant_violations(&summary).is_empty());
    }
}
