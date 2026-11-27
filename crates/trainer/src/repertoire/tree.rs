use std::collections::{BTreeMap, BTreeSet};

use chess_core::shakmaty::fen::Fen;
use chess_core::shakmaty::{CastlingMode, Chess, EnPassantMode, Position};
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
    pub uci: String,
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
                    out.push(Candidate { uci });
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

    /// Streams the routes explicitly present in PGN, without importing continuations
    /// from other paths that reach the same position.
    pub fn visit_source_paths(
        &self,
        scope: &TrainingScope,
        mut visit: impl FnMut(&[String]) -> std::ops::ControlFlow<()>,
    ) -> Result<(), String> {
        let anchor = self.scope_anchor(scope)?;
        let _ = self.visit_source(anchor, scope, &mut Vec::new(), &mut visit);
        Ok(())
    }

    fn visit_source(
        &self,
        at: NodeId,
        scope: &TrainingScope,
        current: &mut Vec<String>,
        visit: &mut impl FnMut(&[String]) -> std::ops::ControlFlow<()>,
    ) -> std::ops::ControlFlow<()> {
        let mut continued = false;
        if scope.parent_prefix().len() + current.len() < self.max_plies as usize {
            for child in self.game.children(at) {
                let Some(game_move) = self.game.game_move(*child) else {
                    continue;
                };
                let MoveKind::Normal(mv) = game_move.kind else {
                    continue;
                };
                if !self.is_trainable_path(*child) || !self.scope_allows_child(*child, Some(scope))
                {
                    continue;
                }
                continued = true;
                current.push(mv.to_uci(CastlingMode::Standard).to_string());
                let result = self.visit_source(*child, scope, current, visit);
                current.pop();
                result?;
            }
        }
        if current.is_empty() || continued {
            std::ops::ControlFlow::Continue(())
        } else {
            visit(current)
        }
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
