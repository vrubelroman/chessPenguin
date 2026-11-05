//! Opening errors found by the engine: every distinct position of the first plies of
//! the player's games is evaluated once (the games repeat their openings, so about
//! half the positions are shared), and the player's moves are judged from the
//! evaluations before and after them with the rules of the game analysis. The same
//! error in several games — the same position and the same move — is what matters
//! most for preparation, so those are listed on their own.
//!
//! Evaluations are kept per position in a cache file shared by every report: another
//! player of the same database, or the same player later, is judged from the cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chess_core::shakmaty::san::SanPlus;
use chess_core::shakmaty::uci::UciMove;
use chess_core::shakmaty::{Chess, Position};
use game_review::{Eval, Judgement, PosEval, SEARCH_VERSION};
use serde::{Deserialize, Serialize};

use crate::games::{GameInfo, Tally};

/// Nodes of an evaluation that needs no engine (checkmate, stalemate): better than any
/// search.
pub const EXACT: u64 = u64::MAX;

/// The engine's result for one position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionEval {
    /// Nodes of the search ([`EXACT`] for a position without legal moves).
    pub nodes: u64,
    pub eval: Eval,
    /// The engine's line in UCI; the first move is its best move.
    pub pv: Vec<String>,
    pub depth: u32,
}

impl PositionEval {
    pub fn from_engine(nodes: u64, e: &PosEval) -> PositionEval {
        PositionEval {
            nodes,
            eval: e.eval,
            pv: e.pv.iter().take(8).cloned().collect(),
            depth: e.depth,
        }
    }

    pub fn best(&self) -> Option<&str> {
        self.pv.first().map(String::as_str)
    }
}

/// How many inaccuracies, mistakes and blunders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorCounts {
    pub inaccuracies: u32,
    pub mistakes: u32,
    pub blunders: u32,
}

impl ErrorCounts {
    pub fn add(&mut self, j: Judgement) {
        match j {
            Judgement::Inaccuracy => self.inaccuracies += 1,
            Judgement::Mistake => self.mistakes += 1,
            Judgement::Blunder => self.blunders += 1,
            Judgement::Great | Judgement::Brilliant => {}
        }
    }

    pub fn total(&self) -> u32 {
        self.inaccuracies + self.mistakes + self.blunders
    }
}

/// A move of the player judged an inaccuracy, a mistake or a blunder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpeningError {
    /// Index into [`crate::PrepReport::games`].
    pub game: u32,
    /// The move's place in the line (0-based; see [`GameInfo::move_label`]).
    pub ply: u32,
    /// The position before the move.
    pub key: u64,
    pub uci: String,
    pub san: String,
    /// The engine's move instead.
    pub best_uci: String,
    pub best_san: String,
    pub judgement: Judgement,
    pub before: Eval,
    pub after: Eval,
    /// The game's opening ([`crate::PrepReport::openings`]).
    pub opening: Option<usize>,
}

/// The same error in several games: the same position and the same move.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepeatedError {
    pub key: u64,
    pub ply: u32,
    pub uci: String,
    pub san: String,
    pub best_san: String,
    pub judgement: Judgement,
    /// The games (indices into [`crate::PrepReport::games`]), in order.
    pub games: Vec<u32>,
    /// How those games ended.
    pub tally: Tally,
    /// The opening of the first of them.
    pub opening: Option<usize>,
}

impl RepeatedError {
    pub fn times(&self) -> u32 {
        self.games.len() as u32
    }
}

/// Judges the player's moves in `games[g]` for `g` in `which` from `evals`: the
/// move is not the engine's best, and the evaluations before and after it say it
/// lost enough (the rules of the game analysis, [`game_review::judge`]). Only the
/// prefix before the opponent's first inaccuracy/mistake/blunder is eligible.
/// A missing opponent evaluation or best move stops the prefix too: later player
/// errors are not published until those preceding moves have been checked.
pub fn judge_games(
    games: &[GameInfo],
    which: &[u32],
    evals: &HashMap<u64, PositionEval>,
) -> Vec<OpeningError> {
    let mut out = Vec::new();
    for &g in which {
        let game = &games[g as usize];
        let mut pos = Chess::default();
        for (i, uci) in game.ucis.iter().enumerate() {
            let Some(m) = uci
                .parse::<UciMove>()
                .ok()
                .and_then(|u| u.to_move(&pos).ok())
            else {
                break;
            };
            let evaluated = evals
                .get(&game.keys[i])
                .and_then(|before| Some((before, evals.get(&game.keys[i + 1])?, before.best()?)));
            if !game.player_moves_at(i) && evaluated.is_none() {
                break;
            }
            if let Some((before, after, best)) = evaluated
                && best != uci
                && let Some(judgement) = game_review::judge(before.eval, after.eval, pos.turn())
            {
                if !game.player_moves_at(i) {
                    break;
                }
                let best_san = best
                    .parse::<UciMove>()
                    .ok()
                    .and_then(|u| u.to_move(&pos).ok())
                    .map(|bm| SanPlus::from_move(pos.clone(), bm).to_string())
                    .unwrap_or_else(|| best.to_owned());
                out.push(OpeningError {
                    game: g,
                    ply: i as u32,
                    key: game.keys[i],
                    uci: uci.clone(),
                    san: game.sans.get(i).cloned().unwrap_or_default(),
                    best_uci: best.to_owned(),
                    best_san,
                    judgement,
                    before: before.eval,
                    after: after.eval,
                    opening: game.opening,
                });
            }
            pos.play_unchecked(m);
        }
    }
    out
}

