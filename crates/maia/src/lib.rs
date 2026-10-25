//! A human-like move predictor: the Maia-3 network (CSSLab, University of Toronto,
//! "Chessformer", ICLR 2026) computed in plain Rust, without a machine-learning runtime.
//!
//! The network sees the position (and up to seven positions before it) from the side to
//! move and the ratings of both players, and gives the probability of each legal move
//! as a player of that rating would play it, plus the chances of win, draw and loss.
//!
//! The weights come from a file in our own simple format (`MAIA3W02`, written by
//! `scripts/maia-convert.py` from the published PyTorch checkpoint): halves (f16),
//! read once into f32.

mod net;
mod weights;

pub use net::Prediction;
pub use weights::WeightsError;

use shakmaty::{Chess, Move, Position};

/// The network, loaded.
pub struct Maia {
    net: net::Net,
}

impl Maia {
    /// Reads the weights file.
    pub fn load(path: &std::path::Path) -> Result<Maia, WeightsError> {
        let bytes = std::fs::read(path).map_err(WeightsError::Io)?;
        Maia::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Maia, WeightsError> {
        let w = weights::Weights::parse(bytes)?;
        Ok(Maia {
            net: net::Net::new(&w)?,
        })
    }

    /// The moves a player rated `self_elo` would play in `history.last()` against one
    /// rated `oppo_elo`, most likely first. `history`: the positions of the game up to
    /// and including the current one (the last eight are used; fewer are padded with
    /// the earliest given). Empty when there is no legal move.
    pub fn predict(&self, history: &[Chess], self_elo: u32, oppo_elo: u32) -> Prediction {
        self.net.predict(history, self_elo, oppo_elo)
    }
}

/// A move to play from a prediction: the most likely one with `temperature` 0,
/// otherwise drawn at random with probabilities sharpened (< 1) or flattened (> 1).
/// `random` is a number in [0, 1).
pub fn pick(prediction: &Prediction, temperature: f32, random: f32) -> Option<Move> {
    let moves = &prediction.moves;
    if moves.is_empty() {
        return None;
    }
    if temperature <= 0.0 {
        return Some(moves[0].0);
    }
    let weights: Vec<f32> = moves
        .iter()
        .map(|(_, p)| p.max(1e-12).powf(1.0 / temperature))
        .collect();
    let total: f32 = weights.iter().sum();
    let mut target = random.clamp(0.0, 0.999_999) * total;
    for ((m, _), w) in moves.iter().zip(&weights) {
        if target < *w {
            return Some(*m);
        }
        target -= w;
    }
    moves.last().map(|(m, _)| *m)
}

/// The position after `m`, for building the history.
pub fn after(pos: &Chess, m: Move) -> Chess {
    let mut p = pos.clone();
    p.play_unchecked(m);
    p
}
