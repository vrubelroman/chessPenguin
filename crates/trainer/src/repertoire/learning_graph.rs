use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use chess_core::MoveKind;
use chess_core::shakmaty::{CastlingMode, Position};
use serde::{Deserialize, Serialize};

use super::aggregate::{LearningAggregate, aggregate};
use super::learning::{LearningData, LearningItem, LearningItemKey};
use super::learning_modes::LearningPriority;
use super::model::{LearningSide, Library, Repertoire, RepertoireId};
use super::srs::SrsSettings;
use super::stats::TrainingMode;
use super::tree::RepertoireTree;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StatsScope {
    pub repertoire_ids: BTreeSet<RepertoireId>,
    pub side: LearningSide,
    pub max_depth: Option<u32>,
    #[serde(default = "default_only_active")]
    pub only_active: bool,
    pub branch_root: Option<Vec<String>>,
    pub as_of: i64,
    #[serde(default)]
    pub learning_priority: LearningPriority,
}

impl StatsScope {
    pub fn new(
        repertoire_ids: impl IntoIterator<Item = RepertoireId>,
        side: LearningSide,
        as_of: i64,
    ) -> Self {
        Self {
            repertoire_ids: repertoire_ids.into_iter().collect(),
            side,
            max_depth: None,
            only_active: true,
            branch_root: None,
            as_of,
            learning_priority: LearningPriority::default(),
        }
    }

    pub fn for_repertoire(id: RepertoireId, side: LearningSide, as_of: i64) -> Self {
        Self::new([id], side, as_of)
    }

    pub fn with_max_depth(mut self, max_depth: Option<u32>) -> Self {
        self.max_depth = max_depth;
        self
    }

    pub fn with_branch_root(mut self, branch_root: Vec<String>) -> Self {
        self.branch_root = Some(branch_root);
        self
    }

    pub fn with_learning_priority(mut self, learning_priority: LearningPriority) -> Self {
        self.learning_priority = learning_priority;
        self
    }
}

