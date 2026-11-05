use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::aggregate::{LearningAggregate, aggregate};
use super::learning::{LearningData, LearningItem, LearningItemKey};
use super::learning_graph::{
    LearningGraphError, StatsScope, aggregate_scope, all_path_learning_memberships,
};
use super::model::{Library, RepertoireId};
use super::srs::SrsSettings;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchLearningAnalytics {
    pub repertoire_id: RepertoireId,
    pub branch_root: Vec<String>,
    pub summary: LearningAggregate,
    pub last_independent_at: Option<i64>,
}

pub fn branch_learning_analytics(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    settings: &SrsSettings,
) -> Result<Vec<BranchLearningAnalytics>, LearningGraphError> {
    let memberships = all_path_learning_memberships(library, scope)?;
    let scoped = aggregate_scope(library, learning, scope, settings)?;
    let items_by_key: BTreeMap<&LearningItemKey, &LearningItem> =
        scoped.items.iter().map(|item| (&item.key, item)).collect();
    let first_prefix_len = scope
        .branch_root
        .as_ref()
        .map_or(1, |root| root.len().max(1));
    let mut branches: BTreeMap<
        (RepertoireId, Vec<String>),
        BTreeMap<&LearningItemKey, &LearningItem>,
    > = BTreeMap::new();

    for membership in memberships {
        let Some((key, item)) = items_by_key.get_key_value(&membership.key) else {
            continue;
        };
        for prefix_len in first_prefix_len..=membership.path.len() {
            branches
                .entry((
                    membership.repertoire_id.clone(),
                    membership.path[..prefix_len].to_vec(),
                ))
                .or_default()
                .insert(*key, *item);
        }
    }

    Ok(branches
        .into_iter()
        .map(|((repertoire_id, branch_root), items)| {
            let last_independent_at = items
                .values()
                .filter_map(|item| item.last_independent_at)
                .max();
            let summary = aggregate(items.values().copied(), scope.as_of, settings);
            BranchLearningAnalytics {
                repertoire_id,
                branch_root,
                summary,
                last_independent_at,
            }
        })
        .collect())
}
