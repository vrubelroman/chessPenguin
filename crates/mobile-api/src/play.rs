//! A game against Maia on the phone (docs/design/android-play.md §31): the small model
//! (5M) chooses her moves at a strength on the Lichess blitz scale, by the same tables
//! and rule as on the desktop (`maia::strength`). No clocks on the phone.

use std::sync::Arc;

use maia::strength::{Model, choose, rating_input, temperature};
use shakmaty::{CastlingMode, Chess};

use crate::ApiError;
use crate::session::GameSession;

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
