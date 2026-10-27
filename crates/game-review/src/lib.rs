//! Whole-game analysis, shared by the desktop and the Android app: the engine evaluates
//! every position of the main line, and the evaluations give the move judgements
//! (inaccuracy, mistake, blunder), the accuracy and the evaluation graph.
//!
//! [`GameAnalysis`] holds the evaluations of one game and what follows from them; it
//! never changes the game. [`run`] is the engine job (its own engine process) that
//! produces them. Results are cached per main line, and `[%eval]` comments of the PGN
//! are used without an engine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chess_core::shakmaty::san::SanPlus;
use chess_core::shakmaty::uci::UciMove;
use chess_core::shakmaty::{CastlingMode, Chess, Color, Move, Position};
use chess_core::{Annotation, CommentPart, Game, NodeId, parse_comment};
use serde::{Deserialize, Serialize};
use uci::{EnginePosition, Side};

mod features;
mod job;
pub mod report;

pub use features::{MoveFeatures, Rules};
pub use job::{JobEvent, JobFailure, JobSettings, Target, run};
pub use report::{Eval, Judgement, Phase, Phases, Ply, Report, SideStats};

/// Engine result for one position of the main line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PosEval {
    pub eval: Eval,
    /// Principal variation in UCI; the first move is the engine's best move.
    #[serde(default)]
    pub pv: Vec<String>,
    #[serde(default)]
    pub depth: u32,
    /// The best move but the one played, when that one was the engine's best and could
    /// be brilliant or great (older analyses have none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<Second>,
}

/// The best of the other moves: how the position stood without the move played.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Second {
    /// The move left out (UCI).
    pub without: String,
    pub eval: Eval,
}

/// Where the evaluations came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewMeta {
    pub engine: String,
    /// Nodes per position; 0 = read from `[%eval]` comments of the PGN.
    pub nodes: u64,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    meta: ReviewMeta,
    evals: Vec<(String, PosEval)>,
}

const CACHE_VERSION: u32 = 1;

/// An engine assessment of a main-line move: an error, or a move of note (!!, !).
#[derive(Clone, Debug)]
pub struct Mark {
    pub judgement: Judgement,
    /// Node before the move (where the better line starts).
    pub base: NodeId,
    pub best: Vec<Move>,
    /// The better line in SAN (Latin letters, as in PGN).
    pub best_san: Vec<String>,
    /// "23…Nf6".
    pub best_label: Option<String>,
}

/// The evaluations of a game's main line and the report built from them.
pub struct GameAnalysis {
    evals: HashMap<String, PosEval>,
    pub meta: Option<ReviewMeta>,
    built_rev: Option<u64>,
    dirty: bool,
    /// Root and main-line nodes: the analysed positions.
    line: Vec<NodeId>,
    keys: Vec<String>,
    /// Side making each move of the line.
    movers: Vec<Color>,
    /// Evaluation per position (engine result or a terminal position).
    values: Vec<Option<Eval>>,
    /// Positions without legal moves (mate, stalemate): no engine needed.
    terminal: Vec<bool>,
    /// What each move of the line does on the board (`features[i]`: the move from
    /// position `i`); `None` for a null move.
    features: Vec<Option<MoveFeatures>>,
    /// The move played from each position of the line (UCI; `None` for a null move).
    played: Vec<Option<String>>,
    pub report: Report,
    pub marks: HashMap<NodeId, Mark>,
    /// The rules of !! and ! the report follows.
    rules: Rules,
}

impl GameAnalysis {
    /// The analysis of `game`: from the cache in `cache_dir` (the folder of the cached
    /// analyses) when this main line was analysed before, else from `[%eval]` comments
    /// when every move has one, else empty.
    pub fn new(game: &Game, cache_dir: Option<&Path>) -> Self {
        let mut a = GameAnalysis {
            evals: HashMap::new(),
            meta: None,
            built_rev: None,
            dirty: false,
            line: Vec::new(),
            keys: Vec::new(),
            movers: Vec::new(),
            values: Vec::new(),
            terminal: Vec::new(),
            features: Vec::new(),
            played: Vec::new(),
            report: Report::default(),
            marks: HashMap::new(),
            rules: Rules::default(),
        };
        a.rebuild(game);
        if cache_dir.is_some_and(|d| a.load_cache(d)) || a.load_pgn_evals(game) {
            a.rebuild(game);
        }
        a
    }