fn default_only_active() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LearningItemMembership {
    pub repertoire_id: RepertoireId,
    pub key: LearningItemKey,
    pub min_depth: u32,
    pub path: Vec<String>,
    #[serde(default)]
    pub pgn_order: usize,
    #[serde(default)]
    pub variation_depth: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopedLearningAggregate {
    pub scope: StatsScope,
    pub items: Vec<LearningItem>,
    pub summary: LearningAggregate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetRepertoireImpact {
    pub id: RepertoireId,
    pub name: String,
    pub item_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetImpactProjection {
    pub reset_count: usize,
    pub shared_count: usize,
    pub affected_repertoires: Vec<ResetRepertoireImpact>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LearningGraphError {
    BothSidesNotSupported,
    RepertoireNotFound(RepertoireId),
    BranchRootNotFound {
        repertoire_id: RepertoireId,
        branch_root: Vec<String>,
    },
    DepthOverflow(RepertoireId),
    TrainingPathGeneration {
        repertoire_id: RepertoireId,
        message: String,
    },
}

impl fmt::Display for LearningGraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BothSidesNotSupported => {
                f.write_str("learning statistics require exactly one side, not Both")
            }
            Self::RepertoireNotFound(id) => write!(f, "repertoire not found: {id}"),
            Self::BranchRootNotFound {
                repertoire_id,
                branch_root,
            } => write!(
                f,
                "branch root {} is not present in repertoire {repertoire_id}",
                branch_root.join(" ")
            ),
            Self::DepthOverflow(id) => {
                write!(
                    f,
                    "learning depth exceeds the supported limit in repertoire {id}"
                )
            }
            Self::TrainingPathGeneration {
                repertoire_id,
                message,
            } => write!(
                f,
                "could not generate training paths in repertoire {repertoire_id}: {message}"
            ),
        }
    }
}

impl std::error::Error for LearningGraphError {}

impl TryFrom<TrainingMode> for LearningSide {
    type Error = LearningGraphError;

    fn try_from(mode: TrainingMode) -> Result<Self, Self::Error> {
        match mode {
            TrainingMode::White => Ok(Self::White),
            TrainingMode::Black => Ok(Self::Black),
            TrainingMode::Both => Err(LearningGraphError::BothSidesNotSupported),
        }
    }
}

#[derive(Clone, Debug)]
struct GraphEdge {
    /// Actual PGN occurrence; graph shortcuts must not invent a context path.
    source_path: Vec<String>,
    uci: String,
    to: String,
    parent_order: usize,
    child_order: usize,
    pgn_order: usize,
}

#[derive(Clone, Debug)]
struct PositionEntry {
    side: LearningSide,
    edges: Vec<GraphEdge>,
}

pub fn active_learning_memberships(
    library: &Library,
    scope: &StatsScope,
) -> Result<Vec<LearningItemMembership>, LearningGraphError> {
    let mut memberships = Vec::new();
    for id in &scope.repertoire_ids {
        let repertoire = library
            .repertoire(id)
            .ok_or_else(|| LearningGraphError::RepertoireNotFound(id.clone()))?;
        memberships.extend(repertoire_memberships(repertoire, scope)?);
    }
    memberships.sort_by(|left, right| {
        (&left.repertoire_id, &left.key).cmp(&(&right.repertoire_id, &right.key))
    });
    Ok(memberships)
}

pub fn active_learning_keys(
    library: &Library,
    scope: &StatsScope,
) -> Result<BTreeSet<LearningItemKey>, LearningGraphError> {
    Ok(active_learning_memberships(library, scope)?
        .into_iter()
        .map(|membership| membership.key)
        .collect())
}

pub fn aggregate_scope(
    library: &Library,
    learning: &LearningData,
    scope: &StatsScope,
    settings: &SrsSettings,
) -> Result<ScopedLearningAggregate, LearningGraphError> {
    let keys = active_learning_keys(library, scope)?;
    let items: Vec<_> = keys
        .into_iter()
        .map(|key| {
            learning
                .item(&key)
                .cloned()
                .unwrap_or_else(|| LearningItem::new(key))
        })
        .collect();
    let summary = aggregate(items.iter(), scope.as_of, settings);
    Ok(ScopedLearningAggregate {
        scope: scope.clone(),
        items,
        summary,
    })
}

pub fn reset_impact(
    library: &Library,
    scope: &StatsScope,
) -> Result<ResetImpactProjection, LearningGraphError> {
    let selected_memberships = active_learning_memberships(library, scope)?;
    let selected_keys: BTreeSet<_> = selected_memberships
        .iter()
        .map(|membership| membership.key.clone())
        .collect();
    let mut selected_by_repertoire: BTreeMap<RepertoireId, BTreeSet<LearningItemKey>> =
        BTreeMap::new();
    for membership in selected_memberships {
        selected_by_repertoire
            .entry(membership.repertoire_id)
            .or_default()
            .insert(membership.key);
    }

    let mut shared_keys = BTreeSet::new();
    let mut affected_repertoires = Vec::new();
    for repertoire in &library.repertoires {
        let selected = scope.repertoire_ids.contains(&repertoire.id);
        let impacted_keys = if selected {
            selected_by_repertoire
                .remove(&repertoire.id)
                .unwrap_or_default()
        } else {
            let other_scope =
                StatsScope::for_repertoire(repertoire.id.clone(), scope.side, scope.as_of);
            let other_keys = active_learning_keys(library, &other_scope)?;
            shared_keys.extend(
                other_keys
                    .intersection(&selected_keys)
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            );
            other_keys
                .intersection(&selected_keys)
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        if !impacted_keys.is_empty() {
            affected_repertoires.push(ResetRepertoireImpact {
                id: repertoire.id.clone(),
                name: repertoire.name.clone(),
                item_count: impacted_keys.len(),
            });
        }
    }

    Ok(ResetImpactProjection {
        reset_count: selected_keys.len(),
        shared_count: shared_keys.len(),
        affected_repertoires,
    })
}

// Preferred answers order the traversal, but never remove another PGN answer.
fn preferred_children(
    repertoire: &Repertoire,
    tree: &RepertoireTree,
    node: chess_core::NodeId,
) -> Vec<chess_core::NodeId> {
    let mut children = tree.game().children(node).to_vec();
    if let Some(preferred) = tree
        .position_key(node)
        .and_then(|position| repertoire.main_answer_override(position))
    {
        children.sort_by_key(|child| {
            tree.game()
                .game_move(*child)
                .and_then(|mv| mv.uci())
                .as_deref()
                != Some(preferred)
        });
    }
    children
}

fn preferred_node_order(repertoire: &Repertoire, tree: &RepertoireTree) -> Vec<chess_core::NodeId> {
    let mut nodes = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        nodes.push(node);
        stack.extend(preferred_children(repertoire, tree, node).into_iter().rev());
    }
    nodes
}

fn repertoire_memberships(
    repertoire: &Repertoire,
    scope: &StatsScope,
) -> Result<Vec<LearningItemMembership>, LearningGraphError> {
    let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
    let game = tree.game();
    let branch_root = scope.branch_root.as_deref();
    let path_allowed = |path: &[String]| {
        branch_root.is_none_or(|root| path.starts_with(root))
            && (!scope.only_active || !repertoire.is_prefix_disabled(path))
    };

    let attached_nodes = preferred_node_order(repertoire, &tree);
    let pgn_orders: BTreeMap<_, _> = attached_nodes
        .iter()
        .enumerate()
        .map(|(order, node)| (*node, order))
        .collect();
    let mut positions: BTreeMap<String, PositionEntry> = BTreeMap::new();
    let mut branch_root_found = branch_root.is_none();
    let mut start = None;
    for (parent_order, node) in attached_nodes.into_iter().enumerate() {
        let Some(path) = tree.path(node) else {
            continue;
        };
        if branch_root.is_some_and(|root| path == root) {
            branch_root_found = true;
            if path_allowed(path) {
                start = tree.position_key(node).map(str::to_owned);
            }
        }
        if !path_allowed(path) {
            continue;
        }
        let Some(position_fen4) = tree.position_key(node).map(str::to_owned) else {
            continue;
        };
        let Some(position) = tree.position(node) else {
            continue;
        };
        let entry = positions
            .entry(position_fen4.clone())
            .or_insert_with(|| PositionEntry {
                side: LearningSide::from(position.turn()),
                edges: Vec::new(),
            });
        for (child_order, child) in preferred_children(repertoire, &tree, node)
            .iter()
            .enumerate()
        {
            if !tree.path(*child).is_some_and(path_allowed) {
                continue;
            }
            let Some(game_move) = game.game_move(*child) else {
                continue;
            };
            let MoveKind::Normal(mv) = game_move.kind else {
                continue;
            };
            let Some(to) = tree.position_key(*child).map(str::to_owned) else {
                continue;
            };
            entry.edges.push(GraphEdge {
                source_path: tree.path(*child).expect("checked PGN path").to_vec(),
                uci: mv.to_uci(CastlingMode::Standard).to_string(),
                to,
                parent_order,
                child_order,
                pgn_order: pgn_orders.get(child).copied().unwrap_or(usize::MAX),
            });
        }
    }

    if !branch_root_found {
        return Err(LearningGraphError::BranchRootNotFound {
            repertoire_id: repertoire.id.clone(),
            branch_root: branch_root.unwrap_or_default().to_vec(),
        });
    }
    if branch_root.is_none() {
        start = tree.position_key(tree.root()).map(str::to_owned);
    }
    let Some(start) = start else {
        return Ok(Vec::new());
    };

    for entry in positions.values_mut() {
        entry.edges.sort_by(|left, right| {
            (left.source_path.len(), left.parent_order, left.child_order)
                .cmp(&(
                    right.source_path.len(),
                    right.parent_order,
                    right.child_order,
                ))
                .then_with(|| left.uci.cmp(&right.uci))
                .then_with(|| left.to.cmp(&right.to))
        });
        let mut seen = BTreeSet::new();
        entry.edges.retain(|edge| seen.insert(edge.uci.clone()));
    }

    let base_depth = u32::try_from(branch_root.unwrap_or_default().len())
        .map_err(|_| LearningGraphError::DepthOverflow(repertoire.id.clone()))?;
    let mut queue = VecDeque::from([(
        start.clone(),
        base_depth,
        branch_root.unwrap_or_default().to_vec(),
        0_u32,
    )]);
    let mut visited = BTreeSet::from([start]);
    let mut result: BTreeMap<LearningItemKey, LearningItemMembership> = BTreeMap::new();

    while let Some((position_fen4, depth, path, variation_depth)) = queue.pop_front() {
        if scope.max_depth.is_some_and(|maximum| depth >= maximum) {
            continue;
        }
        let Some(entry) = positions.get(&position_fen4) else {
            continue;
        };
        let own_position = entry.side == scope.side;
        let edges = &entry.edges;

        for edge in edges {
            let next_depth = depth
                .checked_add(1)
                .ok_or_else(|| LearningGraphError::DepthOverflow(repertoire.id.clone()))?;
            let next_variation_depth = variation_depth
                .checked_add(u32::from(edge.child_order > 0))
                .ok_or_else(|| LearningGraphError::DepthOverflow(repertoire.id.clone()))?;
            let mut next_path = path.clone();
            next_path.push(edge.uci.clone());
            if own_position {
                let key = LearningItemKey::local(position_fen4.clone(), edge.uci.clone());
                result
                    .entry(key.clone())
                    .or_insert_with(|| LearningItemMembership {
                        repertoire_id: repertoire.id.clone(),
                        key,
                        min_depth: next_depth,
                        path: edge.source_path.clone(),
                        pgn_order: edge.pgn_order,
                        variation_depth: next_variation_depth,
                    });
            }
            if visited.insert(edge.to.clone()) {
                queue.push_back((edge.to.clone(), next_depth, next_path, next_variation_depth));
            }
        }
    }

    Ok(result.into_values().collect())
}

pub(crate) fn all_path_learning_memberships(
    library: &Library,
    scope: &StatsScope,
) -> Result<Vec<LearningItemMembership>, LearningGraphError> {
    let mut memberships = Vec::new();
    for id in &scope.repertoire_ids {
        let repertoire = library
            .repertoire(id)
            .ok_or_else(|| LearningGraphError::RepertoireNotFound(id.clone()))?;
        memberships.extend(all_path_repertoire_memberships(repertoire, scope)?);
    }
    memberships.sort_by(|left, right| {
        (&left.repertoire_id, &left.path, &left.key).cmp(&(
            &right.repertoire_id,
            &right.path,
            &right.key,
        ))
    });
    Ok(memberships)
}

struct AllPathContext<'a> {
    repertoire: &'a Repertoire,
    scope: &'a StatsScope,
    tree: &'a RepertoireTree,
    pgn_orders: &'a BTreeMap<chess_core::NodeId, usize>,
}

