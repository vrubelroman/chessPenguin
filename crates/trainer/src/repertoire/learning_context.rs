use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::repertoire::learning::{LearningData, LearningItem, LearningItemKey};

const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
const MIN_SPEED_FACTOR: f64 = 0.25;
const MAX_SPEED_FACTOR: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearningContextPolicy {
    pub reinforce_minimum_correct: u32,
    pub reinforce_minimum_days: Option<u32>,
    pub dont_show_start_moves_threshold: u32,
    pub use_fixed_piece_speed: bool,
    pub move_speed_adjustment: f64,
    pub consider_leadup_moves: bool,
}

impl Default for LearningContextPolicy {
    fn default() -> Self {
        Self {
            reinforce_minimum_correct: 3,
            reinforce_minimum_days: None,
            dont_show_start_moves_threshold: 100,
            use_fixed_piece_speed: true,
            move_speed_adjustment: 1.0,
            consider_leadup_moves: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LearningContextAction {
    Automatic,
    Independent,
}

pub(crate) struct LearningContextMove {
    pub key: LearningItemKey,
    pub own_move: bool,
    pub least_recent_target: bool,
}

pub(crate) struct LearningContextPlan {
    pub actions: Vec<LearningContextAction>,
    pub skip_prefix: usize,
}

pub(crate) fn plan_context(
    moves: &[LearningContextMove],
    learning: &LearningData,
    policy: &LearningContextPolicy,
    completed_keys: &BTreeSet<LearningItemKey>,
    least_recent: bool,
    now: i64,
) -> LearningContextPlan {
    let actions = moves
        .iter()
        .map(|context| {
            if !context.own_move || completed_keys.contains(&context.key) {
                return LearningContextAction::Automatic;
            }
            let item = learning.item(&context.key);
            let reinforce =
                policy.consider_leadup_moves && requires_reinforcement(item, policy, now);
            if (least_recent && context.least_recent_target) || reinforce {
                LearningContextAction::Independent
            } else {
                LearningContextAction::Automatic
            }
        })
        .collect::<Vec<_>>();
    let mut skip_prefix = 0;
    let mut known_own_move = false;
    let mut stopped = false;
    for (index, (context, action)) in moves.iter().zip(&actions).enumerate() {
        if *action == LearningContextAction::Independent
            || (context.own_move && !very_well_known(learning.item(&context.key), policy))
        {
            skip_prefix = if known_own_move { index } else { 0 };
            stopped = true;
            break;
        }
        known_own_move |= context.own_move;
    }
    if known_own_move && !stopped {
        skip_prefix = moves.len();
    }
    LearningContextPlan {
        actions,
        skip_prefix,
    }
}

pub(crate) fn speed_factor(policy: &LearningContextPolicy, item: Option<&LearningItem>) -> f64 {
    if policy.use_fixed_piece_speed {
        return 1.0;
    }
    let adjustment = if policy.move_speed_adjustment.is_finite() {
        policy.move_speed_adjustment
    } else {
        1.0
    };
    let scale = u32::max(policy.reinforce_minimum_correct, 1);
    let knowledge =
        f64::from(item.map_or(0, |item| item.correct_streak).min(scale)) / f64::from(scale);
    (adjustment * (1.0 + knowledge)).clamp(MIN_SPEED_FACTOR, MAX_SPEED_FACTOR)
}

fn requires_reinforcement(
    item: Option<&LearningItem>,
    policy: &LearningContextPolicy,
    now: i64,
) -> bool {
    let weak = item.map_or(0, |item| item.correct_streak) < policy.reinforce_minimum_correct;
    let due_soon = policy.reinforce_minimum_days.is_some_and(|days| {
        let horizon = now.saturating_add(i64::from(days).saturating_mul(SECONDS_PER_DAY));
        item.is_some_and(|item| {
            item.introduced && item.next_review_at.is_some_and(|next| next <= horizon)
        })
    });
    weak || due_soon
}

fn very_well_known(item: Option<&LearningItem>, policy: &LearningContextPolicy) -> bool {
    policy.dont_show_start_moves_threshold > 0
        && item.is_some_and(|item| item.correct_streak >= policy.dont_show_start_moves_threshold)
}
