use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::aggregate::{LearningAggregate, aggregate};
use super::learning::{LearningData, LearningItem, LearningItemKey};
use super::learning_graph::{
    LearningGraphError, StatsScope, aggregate_scope, all_path_learning_memberships,
};
use super::model::{Library, RepertoireId};
use super::scope::TrainingScope;
use super::srs::SrsSettings;
use super::tree::RepertoireTree;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineLearningAnalytics {
    pub repertoire_id: RepertoireId,
    pub line: Vec<String>,
    pub summary: LearningAggregate,
    pub line_learnt: bool,
    pub line_fresh: bool,
}

pub fn line_learning_analytics(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    settings: &SrsSettings,
) -> Result<Vec<LineLearningAnalytics>, LearningGraphError> {
    let memberships = all_path_learning_memberships(library, scope)?;
    let scoped = aggregate_scope(library, learning, scope, settings)?;
    let items_by_key: BTreeMap<&LearningItemKey, &LearningItem> =
        scoped.items.iter().map(|item| (&item.key, item)).collect();
    let mut terminal_lines: BTreeMap<RepertoireId, Vec<Vec<String>>> = BTreeMap::new();
    for id in &scope.repertoire_ids {
        let repertoire = library
            .repertoire(id)
            .ok_or_else(|| LearningGraphError::RepertoireNotFound(id.clone()))?;
        let disabled_prefixes = if scope.only_active {
            repertoire.disabled_prefixes.clone()
        } else {
            Vec::new()
        };
        let tree = RepertoireTree::from_game(
            repertoire.game(),
            scope.max_depth.unwrap_or(u32::MAX),
            disabled_prefixes,
        );
        let lines = terminal_lines.entry(id.clone()).or_default();
        tree.visit_source_paths(&TrainingScope::whole(), |line| {
            if scope
                .branch_root
                .as_deref()
                .is_none_or(|root| line.starts_with(root))
            {
                lines.push(line.to_vec());
            }
            std::ops::ControlFlow::Continue(())
        })
        .map_err(|message| LearningGraphError::TrainingPathGeneration {
            repertoire_id: id.clone(),
            message,
        })?;
        lines.sort();
        lines.dedup();
    }

    let mut lines: BTreeMap<
        (RepertoireId, Vec<String>),
        BTreeMap<&LearningItemKey, &LearningItem>,
    > = BTreeMap::new();
    for membership in memberships {
        let Some((key, item)) = items_by_key.get_key_value(&membership.key) else {
            continue;
        };
        let Some(candidates) = terminal_lines.get(&membership.repertoire_id) else {
            continue;
        };
        let first = candidates.partition_point(|line| line.as_slice() < membership.path.as_slice());
        for line in &candidates[first..] {
            if !line.starts_with(&membership.path) {
                break;
            }
            lines
                .entry((membership.repertoire_id.clone(), line.clone()))
                .or_default()
                .insert(*key, *item);
        }
    }

    Ok(lines
        .into_iter()
        .map(|((repertoire_id, line), items)| {
            let summary = aggregate(items.values().copied(), scope.as_of, settings);
            let line_learnt =
                summary.total > 0 && items.values().all(|item| item.correct_streak >= 3);
            let line_fresh = line_learnt && summary.uncertain == 0 && summary.due == 0;
            LineLearningAnalytics {
                repertoire_id,
                line,
                summary,
                line_learnt,
                line_fresh,
            }
        })
        .collect())
}