    pub fn has_data(&self) -> bool {
        !self.evals.is_empty()
    }

    /// Root and main-line nodes, in order: index `i` of [`Self::values`] belongs here.
    pub fn line(&self) -> &[NodeId] {
        &self.line
    }

    /// Evaluation per position of [`Self::line`].
    pub fn values(&self) -> &[Option<Eval>] {
        &self.values
    }

    /// The side making each move of the line (`movers[i]` plays from position `i`).
    pub fn movers(&self) -> &[Color] {
        &self.movers
    }

    /// The engine result for position `i` of the line, if there is one.
    pub fn eval_at(&self, i: usize) -> Option<&PosEval> {
        self.keys.get(i).and_then(|k| self.evals.get(k))
    }

    /// Positions of the current main line without an evaluation.
    pub fn missing(&self) -> usize {
        self.values.iter().filter(|v| v.is_none()).count()
    }

    /// Positions an engine would look at in a full analysis (not a final mate or
    /// stalemate).
    pub fn positions(&self) -> usize {
        self.terminal.iter().filter(|t| !**t).count()
    }

    /// Engine searches a full analysis is expected to make, for its time: one per
    /// position, and a second one for about half of the moves that could be brilliant or
    /// great (those the engine finds best too).
    pub fn searches(&self) -> usize {
        let candidates = self
            .features
            .iter()
            .flatten()
            .filter(|f| f.candidate())
            .count();
        self.positions() + candidates / 2
    }

    /// Follows `rules` for !! and ! from the next [`Self::refresh`] on (the evaluations
    /// serve both).
    pub fn set_rules(&mut self, rules: Rules) {
        if self.rules != rules {
            self.rules = rules;
            self.dirty = true;
        }
    }

    /// Re-derives the report when the game or the evaluations changed. Cheap otherwise.
    pub fn refresh(&mut self, game: &Game) {
        if self.dirty || self.built_rev != Some(game.revision()) {
            self.rebuild(game);
        }
    }

    /// Prepares an engine run: `full` drops the earlier results. Returns the positions
    /// without an evaluation, for [`run`]. Filling in an engine analysis keeps its
    /// nodes per position, so that one report never mixes two qualities: the run must use
    /// `self.meta`'s nodes, not the asked ones.
    pub fn begin(&mut self, game: &Game, meta: ReviewMeta, full: bool) -> Vec<Target> {
        if full {
            self.evals.clear();
        }
        let nodes = match &self.meta {
            Some(old) if !full && old.nodes > 0 && self.has_data() => old.nodes,
            _ => meta.nodes,
        };
        self.meta = Some(ReviewMeta { nodes, ..meta });
        self.rebuild(game);
        (0..self.line.len())
            .filter_map(|i| {
                let known = self.evals.get(&self.keys[i]).cloned();
                // Already evaluated: only a missing second search can be left.
                if known.is_some() && !self.second_missing(i)
                    || known.is_none() && self.values[i].is_some()
                {
                    return None;
                }
                let here = game.position(self.line[i])?;
                // The move played, when it could be brilliant or great.
                let played = self
                    .features
                    .get(i)
                    .copied()
                    .flatten()
                    .filter(|f| f.candidate())
                    .and_then(|_| self.played.get(i).cloned().flatten());
                let others = match &played {
                    Some(p) => here
                        .legal_moves()
                        .iter()
                        .map(|m| m.to_uci(CastlingMode::Standard).to_string())
                        .filter(|u| u != p)
                        .collect(),
                    None => Vec::new(),
                };
                let (key, pos) = position_key(game, self.line[i])?;
                let turn = match here.turn() {
                    Color::White => Side::White,
                    Color::Black => Side::Black,
                };
                Some(Target {
                    key,
                    pos,
                    turn,
                    played,
                    others,
                    known,
                })
            })
            .collect()
    }

