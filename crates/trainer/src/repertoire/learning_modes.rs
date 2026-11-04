use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::learning::{LearningData, LearningItemKey};
use super::learning_graph::LearningItemMembership;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LearningPriority {
    #[default]
    DepthFirst,
    BreadthOverDepth,
    MainLinesFirst,
    CommonPositionsFirst,
}

pub(crate) fn sort_by_priority(
    memberships: &mut [LearningItemMembership],
    priority: LearningPriority,
) {
    memberships.sort_by(|left, right| compare_priority(left, right, priority));
}

pub(crate) fn sort_least_recent(
    memberships: &mut [LearningItemMembership],
    learning: &LearningData,
    priority: LearningPriority,
) {
    let states: BTreeMap<LearningItemKey, (bool, Option<i64>)> = memberships
        .iter()
        .map(|membership| {
            let key = membership.key.clone();
            let item = learning.item(&key);
            let introduced = item.is_some_and(|item| item.introduced);
            let last_attempt = learning
                .attempts
                .iter()
                .filter(|attempt| attempt.item_key() == key)
                .map(|attempt| attempt.happened_at)
                .max();
            let last_seen = item
                .and_then(|item| item.last_review_at)
                .max(last_attempt)
                .or(last_attempt);
            (key, (introduced, last_seen))
        })
        .collect();

    memberships.sort_by(|left, right| {
        let (left_introduced, left_last_seen) =
            states.get(&left.key).copied().unwrap_or((false, None));
        let (right_introduced, right_last_seen) =
            states.get(&right.key).copied().unwrap_or((false, None));
        left_introduced
            .cmp(&right_introduced)
            .then_with(|| left_last_seen.cmp(&right_last_seen))
            .then_with(|| compare_priority(left, right, priority))
    });
}

fn compare_priority(
    left: &LearningItemMembership,
    right: &LearningItemMembership,
    priority: LearningPriority,
) -> Ordering {
    left.repertoire_id
        .cmp(&right.repertoire_id)
        .then_with(|| match priority {
            LearningPriority::DepthFirst => left.pgn_order.cmp(&right.pgn_order),
            // No popularity or engine data is stored: common-position priority is
            // explicitly the deterministic breadth-first fallback.
            LearningPriority::BreadthOverDepth | LearningPriority::CommonPositionsFirst => left
                .min_depth
                .cmp(&right.min_depth)
                .then_with(|| left.pgn_order.cmp(&right.pgn_order)),
            LearningPriority::MainLinesFirst => left
                .variation_depth
                .cmp(&right.variation_depth)
                .then_with(|| left.pgn_order.cmp(&right.pgn_order)),
        })
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.key.cmp(&right.key))
}
