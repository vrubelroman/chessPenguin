//! Game review metrics: win percentage, move and game accuracy, inaccuracy/mistake/blunder
//! judgements, average centipawn loss and game phases.
//!
//! The method follows Lichess (lila `WinPercent`, `AccuracyPercent`, `Advice`, and the
//! scalachess `Divider`, see <https://lichess.org/page/accuracy>); this is an independent
//! reimplementation of the published formulas.

use chess_core::shakmaty::{Bitboard, Chess, Color, Position, Rank};
use serde::{Deserialize, Serialize};

/// Engine evaluation from White's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Eval {
    Cp(i32),
    /// Forced mate; `moves == 0` in a checkmate position.
    Mate {
        white: bool,
        moves: u32,
    },
}

/// Centipawn ceiling: larger advantages (and mates) count as this much.
const CP_CEILING: i32 = 1000;

impl Eval {
    pub fn from_white_eval(e: uci::WhiteEval) -> Eval {
        match e {
            uci::WhiteEval::Cp(cp) => Eval::Cp(cp),
            uci::WhiteEval::Mate { winner, moves } => Eval::Mate {
                white: winner == uci::Side::White,
                moves,
            },
        }
    }

    /// Centipawns for `side`, mates folded into the ceiling.
    fn cp_for(self, side: Color) -> i32 {
        let white = match self {
            Eval::Cp(cp) => cp.clamp(-CP_CEILING, CP_CEILING),
            Eval::Mate { white: true, .. } => CP_CEILING,
            Eval::Mate { white: false, .. } => -CP_CEILING,
        };
        if side == Color::White { white } else { -white }
    }

    /// `Some(true)` if `side` has a forced mate, `Some(false)` if it is being mated.
    fn mate_for(self, side: Color) -> Option<bool> {
        match self {
            Eval::Cp(_) => None,
            Eval::Mate { white, .. } => Some(white == (side == Color::White)),
        }
    }

    /// Chance of winning for `side`, 0..100.
    pub fn win_percent(self, side: Color) -> f64 {
        50.0 + 50.0 * winning_chances(self.cp_for(side))
    }

    /// "+0.35", "−1.20", "#3", "#-2", "1-0".
    pub fn format(self) -> String {
        match self {
            Eval::Cp(cp) => format!("{:+.2}", f64::from(cp) / 100.0),
            Eval::Mate { white, moves: 0 } => if white { "1-0" } else { "0-1" }.into(),
            Eval::Mate { white: true, moves } => format!("#{moves}"),
            Eval::Mate {
                white: false,
                moves,
            } => format!("#-{moves}"),
        }
    }

    /// Value for a PGN `[%eval …]` command (pawns or `#n`).
    pub fn pgn(self) -> String {
        match self {
            Eval::Cp(cp) => format!("{:.2}", f64::from(cp) / 100.0),
            Eval::Mate { white, moves } => {
                format!("#{}{moves}", if white { "" } else { "-" })
            }
        }
    }

    /// Parses a PGN `[%eval …]` payload ("0.35", "-1.2", "#3", "#-2").
    pub fn parse_pgn(s: &str) -> Option<Eval> {
        let s = s.split([',', ' ']).next()?.trim();
        if let Some(m) = s.strip_prefix('#') {
            let n: i32 = m.parse().ok()?;
            return Some(Eval::Mate {
                white: !m.starts_with('-'),
                moves: n.unsigned_abs(),
            });
        }
        let pawns: f64 = s.parse().ok()?;
        pawns
            .is_finite()
            .then(|| Eval::Cp((pawns * 100.0).round().clamp(-100_000.0, 100_000.0) as i32))
    }
}

/// Lichess winning chances, −1..1.
fn winning_chances(cp: i32) -> f64 {
    2.0 / (1.0 + (-0.003_682_08 * f64::from(cp)).exp()) - 1.0
}