    /// Position `i` is evaluated, its move could be brilliant or great and was the
    /// engine's best, but the best other move was never searched for (an analysis from
    /// before !! and !).
    fn second_missing(&self, i: usize) -> bool {
        let (Some(e), Some(Some(played)), Some(Some(f))) = (
            self.evals.get(&self.keys[i]),
            self.played.get(i),
            self.features.get(i),
        ) else {
            return false;
        };
        f.candidate()
            && e.pv.first() == Some(played)
            && report::worth_second(e.eval, self.movers[i])
            && e.second.as_ref().map(|s| &s.without) != Some(played)
    }

    /// Positions whose evaluation is there but which still want the second search of
    /// !! and ! ("Update" fills them in without evaluating anything again). None for
    /// evaluations from the PGN, which have no engine lines.
    pub fn seconds_missing(&self) -> usize {
        if self.meta.as_ref().is_none_or(|m| m.nodes == 0) {
            return 0;
        }
        (0..self.played.len())
            .filter(|i| self.second_missing(*i))
            .count()
    }

    /// Takes in what the engine job reported.
    pub fn apply(&mut self, event: JobEvent) {
        match event {
            JobEvent::Engine(name) => {
                if let Some(m) = &mut self.meta {
                    m.engine = name;
                }
            }
            JobEvent::Eval { key, eval, .. } => {
                self.evals.insert(key, eval);
                self.dirty = true;
            }
        }
    }

    fn rebuild(&mut self, game: &Game) {
        self.line = std::iter::once(NodeId::ROOT)
            .chain(game.mainline())
            .collect();
        let positions: Vec<Chess> = self
            .line
            .iter()
            .filter_map(|id| game.position(*id).cloned())
            .collect();
        self.keys = self
            .line
            .iter()
            .map(|id| position_key(game, *id).map(|k| k.0).unwrap_or_default())
            .collect();
        self.terminal = (0..self.line.len())
            .map(|i| positions.get(i).and_then(terminal_eval).is_some())
            .collect();
        self.values = (0..self.line.len())
            .map(|i| {
                self.evals
                    .get(&self.keys[i])
                    .map(|e| e.eval)
                    .or_else(|| positions.get(i).and_then(terminal_eval))
            })
            .collect();
        let moves: Vec<Option<Move>> = (1..self.line.len())
            .map(|i| match game.game_move(self.line[i]).map(|m| m.kind) {
                Some(chess_core::MoveKind::Normal(m)) => Some(m),
                _ => None,
            })
            .collect();
        self.features = (0..moves.len())
            .map(|i| {
                let (m, before) = (moves[i].as_ref()?, positions.get(i)?);
                let prev = i.checked_sub(1).and_then(|p| moves[p].as_ref());
                Some(MoveFeatures::of(before, m, prev))
            })
            .collect();
        self.played = (1..self.line.len())
            .map(|i| game.game_move(self.line[i]).and_then(|m| m.uci()))
            .collect();
        let mut plies = Vec::with_capacity(self.line.len().saturating_sub(1));
        for i in 1..self.line.len() {
            let mv = game.game_move(self.line[i]);
            let e = self.evals.get(&self.keys[i - 1]);
            let best = e.and_then(|e| e.pv.first());
            let played = mv.and_then(|m| m.uci());
            plies.push(Ply {
                mover: positions.get(i - 1).map_or(Color::White, |p| p.turn()),
                null: mv.is_none_or(|m| m.is_null()),
                best: best.is_some() && played.as_ref() == best,
                features: self.features[i - 1],
                // Only for the move played now (the line may have changed since).
                second: e
                    .and_then(|e| e.second.as_ref())
                    .filter(|s| Some(&s.without) == played.as_ref())
                    .map(|s| s.eval),
            });
        }
        self.movers = plies.iter().map(|p| p.mover).collect();
        self.report =
            Report::build_with(&self.values, &plies, Phases::divide(&positions), self.rules);
        self.marks.clear();
        for (i, j) in self.report.judgements.iter().enumerate() {
            let Some(judgement) = *j else { continue };
            let (base, node) = (self.line[i], self.line[i + 1]);
            // A better line only for an error: a good move was the best one.
            let (best, best_san) = match (self.evals.get(&self.keys[i]), positions.get(i)) {
                (Some(e), Some(root)) if judgement.is_error() => parse_pv(root, &e.pv),
                _ => (Vec::new(), Vec::new()),
            };
            let best_label = best_san.first().and_then(|san| {
                let (num, color) = game.move_number(node)?;
                Some(match color {
                    Color::White => format!("{num}.{san}"),
                    Color::Black => format!("{num}…{san}"),
                })
            });
            self.marks.insert(
                node,
                Mark {
                    judgement,
                    base,
                    best,
                    best_san,
                    best_label,
                },
            );
        }
        self.built_rev = Some(game.revision());
        self.dirty = false;
    }

