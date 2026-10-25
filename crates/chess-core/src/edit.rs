//! Tree editing with undo/redo.
//!
//! Every edit is a [`Command`] that only re-links existing arena nodes, so
//! `NodeId`s stay valid across undo/redo. Viewing an engine line never edits the
//! game; inserting it is an explicit [`Game::add_line`] with its own undo step.

use shakmaty::{CastlingMode, Move, Position, san::SanPlus, uci::UciMove};

use crate::game::{Game, GameMove, MoveKind, NodeId};
use crate::nag::Nag;

/// NAGs and comments of a node (what annotation edits change).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Annotations {
    nags: Vec<Nag>,
    comments_before: Vec<String>,
    comments_after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Attach {
        parent: NodeId,
        child: NodeId,
        index: usize,
    },
    Detach {
        parent: NodeId,
        child: NodeId,
        index: usize,
    },
    Reorder {
        parent: NodeId,
        from: usize,
        to: usize,
    },
    Annotate {
        node: NodeId,
        old: Annotations,
        new: Annotations,
    },
    /// Several commands undone and redone as one step.
    Group(Vec<Command>),
}

impl Command {
    fn inverse(&self) -> Command {
        match self {
            &Command::Attach {
                parent,
                child,
                index,
            } => Command::Detach {
                parent,
                child,
                index,
            },
            &Command::Detach {
                parent,
                child,
                index,
            } => Command::Attach {
                parent,
                child,
                index,
            },
            &Command::Reorder { parent, from, to } => Command::Reorder {
                parent,
                from: to,
                to: from,
            },
            Command::Annotate { node, old, new } => Command::Annotate {
                node: *node,
                old: new.clone(),
                new: old.clone(),
            },
            Command::Group(cmds) => {
                Command::Group(cmds.iter().rev().map(Command::inverse).collect())
            }
        }
    }

    /// Node the UI should select after this command has been applied.
    fn cursor(&self, game: &Game) -> NodeId {
        match self {
            &Command::Attach { child, .. } => child,
            &Command::Detach { parent, .. } => parent,
            &Command::Reorder { parent, to, .. } => {
                game.children(parent).get(to).copied().unwrap_or(parent)
            }
            Command::Annotate { node, .. } => *node,
            Command::Group(cmds) => cmds.first().map_or(NodeId::ROOT, |c| c.cursor(game)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("unknown node")]
    UnknownNode,
    #[error("node is not part of the current tree")]
    DetachedNode,
    #[error("illegal move {0}")]
    IllegalMove(String),
    #[error("the root cannot be removed")]
    RootNotRemovable,
    #[error("node is already on the main line of its parent")]
    AlreadyMain,
    #[error("empty line")]
    EmptyLine,
    #[error("invalid UCI move {0}")]
    InvalidUci(String),
}

/// Result of undo/redo: the node to select.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EditOutcome {
    pub cursor: NodeId,
}

impl Game {
    fn check_attached(&self, id: NodeId) -> Result<(), EditError> {
        match self.node(id) {
            None => Err(EditError::UnknownNode),
            Some(n) if !n.attached => Err(EditError::DetachedNode),
            Some(_) => Ok(()),
        }
    }

    /// Existing child of `at` reached by `m`.
    pub fn find_child(&self, at: NodeId, m: Move) -> Option<NodeId> {
        self.children(at)
            .iter()
            .copied()
            .find(|c| matches!(self.game_move(*c), Some(GameMove { kind: MoveKind::Normal(x), .. }) if *x == m))
    }

    /// Plays `m` from `at`. If that continuation exists, returns it (no edit);
    /// otherwise appends a new branch (main line if `at` has no children).
    pub fn add_move(&mut self, at: NodeId, m: Move) -> Result<NodeId, EditError> {
        self.add_line(at, &[m])
    }

    /// Adds a line (e.g. an engine PV) from `at`, reusing existing continuations
    /// and creating a new variation from the first new move. All moves are
    /// validated first; on error nothing changes. One undo step. Returns the
    /// last node of the line.
    pub fn add_line(&mut self, at: NodeId, moves: &[Move]) -> Result<NodeId, EditError> {
        self.check_attached(at)?;
        if moves.is_empty() {
            return Err(EditError::EmptyLine);
        }
        // Validate on a scratch position first.
        let mut pos = self.position(at).ok_or(EditError::UnknownNode)?.clone();
        let mut prepared = Vec::with_capacity(moves.len());
        for &m in moves {
            if !pos.is_legal(m) {
                return Err(EditError::IllegalMove(
                    m.to_uci(CastlingMode::Standard).to_string(),
                ));
            }
            let san = SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string();
            prepared.push((m, san, pos.clone()));
        }

        let mut cur = at;
        let mut iter = prepared.into_iter();
        let mut first_new = None;
        for (m, san, position) in iter.by_ref() {
            if let Some(existing) = self.find_child(cur, m) {
                cur = existing;
                continue;
            }
            let id = self.push_child(
                cur,
                GameMove {
                    kind: MoveKind::Normal(m),
                    san,
                },
                position,
            );
            first_new = Some((cur, id));
            cur = id;
            break;
        }
        for (m, san, position) in iter {
            cur = self.push_child(
                cur,
                GameMove {
                    kind: MoveKind::Normal(m),
                    san,
                },
                position,
            );
        }
        if let Some((parent, child)) = first_new {
            let index = self.child_index(child).unwrap_or(0);
            self.record(Command::Attach {
                parent,
                child,
                index,
            });
        }
        Ok(cur)
    }

