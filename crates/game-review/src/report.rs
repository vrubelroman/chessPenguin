//! Game review metrics: win percentage, move and game accuracy, inaccuracy/mistake/blunder
//! judgements, average centipawn loss and game phases.
//!
//! The method follows Lichess (lila `WinPercent`, `AccuracyPercent`, `Advice`, and the
//! scalachess `Divider`, see <https://lichess.org/page/accuracy>); this is an independent
//! reimplementation of the published formulas.

use chess_core::shakmaty::{Bitboard, Chess, Color, Position, Rank};
use serde::{Deserialize, Serialize};

use crate::Rules;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// !! ([`Rules::Strict`]): without the move the game was not won anyway.
    pub const WON_ANYWAY: f64 = 90.0;
    /// !: every other move loses at least this much.
    pub const GAP: f64 = 10.0;
    /// [`Rules::ChessCom`]: stricter — the owner wants few ! and right ones (2026-10-28):
    /// with 25 the check set's ! are chess.com's 87 % of the time (54 % of theirs found).
    pub const GAP_CHESSCOM: f64 = 25.0;
    /// !: the position after the move is not lost, and the best other move did not win.
    pub const GREAT_AFTER: f64 = 40.0;
    pub const GREAT_ALT: f64 = 75.0;
    /// [`Rules::ChessCom`]: nor keep a clear edge. chess.com's ! in the check set
    /// (`chesscom/`, 2026-10-28) all have the best other move at 59 % or less; above it
    /// it gives none (18.Nd4, 23.Rf7+, 25.Rxh7+ of Steinitz — von Bardeleben).
    pub const GREAT_ALT_CHESSCOM: f64 = 60.0;
    /// [`Rules::Strict`]: lost below, won above; a great move changes which.
    pub const LOST: f64 = 33.0;
    pub const WON: f64 = 67.0;
    /// [`Rules::Strict`]: an opponent's move losing this much (or judged a mistake or a
    /// blunder) starts a new combination.
    pub const OPPONENT_ERROR: f64 = 10.0;
    /// Below this the mover's position is too bad for either mark: no second search.
    /// (The lower of the two floors after the move, !'s, less the step from the best.)
    pub const SECOND_FROM: f64 = GREAT_AFTER - NEAR_BEST;
}

/// How hard a move is to find, in the mover's win %, from a shallow search
/// ([`crate::SHALLOW_NODES`]) of the engine's own best move. A move the strong search
/// plays and a weak one underrates is what people call brilliant (Zaidi & Guerzhoy 2024,
/// arXiv 2406.11895); an obvious only move (10.Be2 of an online game, 32.Kd2 of Tal —
/// Larsen) the shallow search finds too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Measure {
    /// How much worse than its own best move the shallow search finds the move played
    /// (0 when it finds this move best).
    #[default]
    Choice,
    /// How much worse the shallow search finds the move played than the deep one.
    Depth,
}

impl Measure {
    /// The difficulty of a move the deep search finds `deep`, the shallow search
    /// `played` and its own best move `best`.
    pub fn difficulty(self, deep: Eval, best: Eval, played: Eval, mover: Color) -> f64 {
        let top = match self {
            Measure::Choice => best,
            Measure::Depth => deep,
        };
        (top.win_percent(mover) - played.win_percent(mover)).max(0.0)
    }
}

/// The say of a move's difficulty ([`Measure`]) in !! and ! ([`Rules::ChessCom`] only),
/// being tried out (2026-10-29) and off by default: the default is the rules without it,
/// and no shallow search is made for them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hardness {
    /// !: only a move at least this hard to find (win %).
    pub great: Option<f64>,
    /// !: every other move loses at least this much (win %).
    pub gap: f64,
    /// !: never a capture nor an attacked piece stepping quietly away (the rules of
    /// 2026-10-28 and 2026-10-29 against obvious !).
    pub bans: bool,
    /// !!: also the only good move (as for !, with [`Self::brilliant_gap`]) at least this
    /// hard to find, sacrifice or not (in the check set at 5 000 nodes: 17…Be6!! of
    /// Byrne — Fischer, 33.c3+!! of Kasparov — Topalov).
    pub brilliant: Option<f64>,
    /// !! of a hard move: every other move loses at least this much (win %).
    pub brilliant_gap: f64,
    /// How the difficulty is measured.
    pub measure: Measure,
}