/// The errors made in two or more games (same position, same move), most repeated
/// first, then the worst first.
pub fn repeated(errors: &[OpeningError], games: &[GameInfo]) -> Vec<RepeatedError> {
    let mut groups: HashMap<(u64, &str), Vec<&OpeningError>> = HashMap::new();
    for e in errors {
        groups.entry((e.key, e.uci.as_str())).or_default().push(e);
    }
    let mut out: Vec<RepeatedError> = groups
        .into_values()
        .filter(|g| g.len() >= 2)
        .map(|g| {
            let first = g[0];
            let mut tally = Tally::default();
            let mut list: Vec<u32> = g.iter().map(|e| e.game).collect();
            list.sort_unstable();
            list.dedup();
            for &game in &list {
                tally.add(games[game as usize].outcome);
            }
            RepeatedError {
                key: first.key,
                ply: first.ply,
                uci: first.uci.clone(),
                san: first.san.clone(),
                best_san: first.best_san.clone(),
                judgement: first.judgement,
                games: list,
                tally,
                opening: first.opening,
            }
        })
        .filter(|r| r.games.len() >= 2)
        .collect();
    out.sort_by(|a, b| {
        b.games
            .len()
            .cmp(&a.games.len())
            .then(b.judgement.cmp(&a.judgement))
            .then(a.ply.cmp(&b.ply))
            .then(a.games.cmp(&b.games))
    });
    out
}

// ----- the cache of evaluations by position --------------------------------------

const CACHE_FILE: &str = "positions.json";
const CACHE_VERSION: u32 = 1;
/// Positions kept at most (about 100 bytes each on disk).
const CACHE_LIMIT: usize = 1_000_000;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    /// The version of the fixed search the evaluations come from ([`SEARCH_VERSION`];
    /// 0: from before it). Those of another one are not used.
    #[serde(default)]
    search: u32,
    positions: Vec<(u64, PositionEval)>,
}

/// Engine evaluations by position, shared by every report: the best quality (most
/// nodes) seen for each position is kept.
#[derive(Default)]
pub struct PositionCache {
    entries: HashMap<u64, PositionEval>,
    dirty: bool,
}

impl PositionCache {
    fn path(dir: &Path) -> PathBuf {
        dir.join(CACHE_FILE)
    }

