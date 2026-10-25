//! A game against Maia on the phone (docs/design/android-play.md §31): the small model
//! (5M) chooses her moves at a strength on the Lichess blitz scale, by the same tables
//! and rule as on the desktop (`maia::strength`). No clocks on the phone.

use std::sync::Arc;

use maia::strength::{Model, choose, rating_input, temperature};
use shakmaty::{CastlingMode, Chess};

use std::sync::Mutex;

use crate::ApiError;
use crate::session::GameSession;

/// How a game stands at a position: over or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum GameEnding {
    /// The side named won by mate.
    Checkmate {
        white_won: bool,
    },
    Stalemate,
    InsufficientMaterial,
    Threefold,
    FiftyMoves,
}

#[uniffi::export]
impl GameSession {
    /// How the game stands at the move at place `at` (its line from the start counts
    /// for repetitions); none while it goes on.
    pub fn ending(&self, at: u32) -> Option<GameEnding> {
        let line = self.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            Some(
                g.path(id)
                    .iter()
                    .filter_map(|n| g.position(*n).cloned())
                    .collect::<Vec<Chess>>(),
            )
        })?;
        Some(match ::play::ending(&line)? {
            ::play::Ending::Checkmate { winner } => GameEnding::Checkmate {
                white_won: winner == shakmaty::Color::White,
            },
            ::play::Ending::Stalemate => GameEnding::Stalemate,
            ::play::Ending::InsufficientMaterial => GameEnding::InsufficientMaterial,
            ::play::Ending::Threefold => GameEnding::Threefold,
            ::play::Ending::FiftyMoves => GameEnding::FiftyMoves,
            // No clocks and no resignations here: the app knows those itself.
            ::play::Ending::Time { .. }
            | ::play::Ending::Resigned { .. }
            | ::play::Ending::DrawAgreed => return None,
        })
    }
}

/// How long Maia "thinks" before her move `ply` of an untimed game (milliseconds):
/// as a person would, quicker with one legal move.
#[uniffi::export]
pub fn maia_pause_ms(ply: u32, only_move: bool) -> u64 {
    ::play::pause(None, ply as usize, only_move).as_millis() as u64
}

/// The tags of a game against Maia on the phone (no clocks): `event` in the app's
/// language; the user plays White when `user_white`.
#[uniffi::export]
pub fn maia_game_tags(
    event: String,
    user_white: bool,
    user_name: String,
    strength: u32,
) -> Vec<crate::Tag> {
    let user = if user_white {
        shakmaty::Color::White
    } else {
        shakmaty::Color::Black
    };
    ::play::game_tags(&event, user, &user_name, None, strength, "-")
        .into_iter()
        .map(|(name, value)| crate::Tag { name, value })
        .collect()
}

/// Maia's turns in a row with her position hopeless before she offers to resign.
#[uniffi::export]
pub fn hopeless_turns() -> u32 {
    ::play::HOPELESS_TURNS
}

/// Stockfish judging the positions where Maia is to move (unseen by the user), for her
/// resignation; one for the whole game.
#[derive(uniffi::Object)]
pub struct ResignJudge {
    judge: Mutex<::play::Judge>,
}

#[uniffi::export]
impl ResignJudge {
    /// The engine at `path` with its network `eval_file`.
    #[uniffi::constructor]
    pub fn start(path: String, eval_file: String) -> Arc<ResignJudge> {
        let config = uci::EngineConfig {
            nice: 10,
            ..uci::EngineConfig::new(path)
        };
        let options = if eval_file.is_empty() {
            Vec::new()
        } else {
            vec![("EvalFile".to_owned(), eval_file)]
        };
        Arc::new(ResignJudge {
            judge: Mutex::new(::play::Judge::start(config, options, Box::new(|| {}))),
        })
    }