impl Default for Hardness {
    fn default() -> Hardness {
        Hardness {
            great: None,
            gap: good::GAP_CHESSCOM,
            bans: true,
            // On by the owner's word (2026-10-29): +2 !! of chess.com's in the check set
            // (90 % found, 95 % right), no wrong one added; the same over 2 000–5 000
            // nodes and thresholds 20–40 %.
            brilliant: Some(30.0),
            brilliant_gap: 10.0,
            measure: Measure::Choice,
        }
    }
}

impl Hardness {
    /// The rules ask how hard a move was to find: the shallow search is wanted.
    pub fn uses_shallow(&self) -> bool {
        self.great.is_some() || self.brilliant.is_some()
    }
}

/// Whether the best other move is worth searching for: the move played was the engine's
/// best (`eval` the position's evaluation), and the position is good enough for !! or !.
pub fn worth_second(eval: Eval, mover: Color) -> bool {
    eval.win_percent(mover) >= good::SECOND_FROM
}

/// Lost, equal, won (0, 1, 2), for [`Rules::Strict`].
fn outcome(win: f64) -> u8 {
    if win < good::LOST {
        0
    } else if win > good::WON {
        2
    } else {
        1
    }
}

/// Brilliant or great, from the evaluations around the move (before the combination rule
/// of [`Rules::Strict`]).
fn note(before: Eval, after: Eval, ply: &Ply, rules: Rules, hard: Hardness) -> Option<Judgement> {
    let f = ply.features?;
    let (wb, wa) = (before.win_percent(ply.mover), after.win_percent(ply.mover));
    let loss = wb - wa;
    let second = ply.second.map(|e| e.win_percent(ply.mover));
    // Without the move: the best other move, or the best move when another was played.
    let without = if ply.best { second } else { Some(wb) };
    // With [`Rules::ChessCom`] a sacrifice that mates sooner than any other move was not
    // won anyway: 22.Qf6+!! of the Immortal game mates in 2, 22.Qf7 only in 4 (chess.com
    // marks it; win % cannot tell the two apart).
    let mates_sooner = rules == Rules::ChessCom
        && ply.best
        && ply
            .second
            .is_some_and(|s| mates_sooner(after, s, ply.mover));
    // Won anyway without the move: [`Rules::Strict`] by win %, [`Rules::ChessCom`] only
    // with a forced mate there — every sacrifice of the check set whose best other move
    // still won (to 93 %) is chess.com's !! (23…Qg3 of Levitsky — Marshall, 22…Rxc3 of
    // Rotlewi — Rubinstein).
    let without_eval = if ply.best { ply.second } else { Some(before) };
    let won_anyway = match rules {
        Rules::Strict => without.is_none_or(|w| w >= good::WON_ANYWAY),
        Rules::ChessCom => without_eval.is_none_or(|e| e.mate_for(ply.mover) == Some(true)),
    };
    if f.brilliant_shape(rules)
        && (ply.best || loss <= good::NEAR_BEST)
        && wa >= good::SOUND_AFTER
        && (!won_anyway || mates_sooner)
    {
        return Some(Judgement::Brilliant);
    }
    if f.pawn_shape(rules) && mates_sooner {
        return Some(Judgement::Brilliant);
    }
    let w2 = second?;
    let bans = rules == Rules::Strict || hard.bans;
    // [`Rules::ChessCom`]: a capture is the natural move, never great (12…Nxc3, 13…Nxe4 of
    // Byrne — Fischer, 22…Nbxd5, 26.Qxd4+ of Kasparov — Topalov are not chess.com's !; the
    // few captures it marks — 20.Rxe7+, 18.fxe6 — are given up for fewer wrong ones).
    if rules == Rules::ChessCom && bans && f.capture {
        return None;
    }
    // The engine's own best move loses nothing: a drop to the next position is the noise
    // of two searches (9.Nf5! of the Immortal game, −0.51 then −0.82 at 3M nodes); the
    // first rules still count it.
    let only_move = |gap: f64| {
        ply.best
            && (rules == Rules::ChessCom || loss <= good::NEAR_BEST)
            && wb - w2 >= gap
            && (rules == Rules::ChessCom || outcome(wa) > outcome(w2))
            && wa >= good::GREAT_AFTER
            && w2
                < match rules {
                    Rules::Strict => good::GREAT_ALT,
                    Rules::ChessCom => good::GREAT_ALT_CHESSCOM,
                }
    };
    // Tried and dropped (2026-10-28): a pawn offered as the only good move brilliant too —
    // finds 33.c3+!! of Kasparov — Topalov, but also marks 17.d5 of Steinitz — von
    // Bardeleben, which chess.com leaves unmarked.
    if !f.great_shape(rules, bans) {
        return None;
    }
    if rules == Rules::Strict {
        return only_move(good::GAP).then_some(Judgement::Great);
    }
    // How hard the move was to find: unknown (no shallow search) is not hard.
    let at_least = |t: Option<f64>| t.is_some_and(|t| ply.difficulty.is_some_and(|d| d >= t));
    if at_least(hard.brilliant) && only_move(hard.brilliant_gap) {
        Some(Judgement::Brilliant)
    } else if only_move(hard.gap) && (hard.great.is_none() || at_least(hard.great)) {
        Some(Judgement::Great)
    } else {
        None
    }
}

