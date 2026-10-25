//! How strong Maia plays (docs/research/maia-strength.md): the measured tables of the
//! published models, from a strength on the Lichess blitz scale to the rating to give
//! the network and how surely to choose, and the choice of the move itself. The desktop
//! and the Android apps play by these.

use shakmaty::Move;

use crate::{Prediction, pick};

/// The published models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// 5M parameters: plays up to about 1800.
    Small,
    /// 79M parameters: stronger and closer to people, several times slower.
    Large,
}

/// The strongest the small model plays (with the large one missing).
pub const SMALL_MAX: u32 = 1800;

/// Measured (docs/research/maia-strength.md): Maia's rating input → her strength on
/// the Lichess blitz scale, as she plays here (temperature 1, rare moves dropped).
const SMALL: &[(u32, u32)] = &[
    (600, 1043),
    (800, 1154),
    (1100, 1333),
    (1300, 1426),
    (1500, 1485),
    (1700, 1563),
    (1900, 1632),
    (2100, 1683),
    (2300, 1737),
    (2600, 1767),
];
const LARGE: &[(u32, u32)] = &[
    (600, 1078),
    (1100, 1375),
    (1500, 1605),
    (1900, 1794),
    (2300, 1964),
    (2600, 2000),
];

/// Above what a model plays choosing among the likely moves (temperature 1), it is made
/// stronger by choosing more surely: measured strength at its top rating input → the
/// temperature (0: always the most likely move).
const SMALL_SURE: &[(u32, f32)] = &[(1767, 1.0), (1854, 0.5)];
const LARGE_SURE: &[(u32, f32)] = &[(1976, 1.0), (2053, 0.5), (2126, 0.0)];

/// How surely `model` chooses its move to play at `strength` (1 below its top).
pub fn temperature(model: Model, strength: u32) -> f32 {
    let table = match model {
        Model::Small => SMALL_SURE,
        Model::Large => LARGE_SURE,
    };
    let (first, last) = (table[0], table[table.len() - 1]);
    if strength <= first.0 {
        return first.1;
    }
    if strength >= last.0 {
        return last.1;
    }
    let i = table
        .iter()
        .position(|p| p.0 >= strength)
        .unwrap_or(table.len() - 1);
    let ((a_s, a_t), (b_s, b_t)) = (table[i - 1], table[i]);
    let k = (strength - a_s) as f32 / (b_s - a_s) as f32;
    a_t + k * (b_t - a_t)
}

/// The rating to give `model` for it to play at `strength` (between the measured
/// points, in proportion; outside them, the nearest).
pub fn rating_input(model: Model, strength: u32) -> u32 {
    let table = match model {
        Model::Small => SMALL,
        Model::Large => LARGE,
    };
    let (first, last) = (table[0], table[table.len() - 1]);
    if strength <= first.1 {
        return first.0;
    }
    if strength >= last.1 {
        return last.0;
    }
    let i = table
        .iter()
        .position(|p| p.1 >= strength)
        .unwrap_or(table.len() - 1);
    let ((a_in, a_out), (b_in, b_out)) = (table[i - 1], table[i]);
    let t = f64::from(strength - a_out) / f64::from(b_out - a_out);
    (f64::from(a_in) + t * f64::from(b_in - a_in)).round() as u32
}

/// The move to play from a prediction: a move less than a tenth as likely as the most
/// likely one is not played (human mistakes, not accidents: a 2600 does not hang the
/// queen one time in five thousand), the rest drawn at `temperature`. `random` is in
/// [0, 1).
pub fn choose(prediction: &Prediction, temperature: f32, random: f32) -> Option<Move> {
    let top = prediction.moves.first().map_or(0.0, |(_, p)| *p);
    let likely = Prediction {
        moves: prediction
            .moves
            .iter()
            .filter(|(_, p)| *p >= 0.1 * top)
            .cloned()
            .collect(),
        wdl: prediction.wdl,
    };
    pick(&likely, temperature, random)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_turns_into_the_rating_to_give() {
        // The measured points themselves, in proportion between them, the ends held.
        assert_eq!(rating_input(Model::Large, 1605), 1500);
        assert_eq!(rating_input(Model::Large, 2000), 2600);
        assert_eq!(rating_input(Model::Small, 1485), 1500);
        assert_eq!(rating_input(Model::Small, SMALL_MAX), 2600);
        assert_eq!(rating_input(Model::Small, 900), 600);
        // Above the top input: surer choices, down to the most likely move.
        assert_eq!(temperature(Model::Large, 1900), 1.0);
        assert!((temperature(Model::Large, 2053) - 0.5).abs() < 1e-6);
        assert_eq!(temperature(Model::Large, 2200), 0.0);
        assert!(temperature(Model::Small, SMALL_MAX) < 1.0);
    }

    #[test]
    fn rare_moves_are_not_chosen() {
        use shakmaty::{Chess, Position};
        let pos = Chess::default();
        let moves = pos.legal_moves();
        let p = Prediction {
            moves: vec![(moves[0], 0.9), (moves[1], 0.05)],
            wdl: [0.0; 3],
        };
        // Whatever the draw: under a tenth of the best is never played.
        for r in [0.0, 0.5, 0.99] {
            assert_eq!(choose(&p, 5.0, r), Some(moves[0]));
        }
    }
}