    /// After an inaccuracy, mistake or blunder: how the opponent punishes it and how
    /// the player best answers, the first two moves of the engine line from the position
    /// after the move (shown as arrows, as on Lichess).
    pub fn refutation(&self, node: NodeId, game: &Game) -> Vec<Move> {
        if !self
            .marks
            .get(&node)
            .is_some_and(|m| m.judgement.is_error())
        {
            return Vec::new();
        }
        let (Some(i), Some(pos)) = (
            self.line.iter().position(|n| *n == node),
            game.position(node),
        ) else {
            return Vec::new();
        };
        let Some(e) = self.eval_at(i) else {
            return Vec::new();
        };
        let (mut moves, _) = parse_pv(pos, &e.pv);
        moves.truncate(2);
        moves
    }

    /// Index in the main line of `cursor`, or of the main-line move a variation leaves from.
    pub fn line_index(&self, cursor: NodeId, game: &Game) -> Option<usize> {
        let mut id = cursor;
        loop {
            if let Some(i) = self.line.iter().position(|n| *n == id) {
                return Some(i);
            }
            id = game.parent(id)?;
        }
    }

    /// The main-line moves of `color` judged `j`, in order.
    pub fn errors(&self, color: Color, j: Judgement) -> Vec<NodeId> {
        self.report
            .judgements
            .iter()
            .enumerate()
            .filter(|(i, x)| **x == Some(j) && self.movers.get(*i) == Some(&color))
            .map(|(i, _)| self.line[i + 1])
            .collect()
    }

    // ----- cache -----------------------------------------------------------------