/// `after` (the move played) mates `mover`'s opponent in fewer moves than `other` (the
/// best other move), or `other` does not mate at all.
fn mates_sooner(after: Eval, other: Eval, mover: Color) -> bool {
    let mine = |e: Eval| match e {
        Eval::Mate { white, moves } if white == (mover == Color::White) => Some(moves),
        _ => None,
    };
    match (mine(after), mine(other)) {
        (Some(a), Some(o)) => a < o,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Judgement of a move from the evaluations before and after it: an inaccuracy, a
/// mistake or a blunder by the drop of the mover's winning chances (never for the
/// engine's own best move: the caller rules that out).
pub fn judge(before: Eval, after: Eval, mover: Color) -> Option<Judgement> {
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
    /// How hard the move was to find ([`Hardness::measure`]), when a shallow search was
    /// made.
    pub difficulty: Option<f64>,
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
        Self::build_with(evals, plies, phases, Rules::default())
    }

    /// [`Report::build`] with these rules for !! and !.
    pub fn build_with(
        evals: &[Option<Eval>],
        plies: &[Ply],
        phases: Phases,
        rules: Rules,
    ) -> Report {
        Self::build_tuned(evals, plies, phases, rules, Hardness::default())
    }

    /// [`Report::build_with`] where the difficulty of a move counts as `hard` says.
    pub fn build_tuned(
        evals: &[Option<Eval>],
        plies: &[Ply],
        phases: Phases,
        rules: Rules,
        hard: Hardness,
    ) -> Report {
        let n = plies.len().min(evals.len().saturating_sub(1));
        let mut report = Report {
            judgements: vec![None; plies.len()],
            phases,
            complete: evals.iter().take(n + 1).all(Option::is_some),
            ..Report::default()
        };
        let mut losses: [Vec<i32>; 2] = [Vec::new(), Vec::new()];
        // For the combination rule of [`Rules::Strict`]: the moves that passed !! or !,
        // and each move's loss.
        let mut noted = vec![false; n];
        let mut lost = vec![0.0; n];
        for i in 0..n {
            let mut ply = plies[i];
            // Winning back after the opponent took the mover's sacrifice goes on with the
            // combination: no trade (18…Bxc4+ of Byrne — Fischer after 17…Be6 18.Bxb6).
            if let (Some(f), Some(own)) = (
                ply.features.as_mut(),
                i.checked_sub(2).and_then(|k| plies[k].features),
            ) && own.sacrifice
            {
                f.takes_back = false;
            }
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
                note(before, after, &ply, rules, hard)
            } else {
                None
            };
            noted[i] = good.is_some();
            // [`Rules::ChessCom`] marks every move of a combination on its own (21.Qxd7+!!
            // 22.Bf5+! 23.Bd7+! in the Evergreen game); [`Rules::Strict`] the first one,
            // unless the opponent erred in between.
            let erred = |k: usize| {
                lost[k] >= good::OPPONENT_ERROR
                    || matches!(
                        report.judgements[k],
                        Some(Judgement::Mistake | Judgement::Blunder)
                    )
            };
            let goes_on = rules == Rules::Strict && i >= 2 && noted[i - 2] && !erred(i - 1);
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
            difficulty: None,
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
            difficulty: None,
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
        // Another move won as well: still !! by chess.com's marks, no !! by the first
        // rules; with a forced mate there: no !! either way.
        let r = Report::build(
            &evals,
            &[noted(w, true, Some(Eval::Cp(800)))],
            Phases::default(),
        );
        assert_eq!(r.judgements[0], Some(Judgement::Brilliant));
        let r = Report::build_with(
            &evals,
            &[noted(w, true, Some(Eval::Cp(800)))],
            Phases::default(),
            Rules::Strict,
        );
        assert_eq!(r.judgements[0], None);
        let mate = Some(Eval::Mate {
            white: true,
            moves: 5,
        });
        let r = Report::build(&evals, &[noted(w, true, mate)], Phases::default());
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
    fn every_move_of_a_combination_is_marked() {
        // As on chess.com: 21.Qxd7+!! 22.Bf5+! 23.Bd7+! in the Evergreen game.
        let (w, b) = (Color::White, Color::Black);
        let great = || noted(w, false, Some(Eval::Cp(-300)));
        let evals = [Some(Eval::Cp(300)); 4];
        let r = Report::build(&evals, &[great(), ply(b), great()], Phases::default());
        assert_eq!(
            r.judgements,
            vec![Some(Judgement::Great), None, Some(Judgement::Great)]
        );
        // Only the best move of a position where the others fall clearly behind.
        let mut close = great();
        close.second = Some(Eval::Cp(250));
        let r = Report::build(&evals, &[close], Phases::default());
        assert_eq!(r.judgements[0], None);
    }

    #[test]
    fn the_strict_rules_want_a_turn_of_the_game_and_one_mark_a_combination() {
        let (w, b) = (Color::White, Color::Black);
        // The only good move of an equal position: great by chess.com, not by the first rules.
        let equal = [Some(Eval::Cp(0)), Some(Eval::Cp(0))];
        let only = || noted(w, false, Some(Eval::Cp(-400)));
        let r = Report::build_with(&equal, &[only()], Phases::default(), Rules::ChessCom);
        assert_eq!(r.judgements[0], Some(Judgement::Great));
        // Others still equal-ish (−1.5): no turn of the game for the first rules, and
        // short of the stricter gap of chess.com's rules.
        let only = || noted(w, false, Some(Eval::Cp(-150)));
        let r = Report::build_with(&equal, &[only()], Phases::default(), Rules::ChessCom);
        assert_eq!(r.judgements[0], None);
        let r = Report::build_with(&equal, &[only()], Phases::default(), Rules::Strict);
        assert_eq!(r.judgements[0], None);
        // A combination: marked once by the first rules.
        let great = || noted(w, false, Some(Eval::Cp(-300)));
        let evals = [Some(Eval::Cp(300)); 4];
        let r = Report::build_with(
            &evals,
            &[great(), ply(b), great()],
            Phases::default(),
            Rules::Strict,
        );
        assert_eq!(r.judgements, vec![Some(Judgement::Great), None, None]);
    }
}