    /// Judges the position at place `at` of the game (Maia to move there).
    pub fn ask(&self, game: Arc<GameSession>, at: u32) {
        let pos = game.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            g.engine_history(id)
        });
        if let Some(h) = pos {
            let p = match h.start_fen {
                None => uci::EnginePosition::startpos(h.moves),
                Some(fen) => uci::EnginePosition::fen(fen, h.moves),
            };
            self.judge.lock().unwrap_or_else(|e| e.into_inner()).ask(p);
        }
    }

    /// Whether the position asked last is hopeless for Maia, once the answer came.
    pub fn hopeless(&self) -> Option<bool> {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .answer()
            .map(::play::hopeless)
    }

    /// Drops the request in flight (its position is gone).
    pub fn forget(&self) {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget();
    }

    /// Moves were taken back: the scores of Maia's turns gone are forgotten.
    pub fn forget_scores(&self) {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget_scores();
    }

    /// A draw may be offered now (not right after a refusal).
    pub fn may_offer_draw(&self) -> bool {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .may_offer_draw()
    }

    /// The user offers a draw: whether Maia accepts (the shared rule, `play::accepts_draw`).
    pub fn offer_draw(&self) -> bool {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .offer_draw()
    }
}

/// The weakest and strongest Maia on the phone (the small model's measured range).
pub const PHONE_MIN: u32 = 1100;
pub const PHONE_MAX: u32 = maia::strength::SMALL_MAX;

/// The strengths the phone offers: (weakest, strongest).
#[uniffi::export]
pub fn maia_range() -> Vec<u32> {
    vec![PHONE_MIN, PHONE_MAX]
}

/// Maia, loaded once for a game.
#[derive(uniffi::Object)]
pub struct MaiaPlayer {
    maia: maia::Maia,
}

#[uniffi::export]
impl MaiaPlayer {
    /// Reads the weights (`maia3-5m.pt`).
    #[uniffi::constructor]
    pub fn load(path: String) -> Result<Arc<MaiaPlayer>, ApiError> {
        let maia =
            maia::Maia::load(std::path::Path::new(&path)).map_err(|e| ApiError::Corrupt {
                detail: e.to_string(),
            })?;
        Ok(Arc::new(MaiaPlayer { maia }))
    }

    /// Maia's move (UCI) in the position at place `at` of the game, playing at
    /// `strength` against a player rated `opponent` (her own strength when unknown).
    /// `random` is in [0, 1). None when the game is over there.
    pub fn choose(
        &self,
        game: Arc<GameSession>,
        at: u32,
        strength: u32,
        opponent: Option<u32>,
        random: f32,
    ) -> Option<String> {
        let history: Vec<Chess> = game.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            Some(
                g.path(id)
                    .iter()
                    .filter_map(|n| g.position(*n).cloned())
                    .collect(),
            )
        })?;
        let strength = strength.clamp(PHONE_MIN, PHONE_MAX);
        let p = self.maia.predict(
            &history,
            rating_input(Model::Small, strength),
            opponent.unwrap_or(strength),
        );
        choose(&p, temperature(Model::Small, strength), random)
            .map(|m| m.to_uci(CastlingMode::Standard).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the weights: `MAIA_WEIGHTS=/path/maia3-5m.pt cargo test -p mobile-api -- --ignored`.
    #[test]
    #[ignore = "needs the weights file (MAIA_WEIGHTS)"]
    fn maia_answers_with_a_legal_move() {
        let path = std::env::var("MAIA_WEIGHTS").expect("MAIA_WEIGHTS");
        let m = MaiaPlayer::load(path).unwrap();
        let s = GameSession::from_pgn("1. e4 *".into()).unwrap();
        let e4 = s.data().nodes.iter().find(|n| n.san == "e4").unwrap().id;
        let uci = m.choose(s.clone(), e4, 1500, None, 0.3).unwrap();
        assert!(s.legal_moves(e4).contains(&uci), "{uci}");
        let mated = GameSession::from_pgn("1. f3 e5 2. g4 Qh4# *".into()).unwrap();
        let end_id = mated
            .data()
            .nodes
            .iter()
            .find(|n| n.san == "Qh4#")
            .unwrap()
            .id;
        assert_eq!(
            mated.ending(end_id),
            Some(GameEnding::Checkmate { white_won: false })
        );
        assert_eq!(mated.ending(0), None);
        let end = mated
            .data()
            .nodes
            .iter()
            .find(|n| n.san == "Qh4#")
            .unwrap()
            .id;
        assert!(m.choose(mated, end, 1500, None, 0.3).is_none());
    }
}