    /// Adds a line given in UCI notation (`["e2e4", "e7e5"]`).
    pub fn add_uci_line<S: AsRef<str>>(
        &mut self,
        at: NodeId,
        moves: &[S],
    ) -> Result<NodeId, EditError> {
        let mut pos = self.position(at).ok_or(EditError::UnknownNode)?.clone();
        let mut parsed = Vec::with_capacity(moves.len());
        for s in moves {
            let s = s.as_ref();
            let uci: UciMove = s.parse().map_err(|_| EditError::InvalidUci(s.to_owned()))?;
            let m = uci
                .to_move(&pos)
                .map_err(|_| EditError::IllegalMove(s.to_owned()))?;
            pos.play_unchecked(m);
            parsed.push(m);
        }
        self.add_line(at, &parsed)
    }

    /// Removes `id` and everything after it from the tree. Returns the parent
    /// (suggested new cursor). Undoable.
    pub fn delete_variation(&mut self, id: NodeId) -> Result<NodeId, EditError> {
        self.check_attached(id)?;
        let parent = self.parent(id).ok_or(EditError::RootNotRemovable)?;
        let index = self.child_index(id).ok_or(EditError::UnknownNode)?;
        self.record(Command::Detach {
            parent,
            child: id,
            index,
        });
        Ok(parent)
    }

    /// Makes the variation containing `id` the main continuation at its
    /// branching point. Undoable.
    pub fn promote_variation(&mut self, id: NodeId) -> Result<(), EditError> {
        self.check_attached(id)?;
        let start = self.variation_start(id).ok_or(EditError::AlreadyMain)?;
        let parent = self.parent(start).ok_or(EditError::AlreadyMain)?;
        let from = self.child_index(start).ok_or(EditError::UnknownNode)?;
        self.record(Command::Reorder {
            parent,
            from,
            to: 0,
        });
        Ok(())
    }

    /// Replaces the NAGs and trailing comments of `id`. Undoable; no-op if unchanged.
    pub fn annotate(
        &mut self,
        id: NodeId,
        nags: Vec<Nag>,
        comments_after: Vec<String>,
    ) -> Result<(), EditError> {
        self.check_attached(id)?;
        let node = self.node(id).ok_or(EditError::UnknownNode)?;
        let old = Annotations {
            nags: node.nags.clone(),
            comments_before: node.comments_before.clone(),
            comments_after: node.comments_after.clone(),
        };
        let new = Annotations {
            nags,
            comments_before: old.comments_before.clone(),
            comments_after,
        };
        if old != new {
            self.record(Command::Annotate { node: id, old, new });
        }
        Ok(())
    }

    /// Replaces the comments before the move of `id` (an introduction to a line).
    /// Undoable; no-op if unchanged.
    pub fn set_comments_before(
        &mut self,
        id: NodeId,
        comments_before: Vec<String>,
    ) -> Result<(), EditError> {
        self.check_attached(id)?;
        let node = self.node(id).ok_or(EditError::UnknownNode)?;
        let old = Annotations {
            nags: node.nags.clone(),
            comments_before: node.comments_before.clone(),
            comments_after: node.comments_after.clone(),
        };
        let new = Annotations {
            nags: old.nags.clone(),
            comments_before,
            comments_after: old.comments_after.clone(),
        };
        if old != new {
            self.record(Command::Annotate { node: id, old, new });
        }
        Ok(())
    }

