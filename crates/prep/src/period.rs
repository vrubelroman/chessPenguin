//! Which of the player's games a report takes: all, those of the last months (counted
//! back from his newest game, so an old database is not left empty), or his last games
//! of each colour. Games without a date are taken by «all» only; by number they count
//! as the oldest, in the order of the database (so a database without dates gives its
//! last records).

use serde::{Deserialize, Serialize};

use chess_import::chessbase::base::Side;

/// The games a report takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Period {
    #[default]
    All,
    /// Played within this many months before the player's newest game.
    Months(u16),
    /// The newest this many games of each colour.
    Last(u32),
}

/// One game of the player before the choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolGame {
    /// Record index in the database.
    pub index: u32,
    pub white: bool,
    /// `yyyymmdd` (month or day 0 when unknown); 0: no date.
    pub date: u32,
}

/// What a period takes of the games of the colour(s) looked at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chosen {
    pub games: usize,
    /// Of all the games: undated ones a period by months leaves out.
    pub undated_out: usize,
}

/// The newest date of the games (0: none dated).
pub fn newest(pool: &[PoolGame]) -> u32 {
    pool.iter().map(|g| g.date).max().unwrap_or(0)
}

/// The first date within `months` before `newest` (`yyyymmdd`).
fn cutoff(newest: u32, months: u16) -> u32 {
    let (y, month, d) = (newest / 10000, newest / 100 % 100, newest % 100);
    let total = (y * 12 + month.max(1) - 1).saturating_sub(u32::from(months));
    let (y, m) = (total / 12, total % 12 + 1);
    // A newest date without a month: the whole year it was in.
    let (m, d) = if month == 0 { (0, 0) } else { (m, d) };
    y * 10000 + m * 100 + d
}

/// The games `period` takes: (record, the player had White), in record order.
pub fn choose(pool: &[PoolGame], period: Period) -> Vec<(u32, bool)> {
    let mut out: Vec<(u32, bool)> = match period {
        Period::All => pool.iter().map(|g| (g.index, g.white)).collect(),
        Period::Months(months) => {
            let newest = newest(pool);
            if newest == 0 {
                return Vec::new();
            }
            let from = cutoff(newest, months);
            pool.iter()
                .filter(|g| g.date != 0 && g.date >= from)
                .map(|g| (g.index, g.white))
                .collect()
        }
        Period::Last(n) => {
            let mut out = Vec::new();
            for white in [true, false] {
                let mut games: Vec<&PoolGame> = pool.iter().filter(|g| g.white == white).collect();
                // Newest first; the undated (0) last, later records counted newer.
                games.sort_by(|a, b| b.date.cmp(&a.date).then(b.index.cmp(&a.index)));
                out.extend(games.iter().take(n as usize).map(|g| (g.index, g.white)));
            }
            out
        }
    };
    out.sort_unstable();
    out
}

/// How many games of `side` `period` takes, and the undated ones it leaves out.
pub fn count(pool: &[PoolGame], period: Period, side: Side) -> Chosen {
    let of_side = |white: bool| match side {
        Side::Any => true,
        Side::White => white,
        Side::Black => !white,
    };
    let games = choose(pool, period)
        .iter()
        .filter(|(_, w)| of_side(*w))
        .count();
    let undated_out = match period {
        Period::Months(_) => pool
            .iter()
            .filter(|g| g.date == 0 && of_side(g.white))
            .count(),
        _ => 0,
    };
    Chosen { games, undated_out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(index: u32, white: bool, date: u32) -> PoolGame {
        PoolGame { index, white, date }
    }

    #[test]
    fn months_count_back_from_the_newest_game() {
        let pool = [
            g(0, true, 20200105),
            g(1, false, 20240301),
            g(2, true, 20250228),
            g(3, true, 20250301),
            g(4, false, 0),
            g(5, true, 20260301),
        ];
        assert_eq!(cutoff(20260301, 12), 20250301);
        assert_eq!(cutoff(20260115, 1), 20251215);
        assert_eq!(cutoff(20260000, 24), 20240000);
        assert_eq!(
            choose(&pool, Period::Months(12)),
            vec![(3, true), (5, true)]
        );
        assert_eq!(
            choose(&pool, Period::Months(24)),
            vec![(1, false), (2, true), (3, true), (5, true)]
        );
        assert_eq!(choose(&pool, Period::All).len(), 6);
        let c = count(&pool, Period::Months(24), Side::Black);
        assert_eq!((c.games, c.undated_out), (1, 1));
        assert_eq!(count(&pool, Period::All, Side::Any).undated_out, 0);
        // Nothing dated: a period takes nothing.
        assert!(choose(&[g(0, true, 0)], Period::Months(12)).is_empty());
    }

    #[test]
    fn the_last_games_of_each_colour() {
        let pool = [
            g(0, true, 20200101),
            g(1, true, 0),
            g(2, true, 20230101),
            g(3, false, 20240101),
            g(4, true, 20220101),
            g(5, false, 20190101),
        ];
        assert_eq!(
            choose(&pool, Period::Last(2)),
            vec![(2, true), (3, false), (4, true), (5, false)]
        );
        // Without dates: the last records.
        let undated = [g(0, true, 0), g(1, true, 0), g(2, true, 0)];
        assert_eq!(
            choose(&undated, Period::Last(2)),
            vec![(1, true), (2, true)]
        );
        assert_eq!(count(&pool, Period::Last(1), Side::Any).games, 2);
    }
}
