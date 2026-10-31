//! Which of the player's games a report takes: all, those of the last months (counted
//! back from his newest game, so an old database is not left empty), or his last games
//! of each colour. Games without a date are taken by «all» only; by number they count
//! as the oldest, in the order of the database (so a database without dates gives its
//! last records). When no game has a date, a period by months takes them all: it
//! cannot choose, and an empty report would explain nothing.

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

/// The latest day a PGN date can mean: «2026.??.??» → 2026.12.31, «2026.02.??» →
/// 2026.02.31 (only compared, never shown).
fn latest(date: u32) -> u32 {
    match (date / 100 % 100, date % 100) {
        (0, _) => date / 10000 * 10000 + 1231,
        (_, 0) => date + 31,
        _ => date,
    }
}

/// Today as `yyyymmdd` (UTC).
fn today() -> u32 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Days since 1970 to the civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y * 10000 + m * 100 + d) as u32
}

/// The date of the player's newest game as written (0: none dated). A date after
/// `today` is a typo («2206») and does not count as the newest.
fn newest_at(pool: &[PoolGame], today: u32) -> u32 {
    pool.iter()
        .filter(|g| g.date != 0 && g.date / 10000 * 10000 <= today)
        .max_by_key(|g| (latest(g.date).min(today), g.date))
        .map_or(0, |g| g.date)
}

/// The date of the player's newest game as written (0: none dated).
pub fn newest(pool: &[PoolGame]) -> u32 {
    newest_at(pool, today())
}

/// The first day within `months` before the day `end` (`yyyymmdd`).
fn cutoff(end: u32, months: u16) -> u32 {
    let (y, m, d) = (end / 10000, end / 100 % 100, end % 100);
    let total = (y * 12 + m - 1).saturating_sub(u32::from(months));
    (total / 12) * 10000 + (total % 12 + 1) * 100 + d
}

/// The games `period` takes: (record, the player had White), in record order.
pub fn choose(pool: &[PoolGame], period: Period) -> Vec<(u32, bool)> {
    choose_at(pool, period, today())
}

/// [`choose`] on the day `today`. A partly known date is in a period when any day it
/// can mean is.
fn choose_at(pool: &[PoolGame], period: Period, today: u32) -> Vec<(u32, bool)> {
    let mut out: Vec<(u32, bool)> = match period {
        Period::All => pool.iter().map(|g| (g.index, g.white)).collect(),
        Period::Months(months) => {
            let newest = newest_at(pool, today);
            if newest == 0 {
                return choose_at(pool, Period::All, today);
            }
            let from = cutoff(latest(newest).min(today), months);
            pool.iter()
                .filter(|g| g.date != 0 && latest(g.date) >= from)
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
        Period::Months(_) if newest(pool) != 0 => pool
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
        // Partly known dates.
        assert_eq!(latest(20260000), 20261231);
        assert_eq!(latest(20260200), 20260231);
        let today = 20261031;
        let year_only = [
            g(0, true, 20260000),
            g(1, true, 20250301),
            g(2, true, 20260700),
        ];
        // «2026» is up to today: three months back is 2026.07.31 — July is in.
        assert_eq!(
            choose_at(&year_only, Period::Months(3), today),
            vec![(0, true), (2, true)]
        );
        // A typo from the future is not the newest game.
        let typo = [
            g(0, true, 22060101),
            g(1, true, 20260301),
            g(2, true, 20250101),
        ];
        assert_eq!(newest_at(&typo, today), 20260301);
        assert_eq!(
            choose_at(&typo, Period::Months(6), today),
            vec![(0, true), (1, true)]
        );
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
        // Nothing dated: a period by months takes all.
        let undated = [g(0, true, 0), g(1, false, 0)];
        assert_eq!(choose(&undated, Period::Months(12)).len(), 2);
        assert_eq!(
            count(&undated, Period::Months(12), Side::Any).undated_out,
            0
        );
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