    /// Runs `f` so that all its edits form a single undo step.
    pub fn edit_group<R>(&mut self, f: impl FnOnce(&mut Game) -> R) -> R {
        let outer = self.group.replace(Vec::new());
        let result = f(self);
        let cmds = std::mem::replace(&mut self.group, outer).unwrap_or_default();
        if !cmds.is_empty() {
            match &mut self.group {
                Some(parent) => parent.push(Command::Group(cmds)),
                None => self.push_undo(Command::Group(cmds)),
            }
        }
        result
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> Option<EditOutcome> {
        let cmd = self.undo_stack.pop()?;
        let inv = cmd.inverse();
        self.apply(&inv);
        let cursor = inv.cursor(self);
        self.redo_stack.push(cmd);
        self.touch();
        Some(EditOutcome { cursor })
    }

    pub fn redo(&mut self) -> Option<EditOutcome> {
        let cmd = self.redo_stack.pop()?;
        self.apply(&cmd);
        let cursor = cmd.cursor(self);
        self.undo_stack.push(cmd);
        self.touch();
        Some(EditOutcome { cursor })
    }

    /// Applies a new command (the Attach of a freshly pushed node is already
    /// linked by `push_child`, so it is only recorded).
    pub(crate) fn record(&mut self, cmd: Command) {
        if !matches!(cmd, Command::Attach { .. }) {
            self.apply(&cmd);
        }
        if let Some(group) = &mut self.group {
            group.push(cmd);
            self.touch();
            return;
        }
        self.push_undo(cmd);
    }

    fn push_undo(&mut self, cmd: Command) {
        if self.saved_depth.is_some_and(|d| d > self.undo_stack.len()) {
            // The saved state lived in the redo stack, which is discarded now.
            self.saved_depth = None;
        }
        self.undo_stack.push(cmd);
        self.redo_stack.clear();
        self.touch();
    }

    fn apply(&mut self, cmd: &Command) {
        match cmd {
            Command::Annotate { node, new, .. } => {
                if let Some(n) = self.node_mut(*node) {
                    n.nags.clone_from(&new.nags);
                    n.comments_before.clone_from(&new.comments_before);
                    n.comments_after.clone_from(&new.comments_after);
                }
                return;
            }
            Command::Group(cmds) => {
                for c in cmds {
                    self.apply(c);
                }
                return;
            }
            _ => {}
        }
        match *cmd {
            Command::Attach {
                parent,
                child,
                index,
            } => {
                if let Some(p) = self.node_mut(parent) {
                    let i = index.min(p.children.len());
                    if !p.children.contains(&child) {
                        p.children.insert(i, child);
                    }
                }
                self.set_attached(child, true);
            }
            Command::Detach { parent, child, .. } => {
                if let Some(p) = self.node_mut(parent) {
                    p.children.retain(|c| *c != child);
                }
                self.set_attached(child, false);
            }
            Command::Reorder { parent, from, to } => {
                if let Some(p) = self.node_mut(parent)
                    && from < p.children.len()
                    && to < p.children.len()
                {
                    let c = p.children.remove(from);
                    p.children.insert(to, c);
                }
            }
            Command::Annotate { .. } | Command::Group(_) => {}
        }
    }

    fn set_attached(&mut self, id: NodeId, attached: bool) {
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if let Some(node) = self.node_mut(n) {
                node.attached = attached;
                stack.extend(node.children.iter().copied());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn modified_flag_follows_undo_to_saved_state() {
        use shakmaty::{Position, uci::UciMove};
        let mut g = crate::Game::new();
        assert!(!g.is_modified());
        let mv = |g: &crate::Game, at, u: &str| {
            u.parse::<UciMove>()
                .ok()
                .and_then(|m| m.to_move(g.position(at)?).ok())
                .expect("legal")
        };
        let m = mv(&g, NodeId::ROOT, "e2e4");
        let a = g.add_move(NodeId::ROOT, m).expect("add");
        assert!(g.is_modified());
        g.undo();
        assert!(!g.is_modified(), "undo back to the imported state");
        g.redo();
        g.mark_saved();
        assert!(!g.is_modified());
        g.undo();
        assert!(g.is_modified());
        // New edit after undoing past the saved point: saved state is unreachable.
        let m = mv(&g, NodeId::ROOT, "d2d4");
        g.add_move(NodeId::ROOT, m).expect("add");
        g.undo();
        assert!(g.is_modified());
        let _ = (a, g.position(NodeId::ROOT).map(|p| p.turn()));
    }

    use super::*;

    fn uci(g: &Game, at: NodeId, s: &str) -> Move {
        let u: UciMove = s.parse().expect("uci");
        u.to_move(g.position(at).expect("node")).expect("legal")
    }

    #[test]
    fn add_move_reuses_existing_and_branches() {
        let mut g = Game::new();
        let e4 = g.add_move(g.root(), uci(&g, g.root(), "e2e4")).expect("e4");
        let again = g.add_move(g.root(), uci(&g, g.root(), "e2e4")).expect("e4");
        assert_eq!(e4, again);
        assert_eq!(g.undo_stack.len(), 1);
        let d4 = g.add_move(g.root(), uci(&g, g.root(), "d2d4")).expect("d4");
        assert_eq!(g.children(g.root()), &[e4, d4]);
        assert_eq!(g.game_move(d4).map(|m| m.san.as_str()), Some("d4"));
        assert!(g.add_move(e4, uci(&g, g.root(), "d2d4")).is_err());
    }

    #[test]
    fn undo_redo_delete_promote() {
        let mut g = Game::new();
        let e4 = g
            .add_uci_line(g.root(), &["e2e4", "e7e5", "g1f3"])
            .expect("line");
        let c5 = g
            .add_uci_line(g.root(), &["e2e4", "c7c5"])
            .expect("variation");
        let first = g.main_child(g.root()).expect("first");
        let e5 = g.main_child(first).expect("e5");
        assert_eq!(g.children(first).len(), 2);
        assert_eq!(g.variation_start(c5), Some(c5));

        g.promote_variation(c5).expect("promote");
        assert_eq!(g.main_child(first), Some(c5));
        assert!(g.is_mainline(c5));
        g.undo();
        assert_eq!(g.main_child(first), Some(e5));

        let cursor = g.delete_variation(e5).expect("delete");
        assert_eq!(cursor, first);
        assert!(!g.is_attached(e4));
        assert_eq!(g.children(first), &[c5]);
        let out = g.undo().expect("undo");
        assert_eq!(out.cursor, e5);
        assert!(g.is_attached(e4));
        assert_eq!(g.children(first), &[e5, c5]);
        g.redo();
        assert!(!g.is_attached(e4));
        g.undo();
        // Undo the c5 insertion, then the first line.
        g.undo();
        g.undo();
        assert_eq!(g.children(g.root()), &[] as &[NodeId]);
        assert!(!g.can_undo());
        g.redo();
        assert_eq!(g.mainline().len(), 3);
        assert!(g.is_modified());
    }

    #[test]
    fn add_line_is_atomic() {
        let mut g = Game::new();
        let err = g.add_uci_line(g.root(), &["e2e4", "e2e4"]);
        assert!(err.is_err());
        assert_eq!(g.arena_len(), 1);
        assert!(!g.can_undo());
    }

    #[test]
    fn cannot_edit_detached() {
        let mut g = Game::new();
        let e4 = g.add_uci_line(g.root(), &["e2e4"]).expect("e4");
        g.delete_variation(e4).expect("delete");
        let m = uci(&g, e4, "e7e5");
        assert_eq!(g.add_move(e4, m), Err(EditError::DetachedNode));
        assert_eq!(
            g.delete_variation(g.root()),
            Err(EditError::RootNotRemovable)
        );
    }

    #[test]
    fn annotate_is_undoable() {
        let mut g = Game::new();
        let e4 = g.add_uci_line(g.root(), &["e2e4"]).expect("e4");
        g.mark_saved();
        g.annotate(e4, vec![Nag::GOOD_MOVE], vec!["[%eval 0.3]".into()])
            .expect("annotate");
        assert_eq!(
            g.node(e4).map(|n| n.nags().to_vec()),
            Some(vec![Nag::GOOD_MOVE])
        );
        assert!(g.is_modified());
        // Unchanged annotations do not create an undo step.
        let depth = g.undo_stack.len();
        g.annotate(e4, vec![Nag::GOOD_MOVE], vec!["[%eval 0.3]".into()])
            .expect("same");
        assert_eq!(g.undo_stack.len(), depth);
        assert_eq!(g.undo().map(|o| o.cursor), Some(e4));
        assert!(
            g.node(e4)
                .is_some_and(|n| n.nags().is_empty() && n.comments_after().is_empty())
        );
        assert!(!g.is_modified());
        g.redo();
        assert_eq!(g.node(e4).map(|n| n.comments_after().len()), Some(1));
    }

    #[test]
    fn edit_group_is_one_undo_step() {
        let mut g = Game::new();
        let end = g
            .add_uci_line(g.root(), &["e2e4", "e7e5", "g1f3"])
            .expect("line");
        let e4 = g.main_child(g.root()).expect("e4");
        let e5 = g.main_child(e4).expect("e5");
        let before = g.canonical_tree();
        let steps = g.undo_stack.len();
        g.edit_group(|g| {
            g.annotate(e5, vec![Nag::MISTAKE], vec!["better c5".into()])
                .expect("annotate");
            g.add_uci_line(e4, &["c7c5", "g1f3"]).expect("variation");
            g.edit_group(|g| g.annotate(end, vec![Nag::DUBIOUS_MOVE], Vec::new()))
                .expect("nested");
        });
        assert_eq!(g.undo_stack.len(), steps + 1);
        assert_eq!(g.children(e4).len(), 2);
        let after = g.canonical_tree();
        g.undo();
        assert_eq!(g.canonical_tree(), before);
        g.redo();
        assert_eq!(g.canonical_tree(), after);
        // An empty group records nothing.
        g.edit_group(|_| ());
        assert_eq!(g.undo_stack.len(), steps + 1);
    }
}