fn all_path_repertoire_memberships(
    repertoire: &Repertoire,
    scope: &StatsScope,
) -> Result<Vec<LearningItemMembership>, LearningGraphError> {
    let tree = RepertoireTree::from_game(repertoire.game(), u32::MAX, Vec::new());
    let game = tree.game();
    let branch_root = scope.branch_root.as_deref();
    let path_allowed = |path: &[String]| {
        branch_root.is_none_or(|root| path.starts_with(root))
            && (!scope.only_active || !repertoire.is_prefix_disabled(path))
    };
    let start = if let Some(root) = branch_root {
        let Some(node) = game
            .attached_nodes()
            .into_iter()
            .find(|node| tree.path(*node).is_some_and(|path| path == root))
        else {
            return Err(LearningGraphError::BranchRootNotFound {
                repertoire_id: repertoire.id.clone(),
                branch_root: root.to_vec(),
            });
        };
        if !tree.path(node).is_some_and(path_allowed) {
            return Ok(Vec::new());
        }
        node
    } else {
        tree.root()
    };
    let base_depth = u32::try_from(branch_root.unwrap_or_default().len())
        .map_err(|_| LearningGraphError::DepthOverflow(repertoire.id.clone()))?;
    let attached_nodes = preferred_node_order(repertoire, &tree);
    let pgn_orders: BTreeMap<_, _> = attached_nodes
        .iter()
        .enumerate()
        .map(|(order, node)| (*node, order))
        .collect();
    let context = AllPathContext {
        repertoire,
        scope,
        tree: &tree,
        pgn_orders: &pgn_orders,
    };
    let mut memberships = Vec::new();
    let mut path = branch_root.unwrap_or_default().to_vec();
    visit_all_path_memberships(&context, start, base_depth, 0, &mut path, &mut memberships)?;
    Ok(memberships)
}

