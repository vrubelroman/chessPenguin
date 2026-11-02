use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use chess_core::shakmaty::uci::UciMove;
use chess_core::{EditError, Game, MoveKind, NodeId, StartPosition, paste};
use serde::{Deserialize, Serialize};
use web_time::{SystemTime, UNIX_EPOCH};

use super::tree::fen4;

pub const LIBRARY_VERSION: u32 = 1;
pub const DEFAULT_MAX_PLIES: u32 = 20;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepertoireId(String);

impl RepertoireId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Self(format!("r-{nanos:x}-{}-{sequence:x}", std::process::id()))
    }

    pub(crate) fn legacy(start: &str) -> Self {
        Self(format!("legacy-{:016x}", stable_hash(start.as_bytes())))
    }
}

impl fmt::Display for RepertoireId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repertoire {
    pub id: RepertoireId,
    pub name: String,
    pub pgn: String,
    pub max_plies: u32,
    pub disabled_prefixes: Vec<Vec<String>>,
    pub path: Option<PathBuf>,
    pub detached: bool,
}

impl Default for Repertoire {
    fn default() -> Self {
        Self {
            id: RepertoireId::new(),
            name: String::new(),
            pgn: Game::new().to_pgn(),
            max_plies: DEFAULT_MAX_PLIES,
            disabled_prefixes: Vec::new(),
            path: None,
            detached: false,
        }
    }
}

impl Repertoire {
    pub fn game(&self) -> Game {
        chess_core::parse_str(&self.pgn).game
    }

    pub fn set_max_plies(&mut self, max_plies: u32) {
        self.max_plies = max_plies;
    }

    pub fn set_prefix_disabled(
        &mut self,
        prefix: &[String],
        disabled: bool,
    ) -> Result<(), RepertoireError> {
        if prefix.is_empty() || prefix.iter().any(|m| m.is_empty()) {
            return Err(RepertoireError::EmptyDisabledPrefix);
        }
        self.disabled_prefixes
            .retain(|existing| existing.as_slice() != prefix);
        if disabled {
            self.disabled_prefixes.push(prefix.to_vec());
        }
        self.disabled_prefixes.sort();
        self.disabled_prefixes.dedup();
        Ok(())
    }

