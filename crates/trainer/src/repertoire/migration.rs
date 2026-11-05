#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::repertoire::learning::{LearningData, LearningItem, LearningItemKey};
use crate::repertoire::stats::{MoveStats, StatsData, TrainingMode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Legacy records coalesced into an existing shared learning item.
    pub merged: usize,
    /// Shared keys whose source records disagreed about streak or schedule.
    pub conflicts: usize,
    pub skipped_both: usize,
}

pub(super) fn migrate_v2(stats: &StatsData) -> (LearningData, MigrationReport) {
    let mut report = MigrationReport::default();
    let mut groups: BTreeMap<LearningItemKey, Vec<&MoveStats>> = BTreeMap::new();
    for entry in &stats.move_stats {
        if entry.key.mode == TrainingMode::Both && entry.key.expected_uci.is_empty() {
            report.skipped_both = report.skipped_both.saturating_add(1);
            continue;
        }
        groups
            .entry(LearningItemKey::local(
                entry.key.position_fen4.clone(),
                entry.key.expected_uci.clone(),
            ))
            .or_default()
            .push(entry);
    }

    let items = groups
        .into_iter()
        .filter_map(|(key, entries)| {
            let mut iter = entries.iter().copied();
            let mut latest = iter.next()?;
            for entry in iter {
                if compare_recency(entry, latest) == Ordering::Greater {
                    latest = entry;
                }
            }

            report.merged = report
                .merged
                .saturating_add(entries.len().saturating_sub(1));
            if entries.iter().any(|entry| {
                entry.streak != latest.streak
                    || entry.due != latest.due
                    || entry.interval_seconds != latest.interval_seconds
            }) {
                report.conflicts = report.conflicts.saturating_add(1);
            }

            let mut item = LearningItem::new(key);
            item.introduced = entries
                .iter()
                .any(|entry| entry.attempts > 0 || entry.due.is_some());
            if latest.streak < 0 {
                item.wrong_streak = latest.streak.unsigned_abs();
            } else {
                item.correct_streak = latest.streak as u32;
            }
            item.last_review_at = latest.last_seen;
            item.next_review_at = entries.iter().filter_map(|entry| entry.due).min();
            item.previous_interval_seconds =
                latest
                    .interval_seconds
                    .or_else(|| match (latest.due, latest.last_seen) {
                        (Some(due), Some(last_seen)) if due > last_seen => {
                            Some(due.saturating_sub(last_seen))
                        }
                        _ => None,
                    });
            Some(item)
        })
        .collect();

    (
        LearningData {
            items,
            attempts: Vec::new(),
            extensions: BTreeMap::new(),
        },
        report,
    )
}

fn compare_recency(left: &MoveStats, right: &MoveStats) -> Ordering {
    left.last_seen
        .cmp(&right.last_seen)
        .then_with(|| left.key.repertoire_id.cmp(&right.key.repertoire_id))
        .then_with(|| mode_rank(left.key.mode).cmp(&mode_rank(right.key.mode)))
}

fn mode_rank(mode: TrainingMode) -> u8 {
    match mode {
        TrainingMode::White => 0,
        TrainingMode::Black => 1,
        TrainingMode::Both => 2,
    }
}