/// Accuracy of a move given the mover's win percentage before and after it, 0..100.
fn move_accuracy(before: f64, after: f64) -> f64 {
    if after >= before {
        return 100.0;
    }
    let raw = 103.166_810_071_164_9 * (-0.043_544_153_867_539_51 * (before - after)).exp()
        - 3.166_924_740_191_411;
    // +1: uncertainty bonus for imperfect analysis, as on Lichess.
    (raw + 1.0).clamp(0.0, 100.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Judgement {
    Inaccuracy,
    Mistake,
    Blunder,
    /// The only good move, or one that turned the game (!).
    Great,
    /// A sound sacrifice of a piece (!!).
    Brilliant,
}

impl Judgement {
    /// An error (?!, ?, ??), not a move of note.
    pub fn is_error(self) -> bool {
        matches!(
            self,
            Judgement::Inaccuracy | Judgement::Mistake | Judgement::Blunder
        )
    }
}

/// The thresholds of !! and ! in the mover's win % (docs/research/brilliant-moves.md §10).
mod good {
    /// At most this behind the engine's best move.
    pub const NEAR_BEST: f64 = 2.0;
    /// !!: the position after the sacrifice is at least this good.
    pub const SOUND_AFTER: f64 = 45.0;
    /// !!: without the move the game was not won anyway.
    pub const WON_ANYWAY: f64 = 90.0;
    /// !: every other move loses at least this much.
    pub const GAP: f64 = 10.0;
    /// !: lost below, won above (the move changes which).
    pub const LOST: f64 = 33.0;
    pub const WON: f64 = 67.0;
    /// !: the position after the move is not lost, and the best other move did not win.
    pub const GREAT_AFTER: f64 = 40.0;
    pub const GREAT_ALT: f64 = 75.0;
    /// An opponent's move losing this much (or judged a mistake or a blunder: near a mate
    /// the win % hardly moves) starts a new combination.
    pub const OPPONENT_ERROR: f64 = 10.0;
    /// Below this the mover's position is too bad for either mark: no second search.
    /// (The lower of the two floors after the move, !'s, less the step from the best.)
    pub const SECOND_FROM: f64 = GREAT_AFTER - NEAR_BEST;
}

/// Whether the best other move is worth searching for: the move played was the engine's
/// best (`eval` the position's evaluation), and the position is good enough for !! or !.
pub fn worth_second(eval: Eval, mover: Color) -> bool {
    eval.win_percent(mover) >= good::SECOND_FROM
}

/// Lost, equal, won (0, 1, 2).
fn outcome(win: f64) -> u8 {
    if win < good::LOST {
        0
    } else if win > good::WON {
        2
    } else {
        1
    }
}

/// Brilliant or great, from the evaluations around the move (before the combination rule).
fn note(before: Eval, after: Eval, ply: &Ply) -> Option<Judgement> {
    let f = ply.features?;
    let (wb, wa) = (before.win_percent(ply.mover), after.win_percent(ply.mover));
    let loss = wb - wa;
    let second = ply.second.map(|e| e.win_percent(ply.mover));
    // Without the move: the best other move, or the best move when another was played.
    let without = if ply.best { second } else { Some(wb) };
    if f.brilliant_shape()
        && (ply.best || loss <= good::NEAR_BEST)
        && wa >= good::SOUND_AFTER
        && without.is_some_and(|w| w < good::WON_ANYWAY)
    {
        return Some(Judgement::Brilliant);
    }
    let w2 = second?;
    (f.great_shape()
        && ply.best
        && loss <= good::NEAR_BEST
        && wb - w2 >= good::GAP
        && outcome(wa) > outcome(w2)
        && wa >= good::GREAT_AFTER
        && w2 < good::GREAT_ALT)
        .then_some(Judgement::Great)
}

/// Judgement of a move from the evaluations before and after it.
fn judge(before: Eval, after: Eval, mover: Color) -> Option<Judgement> {
    use Judgement::*;
    let (mb, ma) = (before.mate_for(mover), after.mate_for(mover));
    if ma == Some(false) && mb != Some(false) {
        // The move allows a forced mate.
        let cp = before.cp_for(mover);
        return Some(if cp < -999 {
            Inaccuracy
        } else if cp < -700 {
            Mistake
        } else {
            Blunder
        });
    }
    if mb == Some(true) && ma != Some(true) {
        // A forced mate was let go.
        let cp = after.cp_for(mover);
        return Some(if cp > 999 {
            Inaccuracy
        } else if cp > 700 {
            Mistake
        } else {
            Blunder
        });
    }
    if mb.is_some() || ma.is_some() {
        return None;
    }
    let delta = winning_chances(before.cp_for(mover)) - winning_chances(after.cp_for(mover));
    if delta >= 0.3 {
        Some(Blunder)
    } else if delta >= 0.2 {
        Some(Mistake)
    } else if delta >= 0.1 {
        Some(Inaccuracy)
    } else {
        None
    }
}

/// Game phases as indices of the first position of each phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Phases {
    pub middlegame: Option<usize>,
    pub endgame: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Opening,
    Middlegame,
    Endgame,
}

impl Phases {
    /// Splits a sequence of positions: the middlegame starts once few pieces are left or
    /// a back rank is sparse; the endgame once at most six queens, rooks and minor pieces remain.
    pub fn divide(positions: &[Chess]) -> Phases {
        let majors_minors = |p: &Chess| {
            let b = p.board();
            (b.knights() | b.bishops() | b.rooks() | b.queens()).count()
        };
        let backrank_sparse = |p: &Chess| {
            let b = p.board();
            (b.by_color(Color::White) & Bitboard::from_rank(Rank::First)).count() < 4
                || (b.by_color(Color::Black) & Bitboard::from_rank(Rank::Eighth)).count() < 4
        };
        let middlegame = positions
            .iter()
            .position(|p| majors_minors(p) <= 10 || backrank_sparse(p));
        let endgame = middlegame.and_then(|m| {
            positions[m..]
                .iter()
                .position(|p| majors_minors(p) <= 6)
                .map(|i| i + m)
        });
        Phases {
            middlegame,
            endgame,
        }
    }

    pub fn of(self, position: usize) -> Phase {
        if self.endgame.is_some_and(|e| position >= e) {
            Phase::Endgame
        } else if self.middlegame.is_some_and(|m| position >= m) {
            Phase::Middlegame
        } else {
            Phase::Opening
        }
    }
}

/// A move of the analysed line.
#[derive(Clone, Copy, Debug)]
pub struct Ply {
    pub mover: Color,
    /// Teaching null move `--`: not graded.
    pub null: bool,
    /// The move played equals the engine's best move (never judged as an error).
    pub best: bool,
    /// What the move does on the board (`None`: not known, never !! or !).
    pub features: Option<crate::MoveFeatures>,
    /// The best other move's evaluation, when the move played was the best.
    pub second: Option<Eval>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SideStats {
    pub inaccuracies: u32,
    pub mistakes: u32,
    pub blunders: u32,
    pub great: u32,
    pub brilliant: u32,
    /// Average centipawn loss.
    pub acpl: Option<u32>,
    pub accuracy: Option<f64>,
    /// Opening, middlegame, endgame.
    pub phase_accuracy: [Option<f64>; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Per ply of the line.
    pub judgements: Vec<Option<Judgement>>,
    pub white: SideStats,
    pub black: SideStats,
    pub phases: Phases,
    /// All positions are evaluated.
    pub complete: bool,
}

impl Report {
    pub fn side(&self, c: Color) -> &SideStats {
        match c {
            Color::White => &self.white,
            Color::Black => &self.black,
        }
    }

    /// `evals[i]` belongs to the position before ply `i`; `evals.len() == plies.len() + 1`.
    pub fn build(evals: &[Option<Eval>], plies: &[Ply], phases: Phases) -> Report {
        let n = plies.len().min(evals.len().saturating_sub(1));
        let mut report = Report {
            judgements: vec![None; plies.len()],
            phases,
            complete: evals.iter().take(n + 1).all(Option::is_some),
            ..Report::default()
        };
        let mut losses: [Vec<i32>; 2] = [Vec::new(), Vec::new()];
        // Moves that passed !! or ! before the combination rule, and each move's loss.
        let mut noted = vec![false; n];
        let mut lost = vec![0.0; n];
        for i in 0..n {
            let ply = plies[i];
            let (Some(before), Some(after)) = (evals[i], evals[i + 1]) else {
                continue;
            };
            if ply.null {
                continue;
            }
            lost[i] = before.win_percent(ply.mover) - after.win_percent(ply.mover);
            let error = if ply.best {
                None
            } else {
                judge(before, after, ply.mover)
            };
            let good = if error.is_none() {
                note(before, after, &ply)
            } else {
                None
            };
            noted[i] = good.is_some();
            // One mark a combination: the moves that go on with it after a mark are not
            // marked again, unless the opponent erred in between.
            let erred = |k: usize| {
                lost[k] >= good::OPPONENT_ERROR
                    || matches!(
                        report.judgements[k],
                        Some(Judgement::Mistake | Judgement::Blunder)
                    )
            };
            let goes_on = i >= 2 && noted[i - 2] && !erred(i - 1);
            let j = error.or(good.filter(|_| !goes_on));
            report.judgements[i] = j;
            let stats = match ply.mover {
                Color::White => &mut report.white,
                Color::Black => &mut report.black,
            };
            match j {
                Some(Judgement::Inaccuracy) => stats.inaccuracies += 1,
                Some(Judgement::Mistake) => stats.mistakes += 1,
                Some(Judgement::Blunder) => stats.blunders += 1,
                Some(Judgement::Great) => stats.great += 1,
                Some(Judgement::Brilliant) => stats.brilliant += 1,
                None => {}
            }
            let loss = (before.cp_for(ply.mover) - after.cp_for(ply.mover)).max(0);
            losses[ply.mover as usize].push(loss);
        }
        for color in [Color::White, Color::Black] {
            let l = &losses[color as usize];
            if !l.is_empty() {
                let avg = l.iter().map(|x| i64::from(*x)).sum::<i64>() as f64 / l.len() as f64;
                let stats = match color {
                    Color::White => &mut report.white,
                    Color::Black => &mut report.black,
                };
                stats.acpl = Some(avg.round() as u32);
            }
        }
        if report.complete && n > 0 {
            let wp: Vec<f64> = evals[..=n]
                .iter()
                .map(|e| e.map_or(50.0, |e| e.win_percent(Color::White)))
                .collect();
            report.fill_accuracy(&wp, &plies[..n]);
        }
        report
    }

    /// Lichess game accuracy: mean of the volatility-weighted mean and the harmonic mean
    /// of move accuracies.
    fn fill_accuracy(&mut self, wp: &[f64], plies: &[Ply]) {
        let window = (wp.len() / 10).clamp(2, 8).min(wp.len());
        let mut windows: Vec<&[f64]> = vec![&wp[..window]; window.saturating_sub(2)];
        windows.extend(wp.windows(window));
        let weights: Vec<f64> = windows
            .iter()
            .map(|w| std_dev(w).clamp(0.5, 12.0))
            .collect();
        // (color, phase, accuracy, weight)
        let mut moves = Vec::new();
        for (i, ply) in plies.iter().enumerate() {
            if ply.null {
                continue;
            }
            let (before, after) = (wp[i], wp[i + 1]);
            let (before, after) = match ply.mover {
                Color::White => (before, after),
                Color::Black => (100.0 - before, 100.0 - after),
            };
            let weight = weights.get(i).copied().unwrap_or(0.5);
            moves.push((
                ply.mover,
                self.phases.of(i),
                move_accuracy(before, after),
                weight,
            ));
        }
        for color in [Color::White, Color::Black] {
            let all: Vec<(f64, f64)> = moves
                .iter()
                .filter(|m| m.0 == color)
                .map(|m| (m.2, m.3))
                .collect();
            let phase = |p: Phase| {
                let v: Vec<(f64, f64)> = moves
                    .iter()
                    .filter(|m| m.0 == color && m.1 == p)
                    .map(|m| (m.2, m.3))
                    .collect();
                // Too few moves in a phase say nothing about it.
                if v.len() >= 3 { mixed_mean(&v) } else { None }
            };
            let stats = match color {
                Color::White => &mut self.white,
                Color::Black => &mut self.black,
            };
            stats.accuracy = mixed_mean(&all);
            stats.phase_accuracy = [
                phase(Phase::Opening),
                phase(Phase::Middlegame),
                phase(Phase::Endgame),
            ];
        }
    }
}

fn std_dev(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64).sqrt()
}

/// Average of the weighted mean and the harmonic mean of `(accuracy, weight)`.
fn mixed_mean(v: &[(f64, f64)]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let wsum: f64 = v.iter().map(|x| x.1).sum();
    let weighted = v.iter().map(|x| x.0 * x.1).sum::<f64>() / wsum.max(f64::EPSILON);
    let harmonic = v.len() as f64 / v.iter().map(|x| 1.0 / x.0.max(1.0)).sum::<f64>();
    Some((weighted + harmonic) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ply(mover: Color) -> Ply {
        Ply {
            mover,
            null: false,
            best: false,
            features: None,
            second: None,
        }
    }

    fn alternating(n: usize) -> Vec<Ply> {
        (0..n)
            .map(|i| {
                ply(if i % 2 == 0 {
                    Color::White
                } else {
                    Color::Black
                })
            })
            .collect()
    }

    #[test]
    fn win_percent_matches_lichess_curve() {
        assert!((Eval::Cp(0).win_percent(Color::White) - 50.0).abs() < 1e-9);
        let w = Eval::Cp(100).win_percent(Color::White);
        assert!((w - 59.1).abs() < 0.1, "{w}");
        assert!((Eval::Cp(100).win_percent(Color::Black) - (100.0 - w)).abs() < 1e-9);
        // Mates count as the centipawn ceiling.
        let m = Eval::Mate {
            white: true,
            moves: 3,
        };
        assert_eq!(
            m.win_percent(Color::White),
            Eval::Cp(5000).win_percent(Color::White)
        );
    }

    #[test]
    fn move_accuracy_bounds() {
        assert_eq!(move_accuracy(50.0, 60.0), 100.0);
        assert_eq!(move_accuracy(50.0, 50.0), 100.0);
        let small = move_accuracy(60.0, 55.0);
        let big = move_accuracy(60.0, 20.0);
        assert!(small > 75.0 && small < 100.0, "{small}");
        assert!(big < 20.0, "{big}");
        assert_eq!(move_accuracy(100.0, 0.0), 0.0);
    }

    #[test]
    fn judgements_follow_win_chance_drop() {
        let w = Color::White;
        assert_eq!(judge(Eval::Cp(0), Eval::Cp(-30), w), None);
        assert_eq!(
            judge(Eval::Cp(0), Eval::Cp(-60), w),
            Some(Judgement::Inaccuracy)
        );
        assert_eq!(
            judge(Eval::Cp(0), Eval::Cp(-150), w),
            Some(Judgement::Mistake)
        );
        assert_eq!(
            judge(Eval::Cp(0), Eval::Cp(-500), w),
            Some(Judgement::Blunder)
        );
        // Black's point of view: White's eval going up is Black's loss.
        assert_eq!(
            judge(Eval::Cp(0), Eval::Cp(500), Color::Black),
            Some(Judgement::Blunder)
        );
        // Already winning by a lot: losing a pawn changes nothing.
        assert_eq!(judge(Eval::Cp(900), Eval::Cp(800), w), None);
        let mated = Eval::Mate {
            white: false,
            moves: 4,
        };
        let mating = Eval::Mate {
            white: true,
            moves: 4,
        };
        assert_eq!(judge(Eval::Cp(0), mated, w), Some(Judgement::Blunder));
        assert_eq!(
            judge(Eval::Cp(-1200), mated, w),
            Some(Judgement::Inaccuracy)
        );
        assert_eq!(judge(mating, Eval::Cp(300), w), Some(Judgement::Blunder));
        assert_eq!(
            judge(mating, Eval::Cp(1500), w),
            Some(Judgement::Inaccuracy)
        );
        assert_eq!(judge(mating, mating, w), None);
        assert_eq!(judge(mated, mated, w), None);
        assert_eq!(judge(Eval::Cp(0), mating, w), None);
    }

    #[test]
    fn report_counts_and_accuracy() {
        // White blunders on ply 2; everything else is quiet.
        let cps = [20, 25, 20, -400, -390, -400, -395];
        let evals: Vec<Option<Eval>> = cps.iter().map(|c| Some(Eval::Cp(*c))).collect();
        let plies = alternating(cps.len() - 1);
        let r = Report::build(&evals, &plies, Phases::default());
        assert!(r.complete);
        assert_eq!(r.judgements[2], Some(Judgement::Blunder));
        assert_eq!(r.white.blunders, 1);
        assert_eq!(
            r.black.blunders + r.black.mistakes + r.black.inaccuracies,
            0
        );
        let (wa, ba) = (r.white.accuracy.expect("w"), r.black.accuracy.expect("b"));
        assert!(wa < ba, "white {wa} black {ba}");
        assert!(ba > 90.0);
        assert!(r.white.acpl.expect("acpl") > r.black.acpl.expect("acpl"));
    }

    #[test]
    fn partial_and_special_plies() {
        let evals = vec![Some(Eval::Cp(0)), Some(Eval::Cp(-500)), None, None];
        let mut plies = alternating(3);
        let r = Report::build(&evals, &plies, Phases::default());
        assert!(!r.complete);
        assert_eq!(r.white.blunders, 1);
        assert_eq!(r.white.accuracy, None);
        // The engine's own best move is never an error; null moves are not graded.
        plies[0].best = true;
        let r = Report::build(&evals, &plies, Phases::default());
        assert_eq!(r.white.blunders, 0);
        plies[0].best = false;
        plies[0].null = true;
        let r = Report::build(&evals, &plies, Phases::default());
        assert_eq!(r.white.blunders, 0);
        assert_eq!(r.white.acpl, None);
    }

    #[test]
    fn pgn_eval_roundtrip() {
        for e in [
            Eval::Cp(35),
            Eval::Cp(-120),
            Eval::Mate {
                white: true,
                moves: 3,
            },
            Eval::Mate {
                white: false,
                moves: 2,
            },
        ] {
            assert_eq!(Eval::parse_pgn(&e.pgn()), Some(e));
        }
        assert_eq!(Eval::parse_pgn("0.17,22"), Some(Eval::Cp(17)));
        assert_eq!(Eval::parse_pgn("abc"), None);
    }

    #[test]
    fn phases_of_a_real_game() {
        // The Opera Game: queens and rooks stay until the very end, so no endgame.
        let game = chess_core::parse_str(
            "1. e4 e5 2. Nf3 d6 3. d4 Bg4 4. dxe5 Bxf3 5. Qxf3 dxe5 6. Bc4 Nf6 7. Qb3 Qe7 \
             8. Nc3 c6 9. Bg5 b5 10. Nxb5 cxb5 11. Bxb5+ Nbd7 12. O-O-O Rd8 13. Rxd7 Rxd7 \
             14. Rd1 Qe6 15. Bxd7+ Nxd7 16. Qb8+ Nxb8 17. Rd8# 1-0",
        )
        .game;
        let positions: Vec<Chess> = std::iter::once(chess_core::NodeId::ROOT)
            .chain(game.mainline())
            .filter_map(|n| game.position(n).cloned())
            .collect();
        assert_eq!(positions.len(), 34);
        let p = Phases::divide(&positions);
        let mid = p.middlegame.expect("middlegame");
        assert!(mid > 4 && mid < 25, "{mid}");
        assert_eq!(p.of(0), Phase::Opening);
        assert_eq!(p.of(mid), Phase::Middlegame);
    }

    fn noted(mover: Color, sacrifice: bool, second: Option<Eval>) -> Ply {
        Ply {
            mover,
            null: false,
            best: true,
            features: Some(crate::MoveFeatures {
                legal: 30,
                sacrifice,
                ..Default::default()
            }),
            second,
        }
    }

    #[test]
    fn a_sound_sacrifice_is_brilliant_unless_the_game_was_won_anyway() {
        let w = Color::White;
        let evals = [Some(Eval::Cp(100)), Some(Eval::Cp(100))];
        let r = Report::build(
            &evals,
            &[noted(w, true, Some(Eval::Cp(-100)))],
            Phases::default(),
        );
        assert_eq!(r.judgements[0], Some(Judgement::Brilliant));
        assert_eq!(r.white.brilliant, 1);
        // Another move won as well: no !!.
        let r = Report::build(
            &evals,
            &[noted(w, true, Some(Eval::Cp(800)))],
            Phases::default(),
        );
        assert_eq!(r.judgements[0], None);
        // No best other move known (an older analysis): nothing said.
        let r = Report::build(&evals, &[noted(w, true, None)], Phases::default());
        assert_eq!(r.judgements[0], None);
        // Unsound: the position after it is bad.
        let bad = [Some(Eval::Cp(-200)), Some(Eval::Cp(-200))];
        let r = Report::build(
            &bad,
            &[noted(w, true, Some(Eval::Cp(-400)))],
            Phases::default(),
        );
        assert_eq!(r.judgements[0], None);
    }

    #[test]
    fn the_only_move_that_turns_the_game_is_great() {
        let w = Color::White;
        let evals = [Some(Eval::Cp(300)), Some(Eval::Cp(300))];
        let r = Report::build(
            &evals,
            &[noted(w, false, Some(Eval::Cp(-300)))],
            Phases::default(),
        );
        assert_eq!(r.judgements[0], Some(Judgement::Great));
        assert_eq!(r.white.great, 1);
        // Others are worse but still win: not great.
        let r = Report::build(
            &evals,
            &[noted(w, false, Some(Eval::Cp(250)))],
            Phases::default(),
        );
        assert_eq!(r.judgements[0], None);
        // Not the engine's move: never great.
        let mut p = noted(w, false, Some(Eval::Cp(-300)));
        p.best = false;
        assert_eq!(
            Report::build(&evals, &[p], Phases::default()).judgements[0],
            None
        );
    }

    #[test]
    fn a_combination_is_marked_once_unless_the_opponent_errs() {
        let (w, b) = (Color::White, Color::Black);
        let quiet = ply(b);
        let great = || noted(w, false, Some(Eval::Cp(-300)));
        let evals = [Some(Eval::Cp(300)); 4];
        let r = Report::build(&evals, &[great(), quiet, great()], Phases::default());
        assert_eq!(r.judgements, vec![Some(Judgement::Great), None, None]);
        // Black blunders in between: a new mark.
        let evals = [
            Some(Eval::Cp(300)),
            Some(Eval::Cp(300)),
            Some(Eval::Cp(900)),
            Some(Eval::Cp(900)),
        ];
        let mut strong = great();
        strong.second = Some(Eval::Cp(-300));
        let r = Report::build(&evals, &[great(), quiet, strong], Phases::default());
        assert_eq!(r.judgements[1], Some(Judgement::Blunder));
        assert_eq!(r.judgements[0], Some(Judgement::Great));
    }

    #[test]
    fn an_opponent_letting_a_mate_in_starts_a_new_combination() {
        // Near a mate the win % hardly moves: Black's move loses little of it but lets a
        // forced mate in, a mistake; White's next move of note is marked again.
        let (w, b) = (Color::White, Color::Black);
        let mate = Some(Eval::Mate {
            white: true,
            moves: 3,
        });
        let evals = [Some(Eval::Cp(900)), Some(Eval::Cp(900)), mate, mate];
        let great = || noted(w, false, Some(Eval::Cp(-300)));
        let r = Report::build(&evals, &[great(), ply(b), great()], Phases::default());
        assert_eq!(r.judgements[1], Some(Judgement::Mistake));
        assert_eq!(r.judgements[2], Some(Judgement::Great));
    }
}