    pub fn is_prefix_disabled(&self, path: &[String]) -> bool {
        self.disabled_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub version: u32,
    pub revision: u64,
    pub repertoires: Vec<Repertoire>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            version: LIBRARY_VERSION,
            revision: 0,
            repertoires: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub added_nodes: usize,
    pub merged_nodes: usize,
    pub annotation_changes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyMigrationReport {
    pub repertoires: usize,
    pub lines: usize,
    pub skipped: Vec<String>,
}

#[derive(Debug)]
pub enum RepertoireError {
    EmptyName,
    NotFound(RepertoireId),
    DuplicateId(RepertoireId),
    EmptyDisabledPrefix,
    DifferentStart { expected: String, actual: String },
    UnparsedMerge,
    Merge(String),
    Edit(EditError),
}

impl fmt::Display for RepertoireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => f.write_str("repertoire name must not be empty"),
            Self::NotFound(id) => write!(f, "repertoire not found: {id}"),
            Self::DuplicateId(id) => write!(f, "repertoire id is already present: {id}"),
            Self::EmptyDisabledPrefix => f.write_str("disabled UCI prefix must not be empty"),
            Self::DifferentStart { expected, actual } => write!(
                f,
                "different starting positions: repertoire has {expected}, imported game has {actual}"
            ),
            Self::UnparsedMerge => {
                f.write_str("cannot merge an unparsed PGN tail into an existing move")
            }
            Self::Merge(message) => write!(f, "PGN merge failed: {message}"),
            Self::Edit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RepertoireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Edit(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EditError> for RepertoireError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

impl Library {
    pub fn validate(&self) -> Result<(), RepertoireError> {
        let mut ids = BTreeSet::new();
        for repertoire in &self.repertoires {
            if !ids.insert(repertoire.id.clone()) {
                return Err(RepertoireError::DuplicateId(repertoire.id.clone()));
            }
        }
        Ok(())
    }

    pub fn repertoire(&self, id: &RepertoireId) -> Option<&Repertoire> {
        self.repertoires.iter().find(|r| &r.id == id)
    }

    pub fn repertoire_mut(&mut self, id: &RepertoireId) -> Option<&mut Repertoire> {
        self.repertoires.iter_mut().find(|r| &r.id == id)
    }

    pub fn create_empty(
        &mut self,
        name: impl Into<String>,
    ) -> Result<RepertoireId, RepertoireError> {
        self.create(name, &Game::new())
    }

    pub fn create(
        &mut self,
        name: impl Into<String>,
        game: &Game,
    ) -> Result<RepertoireId, RepertoireError> {
        let name = checked_name(name.into())?;
        let id = RepertoireId::new();
        self.repertoires.push(Repertoire {
            id: id.clone(),
            name,
            pgn: game.to_pgn(),
            max_plies: DEFAULT_MAX_PLIES,
            disabled_prefixes: Vec::new(),
            path: None,
            detached: false,
        });
        Ok(id)
    }

    pub fn rename(
        &mut self,
        id: &RepertoireId,
        name: impl Into<String>,
    ) -> Result<(), RepertoireError> {
        let name = checked_name(name.into())?;
        self.repertoire_mut(id)
            .ok_or_else(|| RepertoireError::NotFound(id.clone()))?
            .name = name;
        Ok(())
    }

    pub fn delete(&mut self, id: &RepertoireId) -> Result<Repertoire, RepertoireError> {
        let index = self
            .repertoires
            .iter()
            .position(|r| &r.id == id)
            .ok_or_else(|| RepertoireError::NotFound(id.clone()))?;
        Ok(self.repertoires.remove(index))
    }

    /// Merges the complete source tree. The source game is never modified; an error
    /// leaves the repertoire at its previous PGN snapshot.
    pub fn add_game(
        &mut self,
        id: &RepertoireId,
        source: &Game,
    ) -> Result<MergeReport, RepertoireError> {
        let repertoire = self
            .repertoire_mut(id)
            .ok_or_else(|| RepertoireError::NotFound(id.clone()))?;
        let mut target = repertoire.game();
        check_start(&target, source)?;
        let target_is_empty = target.children(NodeId::ROOT).is_empty()
            && target.node(NodeId::ROOT).is_some_and(|root| {
                root.nags().is_empty()
                    && root.comments_before().is_empty()
                    && root.comments_after().is_empty()
                    && root.unparsed().is_empty()
            });
        if target_is_empty {
            repertoire.pgn = source.to_pgn();
            return Ok(MergeReport {
                added_nodes: source.attached_nodes().len().saturating_sub(1),
                merged_nodes: 1,
                annotation_changes: 0,
            });
        }
        let mut report = MergeReport::default();
        merge_annotations(&mut target, NodeId::ROOT, source, NodeId::ROOT, &mut report)?;
        report.merged_nodes += 1;
        merge_children(
            &mut target,
            NodeId::ROOT,
            source,
            NodeId::ROOT,
            true,
            &mut report,
        )?;
        repertoire.pgn = target.to_pgn();
        Ok(report)
    }

    /// Adds exactly the path from the source root through `at`, without siblings or
    /// descendants of `at`. Node comments and NAGs on the selected path are merged.
    pub fn add_prefix(
        &mut self,
        id: &RepertoireId,
        source: &Game,
        at: NodeId,
    ) -> Result<MergeReport, RepertoireError> {
        if !source.is_attached(at) {
            return Err(RepertoireError::Merge(
                "selected source node is not attached".into(),
            ));
        }
        let repertoire = self
            .repertoire_mut(id)
            .ok_or_else(|| RepertoireError::NotFound(id.clone()))?;
        let mut target = repertoire.game();
        check_start(&target, source)?;
        let mut report = MergeReport::default();
        merge_annotations(&mut target, NodeId::ROOT, source, NodeId::ROOT, &mut report)?;
        report.merged_nodes += 1;
        let mut target_parent = NodeId::ROOT;
        for source_node in source.path(at).into_iter().skip(1) {
            target_parent = merge_node(
                &mut target,
                target_parent,
                source,
                source_node,
                false,
                &mut report,
            )?;
        }
        repertoire.pgn = target.to_pgn();
        Ok(report)
    }

    /// Deterministically reconstructs playable lines stored in legacy line keys. The
    /// legacy aggregates remain in `StatsData::legacy_lines`; no per-move history is
    /// inferred from them.
    pub fn migrate_legacy_lines(
        &mut self,
        legacy: &serde_json::Map<String, serde_json::Value>,
    ) -> LegacyMigrationReport {
        let mut groups: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
        let mut report = LegacyMigrationReport::default();
        for key in legacy.keys() {
            let Some((start, moves)) = key.split_once(" | ") else {
                report.skipped.push(key.clone());
                continue;
            };
            let line: Vec<String> = moves
                .split_whitespace()
                .filter(|m| !m.is_empty())
                .map(str::to_owned)
                .collect();
            if line.is_empty() || line.iter().any(|m| m.parse::<UciMove>().is_err()) {
                report.skipped.push(key.clone());
                continue;
            }
            groups.entry(start.to_owned()).or_default().push(line);
        }
        for (start, mut lines) in groups {
            lines.sort();
            lines.dedup();
            let id = RepertoireId::legacy(&start);
            if self.repertoire(&id).is_none() {
                let Ok(position) = StartPosition::from_fen(&format!("{start} 0 1")) else {
                    report
                        .skipped
                        .extend(lines.into_iter().map(|line| line.join(" ")));
                    continue;
                };
                let number = self
                    .repertoires
                    .iter()
                    .filter(|r| r.id.as_str().starts_with("legacy-"))
                    .count()
                    + 1;
                self.repertoires.push(Repertoire {
                    id: id.clone(),
                    name: format!("Перенесённые линии {number}"),
                    pgn: Game::from_start(position).to_pgn(),
                    max_plies: DEFAULT_MAX_PLIES,
                    disabled_prefixes: Vec::new(),
                    path: None,
                    detached: false,
                });
                report.repertoires += 1;
            }
            let Some(mut game) = self.repertoire(&id).map(Repertoire::game) else {
                continue;
            };
            for line in lines {
                match game.add_uci_line(NodeId::ROOT, &line) {
                    Ok(_) => report.lines += 1,
                    Err(_) => report.skipped.push(line.join(" ")),
                }
            }
            if let Some(repertoire) = self.repertoire_mut(&id) {
                repertoire.pgn = game.to_pgn();
            }
        }
        report
    }
}

fn checked_name(name: String) -> Result<String, RepertoireError> {
    let name = name.trim();
    if name.is_empty() {
        Err(RepertoireError::EmptyName)
    } else {
        Ok(name.to_owned())
    }
}

fn check_start(target: &Game, source: &Game) -> Result<(), RepertoireError> {
    let expected = fen4(target.start().position());
    let actual = fen4(source.start().position());
    if expected == actual {
        Ok(())
    } else {
        Err(RepertoireError::DifferentStart { expected, actual })
    }
}

fn merge_children(
    target: &mut Game,
    target_parent: NodeId,
    source: &Game,
    source_parent: NodeId,
    recursive: bool,
    report: &mut MergeReport,
) -> Result<(), RepertoireError> {
    for source_child in source.children(source_parent) {
        merge_node(
            target,
            target_parent,
            source,
            *source_child,
            recursive,
            report,
        )?;
    }
    Ok(())
}

fn merge_node(
    target: &mut Game,
    target_parent: NodeId,
    source: &Game,
    source_node: NodeId,
    recursive: bool,
    report: &mut MergeReport,
) -> Result<NodeId, RepertoireError> {
    let kind = source
        .game_move(source_node)
        .map(|m| m.kind)
        .ok_or_else(|| RepertoireError::Merge("source node has no move".into()))?;
    if let Some(existing) = target
        .children(target_parent)
        .iter()
        .copied()
        .find(|child| target.game_move(*child).is_some_and(|m| m.kind == kind))
    {
        report.merged_nodes += 1;
        merge_annotations(target, existing, source, source_node, report)?;
        if recursive {
            merge_unparsed(target, existing, source, source_node)?;
        }
        if recursive {
            merge_children(target, existing, source, source_node, true, report)?;
        }
        return Ok(existing);
    }

    let plain = matches!(kind, MoveKind::Normal(_))
        && source
            .node(source_node)
            .is_some_and(|n| n.unparsed().is_empty());
    let target_node = if plain {
        let MoveKind::Normal(mv) = kind else {
            return Err(RepertoireError::Merge("invalid normal move".into()));
        };
        let id = target.add_line(target_parent, &[mv])?;
        merge_annotations(target, id, source, source_node, report)?;
        report.added_nodes += 1;
        id
    } else {
        let text = subtree_pgn(source, source_node, recursive, recursive);
        // Clipboard matching searches the entire target for a matching position and
        // may choose another occurrence. An empty local target makes the graft exact:
        // no matched prefix, transposition or extra branch can redirect the insertion.
        let fen = target
            .fen(target_parent)
            .ok_or_else(|| RepertoireError::Merge("missing target parent position".into()))?;
        let start = StartPosition::from_fen(&fen)
            .map_err(|error| RepertoireError::Merge(error.to_string()))?;
        let local = Game::from_start(start);
        let mut plan = paste::plan(&local, NodeId::ROOT, &text)
            .map_err(|e| RepertoireError::Merge(format!("{e:?}")))?;
        if plan.at != NodeId::ROOT {
            return Err(RepertoireError::Merge(
                "PGN graft did not start at its root".into(),
            ));
        }
        plan.at = target_parent;
        plan.continuation = target.children(target_parent).is_empty();
        let id = target.apply_paste(&plan)?;
        let count = if recursive {
            subtree_len(source, source_node)
        } else {
            1
        };
        report.added_nodes += count;
        id
    };
    if plain && recursive {
        merge_children(target, target_node, source, source_node, true, report)?;
    }
    Ok(target_node)
}

fn merge_unparsed(
    target: &Game,
    target_node: NodeId,
    source: &Game,
    source_node: NodeId,
) -> Result<(), RepertoireError> {
    let Some(source_node) = source.node(source_node) else {
        return Ok(());
    };
    if source_node.unparsed().is_empty() {
        return Ok(());
    }
    if target
        .node(target_node)
        .is_some_and(|node| node.unparsed() == source_node.unparsed())
    {
        Ok(())
    } else {
        Err(RepertoireError::UnparsedMerge)
    }
}

fn subtree_len(game: &Game, root: NodeId) -> usize {
    let mut stack = vec![root];
    let mut count = 0;
    while let Some(node) = stack.pop() {
        count += 1;
        stack.extend(game.children(node));
    }
    count
}

fn merge_annotations(
    target: &mut Game,
    target_node: NodeId,
    source: &Game,
    source_node: NodeId,
    report: &mut MergeReport,
) -> Result<(), RepertoireError> {
    let (Some(source_node), Some(current)) = (source.node(source_node), target.node(target_node))
    else {
        return Err(RepertoireError::Merge("missing annotation node".into()));
    };
    let nags = union(current.nags().to_vec(), source_node.nags().to_vec());
    let before = union(
        current.comments_before().to_vec(),
        source_node.comments_before().to_vec(),
    );
    let after = union(
        current.comments_after().to_vec(),
        source_node.comments_after().to_vec(),
    );
    let changed = nags.len() != current.nags().len()
        || before.len() != current.comments_before().len()
        || after.len() != current.comments_after().len();
    target.annotate(target_node, nags, after)?;
    target.set_comments_before(target_node, before)?;
    report.annotation_changes += usize::from(changed);
    Ok(())
}

fn union<T: Clone + PartialEq>(mut target: Vec<T>, source: Vec<T>) -> Vec<T> {
    for value in source {
        if !target.contains(&value) {
            target.push(value);
        }
    }
    target
}

fn subtree_pgn(source: &Game, node: NodeId, recursive: bool, raw: bool) -> String {
    let fen = source
        .position_before(node)
        .map(|p| {
            chess_core::shakmaty::fen::Fen::from_position(
                p,
                chess_core::shakmaty::EnPassantMode::Legal,
            )
        })
        .map(|f| f.to_string())
        .unwrap_or_else(|| source.start().fen());
    let mut out = format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n");
    write_move(source, node, &mut out, raw);
    if recursive {
        write_line(source, node, &mut out);
    }
    out.push_str(" *\n");
    out
}

fn write_line(game: &Game, parent: NodeId, out: &mut String) {
    let mut current = parent;
    while let Some(main) = game.children(current).first().copied() {
        write_move(game, main, out, true);
        for alternative in game.children(current).iter().skip(1) {
            out.push_str(" (");
            write_move(game, *alternative, out, true);
            write_line(game, *alternative, out);
            out.push(')');
        }
        current = main;
    }
}

fn write_move(game: &Game, node: NodeId, out: &mut String, raw: bool) {
    let Some(data) = game.node(node) else {
        return;
    };
    for comment in data.comments_before() {
        write_comment(comment, out);
    }
    if let Some(mv) = data.game_move() {
        out.push(' ');
        out.push_str(&mv.san);
    }
    for nag in data.nags() {
        out.push_str(&format!(" ${}", nag.0));
    }
    for comment in data.comments_after() {
        write_comment(comment, out);
    }
    if raw {
        for text in data.unparsed() {
            out.push(' ');
            out.push_str(text);
        }
    }
}

fn write_comment(comment: &str, out: &mut String) {
    if comment.contains('}') {
        for line in comment.lines() {
            out.push_str(&format!("\n;{}\n", line.trim()));
        }
    } else {
        out.push_str(&format!(" {{{comment}}}"));
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