    /// The cache file of this main line in `dir`.
    fn cache_path(&self, dir: &Path) -> PathBuf {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for k in &self.keys {
            for b in k.bytes().chain(std::iter::once(b'\n')) {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        dir.join(format!("{h:016x}.json"))
    }

    /// Keeps engine results in `dir` (nothing for evaluations read from the PGN).
    pub fn save_cache(&self, dir: &Path) -> std::io::Result<()> {
        let Some(meta) = self.meta.clone().filter(|m| m.nodes > 0) else {
            return Ok(());
        };
        // A game without moves has nothing to report, and every such game would share
        // this one file.
        if self.evals.is_empty() || self.line.len() < 2 {
            return Ok(());
        }
        let file = CacheFile {
            version: CACHE_VERSION,
            meta,
            evals: self
                .evals
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
        write_atomic(&self.cache_path(dir), &bytes)
    }

    fn load_cache(&mut self, dir: &Path) -> bool {
        let Ok(bytes) = std::fs::read(self.cache_path(dir)) else {
            return false;
        };
        let Ok(file) = serde_json::from_slice::<CacheFile>(&bytes) else {
            return false;
        };
        if file.version != CACHE_VERSION || file.evals.is_empty() {
            return false;
        }
        self.meta = Some(file.meta);
        self.evals = file.evals.into_iter().collect();
        true
    }

    /// Uses `[%eval]` comments (Lichess exports, or our own written analysis) when every
    /// main-line move has one.
    fn load_pgn_evals(&mut self, game: &Game) -> bool {
        let mut found = Vec::new();
        for (i, id) in self.line.iter().enumerate().skip(1) {
            let Some(node) = game.node(*id) else {
                return false;
            };
            let e = node
                .comments_after()
                .iter()
                .flat_map(|c| parse_comment(c))
                .find_map(|p| match p {
                    CommentPart::Annotation(Annotation::Other { name, payload })
                        if name == "eval" =>
                    {
                        Eval::parse_pgn(&payload)
                    }
                    _ => None,
                });
            match e {
                Some(e) => found.push((i, e)),
                None if self.values[i].is_some() => {}
                None => return false,
            }
        }
        let Some(&(_, first)) = found.first() else {
            return false;
        };
        // The start position carries no comment: Lichess assumes +0.15 for the initial one.
        let root = if game.start().is_standard() {
            Eval::Cp(15)
        } else {
            first
        };
        found.insert(0, (0, root));
        for (i, eval) in found {
            self.evals.insert(
                self.keys[i].clone(),
                PosEval {
                    eval,
                    pv: Vec::new(),
                    depth: 0,
                    second: None,
                },
            );
        }
        self.meta = Some(ReviewMeta {
            engine: "PGN".into(),
            nodes: 0,
        });
        true
    }
}

/// Cache key and UCI position of a node (full history, see [`Game::engine_history`]).
fn position_key(game: &Game, id: NodeId) -> Option<(String, EnginePosition)> {
    let h = game.engine_history(id)?;
    let key = format!(
        "{}|{}",
        h.start_fen.as_deref().unwrap_or("startpos"),
        h.moves.join(" ")
    );
    let pos = match h.start_fen {
        None => EnginePosition::startpos(h.moves),
        Some(fen) => EnginePosition::fen(fen, h.moves),
    };
    Some((key, pos))
}

/// Evaluation of a position without legal moves.
fn terminal_eval(pos: &Chess) -> Option<Eval> {
    if pos.is_checkmate() {
        Some(Eval::Mate {
            white: pos.turn() == Color::Black,
            moves: 0,
        })
    } else if pos.is_stalemate() {
        Some(Eval::Cp(0))
    } else {
        None
    }
}

/// The legal beginning of an engine line from `root`: its moves and their SAN.
pub fn parse_pv(root: &Chess, pv: &[String]) -> (Vec<Move>, Vec<String>) {
    let mut pos = root.clone();
    let (mut moves, mut san) = (Vec::new(), Vec::new());
    for u in pv {
        let Some(m) = u
            .parse::<UciMove>()
            .ok()
            .and_then(|um| um.to_move(&pos).ok())
        else {
            break;
        };
        san.push(SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string());
        moves.push(m);
    }
    (moves, san)
}

/// Writes through a temporary file, so a crash never leaves half a cache file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(pgn: &str) -> Game {
        chess_core::parse_str(pgn).game
    }

    #[test]
    fn pgn_evals_give_the_report() {
        let g = game(
            "1. e4 {[%eval 0.3]} e5 {[%eval 0.32]} 2. Qh5 {[%eval -0.2]} Ke7 {[%eval 6.5]} \
             3. Qxe5# 1-0",
        );
        let a = GameAnalysis::new(&g, None);
        assert!(a.has_data());
        assert_eq!(a.missing(), 0);
        assert!(a.report.complete);
        assert_eq!(a.meta.as_ref().map(|m| m.nodes), Some(0));
        let ke7 = g.mainline()[3];
        assert_eq!(
            a.marks.get(&ke7).map(|m| m.judgement),
            Some(Judgement::Blunder)
        );
        assert_eq!(a.errors(Color::Black, Judgement::Blunder), vec![ke7]);
        assert!(a.errors(Color::White, Judgement::Blunder).is_empty());
        // The final mate is evaluated without an engine and not counted for one.
        assert_eq!(a.positions(), a.line().len() - 1);
        // So is a final stalemate.
        let stale = game("[FEN \"7k/8/6K1/8/8/8/8/5Q2 w - - 0 1\"] 1. Qf7 *");
        let s = GameAnalysis::new(&stale, None);
        assert_eq!(s.line().len(), 2);
        assert_eq!(s.positions(), 1);
        assert_eq!(s.values()[1], Some(Eval::Cp(0)));
    }

    #[test]
    fn engine_results_are_cached_per_main_line() {
        let dir = std::env::temp_dir().join(format!("game-review-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let g = game("1. e4 e5 2. Nf3 *");
        let mut a = GameAnalysis::new(&g, Some(&dir));
        assert!(!a.has_data());
        let meta = ReviewMeta {
            engine: "Fake".into(),
            nodes: 1000,
        };
        let targets = a.begin(&g, meta.clone(), false);
        assert_eq!(targets.len(), 4);
        for (i, t) in targets.into_iter().enumerate() {
            a.apply(JobEvent::Eval {
                key: t.key,
                eval: PosEval {
                    eval: Eval::Cp(20 - i as i32 * 300),
                    pv: vec!["e2e4".into()],
                    depth: 10,
                    second: None,
                },
                nps: 0,
            });
        }
        a.refresh(&g);
        assert_eq!(a.missing(), 0);
        a.save_cache(&dir).expect("saved");
        let b = GameAnalysis::new(&g, Some(&dir));
        assert_eq!(b.meta, Some(meta));
        assert_eq!(b.values(), a.values());
        // 1.e4 was the engine's own move: never an error, whatever the evaluations say.
        assert!(!b.marks.contains_key(&g.mainline()[0]));
        // Another main line has its own file.
        assert!(!GameAnalysis::new(&game("1. d4 *"), Some(&dir)).has_data());
        // Filling in keeps the quality of the first run.
        let mut c = GameAnalysis::new(&g, Some(&dir));
        let fast = ReviewMeta {
            engine: "Fake".into(),
            nodes: 250,
        };
        // Nothing left to evaluate; 1.e4, the engine's best, still wants the best other
        // move (an analysis from before !! and !).
        assert_eq!(c.seconds_missing(), 1);
        let left = c.begin(&g, fast.clone(), false);
        assert_eq!(left.len(), 1);
        assert!(left[0].known.is_some() && left[0].played.as_deref() == Some("e2e4"));
        assert!(!left[0].others.is_empty() && !left[0].others.contains(&"e2e4".to_string()));
        assert_eq!(c.meta.as_ref().map(|m| m.nodes), Some(1000));
        c.begin(&g, fast, true);
        assert_eq!(c.meta.as_ref().map(|m| m.nodes), Some(250));
        // A game without moves is never cached.
        let empty = game("*");
        let mut e = GameAnalysis::new(&empty, Some(&dir));
        let t = e.begin(
            &empty,
            ReviewMeta {
                engine: "Fake".into(),
                nodes: 1000,
            },
            false,
        );
        for t in t {
            e.apply(JobEvent::Eval {
                key: t.key,
                eval: PosEval {
                    eval: Eval::Cp(20),
                    pv: vec![],
                    depth: 1,
                    second: None,
                },
                nps: 0,
            });
        }
        e.save_cache(&dir).expect("nothing to save is fine");
        assert!(
            !GameAnalysis::new(
                &game("[FEN \"8/8/8/8/8/8/8/K1k5 w - - 0 1\"] *"),
                Some(&dir)
            )
            .has_data()
        );
        assert!(!GameAnalysis::new(&empty, Some(&dir)).has_data());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