fn visit_all_path_memberships(
    context: &AllPathContext<'_>,
    node: chess_core::NodeId,
    depth: u32,
    variation_depth: u32,
    path: &mut Vec<String>,
    memberships: &mut Vec<LearningItemMembership>,
) -> Result<(), LearningGraphError> {
    if context
        .scope
        .max_depth
        .is_some_and(|maximum| depth >= maximum)
    {
        return Ok(());
    }
    let Some(position_fen4) = context.tree.position_key(node) else {
        return Ok(());
    };
    let Some(position) = context.tree.position(node) else {
        return Ok(());
    };
    let own_position = LearningSide::from(position.turn()) == context.scope.side;
    let game = context.tree.game();
    let mut seen_uci = BTreeSet::new();
    for (child_order, child) in preferred_children(context.repertoire, context.tree, node)
        .iter()
        .enumerate()
    {
        if !context.tree.path(*child).is_some_and(|child_path| {
            context
                .scope
                .branch_root
                .as_deref()
                .is_none_or(|root| child_path.starts_with(root))
                && (!context.scope.only_active
                    || !context.repertoire.is_prefix_disabled(child_path))
        }) {
            continue;
        }
        let Some(game_move) = game.game_move(*child) else {
            continue;
        };
        let MoveKind::Normal(mv) = game_move.kind else {
            continue;
        };
        let uci = mv.to_uci(CastlingMode::Standard).to_string();
        if !seen_uci.insert(uci.clone()) {
            continue;
        }
        let next_depth = depth
            .checked_add(1)
            .ok_or_else(|| LearningGraphError::DepthOverflow(context.repertoire.id.clone()))?;
        let next_variation_depth = variation_depth
            .checked_add(u32::from(child_order > 0))
            .ok_or_else(|| LearningGraphError::DepthOverflow(context.repertoire.id.clone()))?;
        path.push(uci.clone());
        if own_position {
            memberships.push(LearningItemMembership {
                repertoire_id: context.repertoire.id.clone(),
                key: LearningItemKey::local(position_fen4, uci),
                min_depth: next_depth,
                path: path.clone(),
                pgn_order: context.pgn_orders.get(child).copied().unwrap_or(usize::MAX),
                variation_depth: next_variation_depth,
            });
        }
        let result = visit_all_path_memberships(
            context,
            *child,
            next_depth,
            next_variation_depth,
            path,
            memberships,
        );
        path.pop();
        result?;
    }
    Ok(())
}
