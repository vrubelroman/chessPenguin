use std::collections::{BTreeMap, BTreeSet};

use chess_core::shakmaty::fen::Fen;
use chess_core::shakmaty::{CastlingMode, Chess, Color, EnPassantMode, Move, Position};
use chess_core::{Game, MoveKind, NodeId};

use super::model::Repertoire;
use super::scope::TrainingScope;

/// The position identity used by opening training: board, side, castling rights and
/// legal en passant. Halfmove and fullmove counters deliberately do not participate.
pub fn fen4(position: &Chess) -> String {
    Fen::from_position(position, EnPassantMode::Legal)
        .to_string()
        .split(' ')
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub mv: Move,
    pub uci: String,
    pub san: String,
    /// The selected PGN path. Other paths to the same move remain in the tree.
    pub node: NodeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrainingTarget {
    pub position_fen4: String,
    pub expected_uci: String,
    /// One deterministic PGN path used to reach the target as context.
    pub path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrainingPath {
    /// Actual moves from the beginning of the scope through its boundary, inclusive.
    pub moves: Vec<String>,
}

#[derive(Clone, Debug)]
struct IndexedNode {
    fen4: String,
    path: Option<Vec<String>>,
}

/// A PGN tree plus derived position/transposition information. Node IDs are used only
/// while this value is alive and are never persisted in statistics or settings.
#[derive(Clone, Debug)]
pub struct RepertoireTree {
    game: Game,
    max_plies: u32,
    disabled_prefixes: Vec<Vec<String>>,
    indexed: BTreeMap<NodeId, IndexedNode>,
    positions: BTreeMap<String, Vec<NodeId>>,
}

impl RepertoireTree {
    pub fn new(repertoire: &Repertoire) -> Self {
        Self::from_game(
            repertoire.game(),
            repertoire.max_plies,
            repertoire.disabled_prefixes.clone(),
        )
    }

    pub fn from_game(game: Game, max_plies: u32, disabled_prefixes: Vec<Vec<String>>) -> Self {
        let mut indexed = BTreeMap::new();
        let mut positions: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        for id in game.attached_nodes() {
            let Some(position) = game.position(id) else {
                continue;
            };
            let key = fen4(position);
            let path = normal_path(&game, id);
            positions.entry(key.clone()).or_default().push(id);
            indexed.insert(id, IndexedNode { fen4: key, path });
        }
        Self {
            game,
            max_plies,
            disabled_prefixes,
            indexed,
            positions,
        }
    }

    pub(crate) fn max_plies(&self) -> u32 {
        self.max_plies
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    pub fn root(&self) -> NodeId {
        self.game.root()
    }

    pub fn position(&self, at: NodeId) -> Option<&Chess> {
        self.game.position(at)
    }

    pub fn position_key(&self, at: NodeId) -> Option<&str> {
        self.indexed.get(&at).map(|n| n.fen4.as_str())
    }

    pub fn path(&self, at: NodeId) -> Option<&[String]> {
        self.indexed.get(&at)?.path.as_deref()
    }

    pub fn transpositions(&self, at: NodeId) -> &[NodeId] {
        self.position_key(at)
            .and_then(|key| self.positions.get(key))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn is_trainable_path(&self, at: NodeId) -> bool {
        let Some(node) = self.game.node(at) else {
            return false;
        };
        let Some(path) = self.path(at) else {
            return false;
        };
        node.ply() <= self.max_plies && !self.disabled(path)
    }

    /// Enabled children of the current path and all its FEN4 transpositions. A direct
    /// child is preferred so ordinary paths stay on their own PGN branch; otherwise the
    /// first PGN path is the canonical continuation. Null and unparsed-after-null paths
    /// never contribute candidates.
    pub fn candidates(&self, at: NodeId) -> Vec<Candidate> {
        self.candidates_impl(at, None)
    }

    pub fn scoped_candidates(&self, at: NodeId, scope: &TrainingScope) -> Vec<Candidate> {
        self.candidates_impl(at, Some(scope))
    }

    pub fn scoped_candidate(
        &self,
        at: NodeId,
        uci: &str,
        scope: &TrainingScope,
    ) -> Option<Candidate> {
        self.scoped_candidates(at, scope)
            .into_iter()
            .find(|candidate| candidate.uci == uci)
    }

    fn candidates_impl(&self, at: NodeId, scope: Option<&TrainingScope>) -> Vec<Candidate> {
        if !self.is_trainable_path(at)
            || self
                .game
                .node(at)
                .is_none_or(|node| node.ply() >= self.max_plies)
        {
            return Vec::new();
        }
        let mut nodes = Vec::new();
        nodes.push(at);
        nodes.extend(
            self.transpositions(at)
                .iter()
                .copied()
                .filter(|node| *node != at),
        );
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for parent in nodes {
            let Some(parent_node) = self.game.node(parent) else {
                continue;
            };
            if !self.is_trainable_path(parent)
                || parent_node.ply() >= self.max_plies
                || !self.scope_allows_parent(parent, scope)
            {
                continue;
            }
            for child in self.game.children(parent) {
                let Some(game_move) = self.game.game_move(*child) else {
                    continue;
                };
                let MoveKind::Normal(mv) = game_move.kind else {
                    continue;
                };
                if !self.is_trainable_path(*child) || !self.scope_allows_child(*child, scope) {
                    continue;
                }
                let uci = mv.to_uci(CastlingMode::Standard).to_string();
                if seen.insert(uci.clone()) {
                    out.push(Candidate {
                        mv,
                        uci,
                        san: game_move.san.clone(),
                        node: *child,
                    });
                }
            }
        }
        out
    }

    pub fn candidate(&self, at: NodeId, uci: &str) -> Option<Candidate> {
        self.candidates(at)
            .into_iter()
            .find(|candidate| candidate.uci == uci)
    }

    /// All one-side targets in deterministic PGN order, deduplicated by FEN4/UCI.
    pub fn targets(&self, side: Color) -> Vec<TrainingTarget> {
        self.targets_impl(side, None)
    }

    pub fn scoped_targets(&self, side: Color, scope: &TrainingScope) -> Vec<TrainingTarget> {
        self.targets_impl(side, Some(scope))
    }

    fn targets_impl(&self, side: Color, scope: Option<&TrainingScope>) -> Vec<TrainingTarget> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for node in self.game.attached_nodes() {
            if !self.scope_allows_parent(node, scope) {
                continue;
            }
            if self.game.position(node).is_none_or(|p| p.turn() != side) {
                continue;
            }
            let Some(path) = self.path(node).map(<[String]>::to_vec) else {
                continue;
            };
            let Some(position_fen4) = self.position_key(node).map(str::to_owned) else {
                continue;
            };
            let candidates = match scope {
                Some(scope) => self.scoped_candidates(node, scope),
                None => self.candidates(node),
            };
            for candidate in candidates {
                if seen.insert((position_fen4.clone(), candidate.uci.clone())) {
                    out.push(TrainingTarget {
                        position_fen4: position_fen4.clone(),
                        expected_uci: candidate.uci,
                        path: path.clone(),
                    });
                }
            }
        }
        out
    }

    /// Follows a normal UCI path on the actual PGN tree. No transposition jump is made
    /// while replaying context, so the selected target path remains exact.
    pub fn node_at_path(&self, path: &[String]) -> Option<NodeId> {
        let mut current = self.root();
        let mut position = self.game.position(current)?.clone();
        for uci in path {
            let mv = uci
                .parse::<chess_core::shakmaty::uci::UciMove>()
                .ok()?
                .to_move(&position)
                .ok()?;
            current = self.game.find_child(current, mv)?;
            position.play_unchecked(mv);
        }
        self.is_trainable_path(current).then_some(current)
    }

    pub fn scope_anchor(&self, scope: &TrainingScope) -> Result<NodeId, String> {
        if let Some(end) = &scope.end_prefix
            && (end.len() < scope.prefix.len() || !end.starts_with(&scope.prefix))
        {
            return Err("scope end must extend the selected prefix".into());
        }
        let anchor = if scope.prefix.is_empty() {
            self.root()
        } else {
            self.node_at_path(scope.parent_prefix())
                .ok_or_else(|| "scope parent is missing, disabled, or beyond depth".to_owned())?
        };
        if let Some(selected) = scope.prefix.last()
            && self.candidate(anchor, selected).is_none()
        {
            return Err("scope move is missing, disabled, or beyond depth".into());
        }
        if let Some(end) = &scope.end_prefix
            && self.node_at_path(end).is_none()
        {
            return Err("scope end is missing, disabled, or beyond depth".into());
        }
        Ok(anchor)
    }

    /// Generates actual scope-to-boundary routes in PGN depth-first order. Repeated FEN4
    /// positions are included as the final move and never expanded again.
    pub fn training_paths(&self, scope: &TrainingScope) -> Result<Vec<TrainingPath>, String> {
        let mut paths = Vec::new();
        self.visit_training_paths(scope, |moves| {
            paths.push(TrainingPath {
                moves: moves.to_vec(),
            });
            std::ops::ControlFlow::Continue(())
        })?;
        Ok(paths)
    }

    /// Streams routes in the same order as `training_paths`, retaining only the active
    /// route and its position history. A visitor may stop without expanding the rest.
    pub fn visit_training_paths(
        &self,
        scope: &TrainingScope,
        mut visit: impl FnMut(&[String]) -> std::ops::ControlFlow<()>,
    ) -> Result<(), String> {
        let anchor = self.scope_anchor(scope)?;
        let mut positions = BTreeSet::new();
        if let Some(key) = self.position_key(anchor) {
            positions.insert(key.to_owned());
        }
        let _ = self.visit_paths(anchor, scope, &mut Vec::new(), &mut positions, &mut visit);
        Ok(())
    }

    pub fn training_path_count(&self, scope: &TrainingScope) -> Result<usize, String> {
        let mut count = Some(0usize);
        self.visit_training_paths(scope, |_| {
            count = count.and_then(|count| count.checked_add(1));
            if count.is_none() {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        })?;
        count.ok_or_else(|| "training path count exceeds the platform limit".to_owned())
    }

    fn visit_paths(
        &self,
        at: NodeId,
        scope: &TrainingScope,
        current: &mut Vec<String>,
        positions: &mut BTreeSet<String>,
        visit: &mut impl FnMut(&[String]) -> std::ops::ControlFlow<()>,
    ) -> std::ops::ControlFlow<()> {
        let candidates = if scope.parent_prefix().len() + current.len() >= self.max_plies as usize {
            Vec::new()
        } else {
            self.scoped_candidates(at, scope)
        };
        if candidates.is_empty() {
            return if current.is_empty() {
                std::ops::ControlFlow::Continue(())
            } else {
                visit(current)
            };
        }
        // Each position has distinct UCI candidates, so each complete UCI route is
        // unique even when its continuation crosses a transposition.
        for candidate in candidates {
            current.push(candidate.uci.clone());
            let key = self.position_key(candidate.node);
            let repeated = key.is_some_and(|key| positions.contains(key));
            let result = if repeated {
                visit(current)
            } else {
                if let Some(key) = key {
                    positions.insert(key.to_owned());
                }
                let result = self.visit_paths(candidate.node, scope, current, positions, visit);
                if let Some(key) = key {
                    positions.remove(key);
                }
                result
            };
            current.pop();
            result?;
        }
        std::ops::ControlFlow::Continue(())
    }

    fn scope_allows_parent(&self, at: NodeId, scope: Option<&TrainingScope>) -> bool {
        let Some(scope) = scope else {
            return true;
        };
        let Some(path) = self.path(at) else {
            return false;
        };
        path == scope.parent_prefix() || self.path_in_scope(path, scope)
    }

    fn scope_allows_child(&self, at: NodeId, scope: Option<&TrainingScope>) -> bool {
        let Some(scope) = scope else {
            return true;
        };
        self.path(at)
            .is_some_and(|path| self.path_in_scope(path, scope))
    }

    fn path_in_scope(&self, path: &[String], scope: &TrainingScope) -> bool {
        if !path.starts_with(&scope.prefix) {
            return false;
        }
        scope
            .end_prefix
            .as_ref()
            .is_none_or(|end| path.len() <= end.len())
    }

    fn disabled(&self, path: &[String]) -> bool {
        self.disabled_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }
}

fn normal_path(game: &Game, at: NodeId) -> Option<Vec<String>> {
    let mut path = Vec::new();
    for node in game.path(at).into_iter().skip(1) {
        match game.game_move(node).map(|m| m.kind) {
            Some(MoveKind::Normal(mv)) => {
                path.push(mv.to_uci(CastlingMode::Standard).to_string());
            }
            Some(MoveKind::Null) | None => return None,
        }
    }
    Some(path)
}