    /// The cache in `dir` (empty when there is none, or it cannot be read).
    pub fn load(dir: &Path) -> PositionCache {
        let entries = std::fs::read(Self::path(dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|f| f.version == CACHE_VERSION && f.search == SEARCH_VERSION)
            .map(|f| f.positions.into_iter().collect())
            .unwrap_or_default();
        PositionCache {
            entries,
            dirty: false,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: u64) -> Option<&PositionEval> {
        self.entries.get(&key)
    }

    /// Keeps `eval` unless the cache has one with more nodes; says whether it was kept.
    pub fn insert(&mut self, key: u64, eval: PositionEval) -> bool {
        if self
            .entries
            .get(&key)
            .is_some_and(|e| e.nodes >= eval.nodes)
        {
            return false;
        }
        self.entries.insert(key, eval);
        self.dirty = true;
        true
    }

    /// Something was added since the load or the last save.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Writes the cache to `dir` (through a temporary file, so a crash never leaves
    /// half a file); nothing when nothing changed.
    pub fn save(&mut self, dir: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut positions: Vec<(u64, PositionEval)> =
            self.entries.iter().map(|(k, v)| (*k, v.clone())).collect();
        if positions.len() > CACHE_LIMIT {
            // Over the limit the searches with the fewest nodes go first.
            positions.sort_by_key(|a| std::cmp::Reverse(a.1.nodes));
            positions.truncate(CACHE_LIMIT);
        }
        let file = CacheFile {
            version: CACHE_VERSION,
            search: SEARCH_VERSION,
            positions,
        };
        let bytes = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
        write_atomic(&Self::path(dir), &bytes)?;
        self.dirty = false;
        Ok(())
    }
}

/// Writes through a temporary file and a rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
    use crate::tree::tests::game;

    fn eval(nodes: u64, cp: i32, best: &str) -> PositionEval {
        PositionEval {
            nodes,
            eval: Eval::Cp(cp),
            pv: vec![best.to_owned()],
            depth: 10,
        }
    }

    #[test]
    fn the_same_error_in_two_games_is_repeated() {
        // Three games of the player with White: 1.e4 e5 2.Qh5?! twice, 2.Nf3 once.
        let games = vec![
            game(0, true, &["e4", "e5", "Qh5", "Nc6"], "1-0", "", 0),
            game(1, true, &["e4", "e5", "Qh5", "Nf6"], "0-1", "", 0),
            game(2, true, &["e4", "e5", "Nf3", "Nc6"], "1/2-1/2", "", 0),
            game(3, false, &["e4", "e5", "Qh5", "Nc6"], "0-1", "", 0),
        ];
        let mut evals = HashMap::new();
        evals.insert(games[0].keys[0], eval(1000, 30, "e2e4"));
        evals.insert(games[0].keys[1], eval(1000, 30, "e7e5"));
        // 2.Qh5 drops from +0.3 to −1.0: a mistake; 2.Nf3 is the engine's move.
        evals.insert(games[0].keys[2], eval(1000, 30, "g1f3"));
        evals.insert(games[0].keys[3], eval(1000, -100, "b8c6"));
        evals.insert(games[1].keys[3], eval(1000, -100, "g8f6"));
        evals.insert(games[2].keys[3], eval(1000, 30, "b8c6"));
        let errors = judge_games(&games, &[0, 1, 2], &evals);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].game, 0);
        assert_eq!(errors[0].ply, 2);
        assert_eq!(errors[0].san, "Qh5");
        assert_eq!(errors[0].best_san, "Nf3");
        assert_eq!(errors[0].judgement, Judgement::Mistake);
        assert_eq!(errors[1].game, 1);
        let rep = repeated(&errors, &games);
        assert_eq!(rep.len(), 1);
        assert_eq!(rep[0].games, vec![0, 1]);
        assert_eq!(rep[0].times(), 2);
        assert_eq!((rep[0].tally.wins, rep[0].tally.losses), (1, 1));
        // Black's moves in game 3 are the player's: 2.Qh5 is the opponent's there.
        let evals_black = evals.clone();
        assert!(judge_games(&games, &[3], &evals_black).is_empty());
        // A position without an evaluation after the move is not judged (the two
        // games share the position after 2.Qh5).
        let mut partial = evals.clone();
        partial.remove(&games[1].keys[3]);
        assert!(judge_games(&games, &[0, 1, 2], &partial).is_empty());
        // One game less: nothing repeats.
        assert!(repeated(&judge_games(&games, &[0, 2], &evals), &games).is_empty());
    }

    fn line_evals(game: &GameInfo, values: &[i32]) -> HashMap<u64, PositionEval> {
        game.keys
            .iter()
            .enumerate()
            .map(|(ply, &key)| {
                (
                    key,
                    eval(
                        1000,
                        values[ply],
                        game.ucis.get(ply).map_or("", String::as_str),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn every_opponent_error_category_stops_later_errors_for_both_colours() {
        for white in [true, false] {
            for (loss, category) in [
                (60, Judgement::Inaccuracy),
                (150, Judgement::Mistake),
                (500, Judgement::Blunder),
            ] {
                let game = game(0, white, &["e4", "e5", "Qh5", "Nc6"], "*", "", 0);
                let (opponent_ply, player_ply, values) = if white {
                    (1, 2, vec![0, 0, loss, -150, -150])
                } else {
                    (0, 1, vec![0, -loss, 150, 150, 150])
                };
                let mut evals = line_evals(&game, &values);
                evals.get_mut(&game.keys[opponent_ply]).unwrap().pv =
                    vec![if white { "c7c5" } else { "d2d4" }.to_owned()];
                evals.get_mut(&game.keys[player_ply]).unwrap().pv =
                    vec![if white { "g1f3" } else { "c7c5" }.to_owned()];
                let mover = if white {
                    chess_core::shakmaty::Color::Black
                } else {
                    chess_core::shakmaty::Color::White
                };
                assert_eq!(
                    game_review::judge(
                        Eval::Cp(values[opponent_ply]),
                        Eval::Cp(values[opponent_ply + 1]),
                        mover
                    ),
                    Some(category)
                );
                assert!(judge_games(std::slice::from_ref(&game), &[0], &evals).is_empty());
                // The engine's own best move is exempt even if evaluations differ.
                evals.get_mut(&game.keys[opponent_ply]).unwrap().pv =
                    vec![game.ucis[opponent_ply].clone()];
                let errors = judge_games(std::slice::from_ref(&game), &[0], &evals);
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].ply, player_ply as u32);
            }
        }
    }

    #[test]
    fn earlier_player_errors_survive_but_the_whole_later_suffix_is_excluded() {
        for white in [true, false] {
            let game = game(
                0,
                white,
                &["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6"],
                "*",
                "",
                0,
            );
            let values = if white {
                vec![0, -150, 0, -150, -150, -300, -300]
            } else {
                vec![0, 0, 150, 0, 150, 0, 150]
            };
            let mut evals = line_evals(&game, &values);
            let first_player = usize::from(!white);
            let opponent = first_player + 1;
            let alternatives = if white {
                ["d2d4", "c7c5", "g1f3", "d2d3"]
            } else {
                ["c7c5", "g1f3", "g8f6", "d7d6"]
            };
            for (ply, best) in [first_player, opponent, first_player + 2, first_player + 4]
                .into_iter()
                .zip(alternatives)
            {
                evals.get_mut(&game.keys[ply]).unwrap().pv = vec![best.to_owned()];
            }
            let errors = judge_games(std::slice::from_ref(&game), &[0], &evals);
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].ply, first_player as u32);
        }
    }

    #[test]
    fn incomplete_opponent_prefix_withholds_errors_until_checked() {
        let game = game(0, true, &["e4", "e5", "Qh5", "Nc6"], "*", "", 0);
        let mut complete = line_evals(&game, &[0, 0, 0, -150, -150]);
        complete.get_mut(&game.keys[2]).unwrap().pv = vec!["g1f3".to_owned()];
        for missing in [game.keys[1], game.keys[2]] {
            let mut partial = complete.clone();
            partial.remove(&missing);
            assert!(judge_games(std::slice::from_ref(&game), &[0], &partial).is_empty());
        }
        let mut missing_best = complete.clone();
        missing_best.get_mut(&game.keys[1]).unwrap().pv.clear();
        assert!(judge_games(std::slice::from_ref(&game), &[0], &missing_best).is_empty());
        assert_eq!(
            judge_games(std::slice::from_ref(&game), &[0], &complete).len(),
            1
        );
    }

    #[test]
    fn opponent_allowing_mate_is_judged_from_the_actual_mover() {
        for white in [true, false] {
            let game = game(0, white, &["e4", "e5", "Qh5", "Nc6"], "*", "", 0);
            let opponent = usize::from(white);
            let mut evals = line_evals(&game, &[0, 0, 0, 0, 0]);
            evals.get_mut(&game.keys[opponent]).unwrap().pv =
                vec![if white { "c7c5" } else { "d2d4" }.to_owned()];
            evals.get_mut(&game.keys[opponent + 1]).unwrap().eval = Eval::Mate { white, moves: 3 };
            evals.get_mut(&game.keys[opponent + 1]).unwrap().pv =
                vec![if white { "g1f3" } else { "c7c5" }.to_owned()];
            // The player's next move lets that forced mate go, normally an error.
            assert!(judge_games(std::slice::from_ref(&game), &[0], &evals).is_empty());
        }
    }

    #[test]
    fn the_position_cache_keeps_the_best_quality_and_survives_a_save() {
        let dir = std::env::temp_dir().join(format!("prep-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = PositionCache::load(&dir);
        assert!(c.is_empty());
        assert!(c.insert(1, eval(1000, 10, "e2e4")));
        assert!(!c.insert(1, eval(250, 20, "d2d4")), "fewer nodes: not kept");
        assert!(c.insert(1, eval(3000, 30, "c2c4")));
        assert!(c.insert(2, eval(EXACT, 0, "")));
        assert_eq!(c.get(1).map(|e| e.nodes), Some(3000));
        assert!(c.dirty());
        c.save(&dir).expect("saved");
        assert!(!c.dirty());
        let d = PositionCache::load(&dir);
        assert_eq!(d.len(), 2);
        assert_eq!(d.get(1), c.get(1));
        assert_eq!(d.get(2).map(|e| e.nodes), Some(EXACT));
        assert_eq!(d.get(3), None);
        // A cache from before the fixed search of the game analysis is not used.
        let path = PositionCache::path(&dir);
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("cache")).expect("json");
        json.as_object_mut().expect("cache").remove("search");
        std::fs::write(&path, serde_json::to_vec(&json).expect("json")).expect("written");
        assert!(PositionCache::load(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
