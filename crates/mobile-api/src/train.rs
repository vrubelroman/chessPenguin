//! Learning an opening on the phone (docs/design/android-play.md §32): the shared
//! trainer (`crates/trainer`) over a line of a game; the app draws, this judges.

use std::sync::{Arc, Mutex};

use chess_core::NodeId;
use shakmaty::fen::Fen;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, EnPassantMode, Position};
use trainer::{LineStats, TrainStart, Training, Verdict};

use crate::session::GameSession;

/// Where the runs are kept (`training.json` in the app's files).
#[uniffi::export]
pub fn trainer_set_file(path: String) {
    trainer::set_file(path.into());
}

/// The runs of a line so far.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TrainStats {
    pub runs: u32,
    pub last: Option<u32>,
    pub best: Option<u32>,
    pub clean_streak: u32,
    /// The last runs, oldest first: their mistakes and whether the move was shown.
    pub recent_mistakes: Vec<u32>,
    pub recent_hinted: Vec<bool>,
}

impl From<&LineStats> for TrainStats {
    fn from(s: &LineStats) -> Self {
        TrainStats {
            runs: s.runs,
            last: s.last,
            best: s.best,
            clean_streak: s.clean_streak,
            recent_mistakes: s.recent.iter().map(|r| r.0).collect(),
            recent_hinted: s.recent.iter().map(|r| r.1).collect(),
        }
    }
}

/// What a move made was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TrainVerdict {
    Right,
    Finished,
    Wrong,
    Done,
}

/// The state of the run after `ply` half-moves.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TrainState {
    /// The position now, and the move that led here (none at the start).
    pub fen: String,
    pub from_square: Option<String>,
    pub to_square: Option<String>,
    /// The moves played so far, in SAN (Latin letters).
    pub played: Vec<String>,
    /// Half-moves in the line.
    pub length: u32,
    pub mistakes_here: u32,
    pub mistakes: u32,
    /// The user may ask to see this move; it is shown now (from, to).
    pub may_show: bool,
    pub hint: Option<Vec<String>>,
    pub hinted: bool,
    pub done: bool,
    pub before: TrainStats,
    pub after: Option<TrainStats>,
}

/// A training of one line.
#[derive(uniffi::Object)]
pub struct Trainer {
    training: Mutex<Training>,
    start: Chess,
}

impl Trainer {
    fn with<R>(&self, f: impl FnOnce(&mut Training) -> R) -> R {
        f(&mut self.training.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The position after `ply` moves of the line.
    fn position(&self, t: &Training, ply: usize) -> Chess {
        let mut pos = self.start.clone();
        for m in t.start.line.iter().take(ply) {
            pos.play_unchecked(*m);
        }
        pos
    }
}

fn fen(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

/// The squares of a move as the board shows them (castling: the king's two squares).
fn squares(m: &shakmaty::Move) -> (String, String) {
    let to = match (m.castling_side(), m.from()) {
        (Some(side), Some(from)) => side.king_to(if from.rank() == shakmaty::Rank::First {
            shakmaty::Color::White
        } else {
            shakmaty::Color::Black
        }),
        _ => m.to(),
    };
    (m.from().unwrap_or(m.to()).to_string(), to.to_string())
}

#[uniffi::export]
impl GameSession {
    /// A training of the line from the start to the move at place `at` (none: no move
    /// there, or a null move on the way).
    pub fn trainer(&self, at: u32) -> Option<Arc<Trainer>> {
        let start = self.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            (id != NodeId::ROOT)
                .then(|| TrainStart::from_game(g, id))
                .flatten()
        })?;
        let pos = match start.start_fen.as_deref() {
            Some(f) => f
                .parse::<Fen>()
                .ok()?
                .into_position::<Chess>(CastlingMode::Standard)
                .ok()?,
            None => Chess::default(),
        };
        Some(Arc::new(Trainer {
            training: Mutex::new(Training::new(start)),
            start: pos,
        }))
    }

    /// How many times the line up to the move at place `at` was trained.
    pub fn training_runs(&self, at: u32) -> Option<u32> {
        self.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            trainer::runs_to(g, id)
        })
    }
}

#[uniffi::export]
impl Trainer {
    /// «Carlsen — Caruana», when the game names its players.
    pub fn game(&self) -> Option<String> {
        self.with(|t| t.start.game.clone())
    }

    /// «Wijk aan Zee 2024», when the game says so.
    pub fn event(&self) -> Option<String> {
        self.with(|t| t.start.event.clone())
    }

    /// The line's last move («12…Rd8», SAN in Latin letters).
    pub fn up_to(&self) -> String {
        self.with(|t| t.start.up_to.clone())
    }

    /// The run after `ply` half-moves.
    pub fn state(&self, ply: u32) -> TrainState {
        let k = ply as usize;
        self.with(|t| {
            let pos = self.position(t, k);
            let mut played = Vec::new();
            let mut p = self.start.clone();
            for m in t.start.line.iter().take(k) {
                played.push(shakmaty::san::San::from_move(&p, *m).to_string());
                p.play_unchecked(*m);
            }
            let last = k
                .checked_sub(1)
                .and_then(|i| t.start.line.get(i))
                .map(squares);
            TrainState {
                fen: fen(&pos),
                from_square: last.as_ref().map(|s| s.0.clone()),
                to_square: last.map(|s| s.1),
                played,
                length: t.start.line.len() as u32,
                mistakes_here: t.mistakes_on(k),
                mistakes: t.total(),
                may_show: t.may_show(k),
                hint: (t.hint && t.shown.get(k).copied().unwrap_or(false))
                    .then(|| {
                        t.wanted(k).map(|m| {
                            let (a, b) = squares(&m);
                            vec![a, b]
                        })
                    })
                    .flatten(),
                hinted: t.hinted(),
                done: t.done,
                before: (&t.before).into(),
                after: t.after.as_ref().map(Into::into),
            }
        })
    }

    /// The legal moves after `ply` half-moves (UCI).
    pub fn legal_moves(&self, ply: u32) -> Vec<String> {
        self.with(|t| {
            if t.done {
                return Vec::new();
            }
            self.position(t, ply as usize)
                .legal_moves()
                .iter()
                .map(|m| m.to_uci(CastlingMode::Standard).to_string())
                .collect()
        })
    }

    /// The line's move on half-move `ply` in UCI: its piece where a pawn promotes (the
    /// phone promotes to a queen otherwise, android-play.md §29.4).
    pub fn promotion_at(&self, ply: u32) -> Option<String> {
        self.with(|t| {
            t.wanted(ply as usize)
                .filter(|m| m.promotion().is_some())
                .map(|m| m.to_uci(CastlingMode::Standard).to_string())
        })
    }

    /// Judges `uci` made after `ply` half-moves.
    pub fn judge(&self, ply: u32, uci: String) -> TrainVerdict {
        self.with(|t| {
            let pos = self.position(t, ply as usize);
            let Some(m) = uci
                .parse::<UciMove>()
                .ok()
                .and_then(|u| u.to_move(&pos).ok())
            else {
                return TrainVerdict::Wrong;
            };
            match t.judge(ply as usize, m) {
                Verdict::Right => TrainVerdict::Right,
                Verdict::Finished => TrainVerdict::Finished,
                Verdict::Wrong => TrainVerdict::Wrong,
                Verdict::Done => TrainVerdict::Done,
            }
        })
    }

    /// The position after a wrong `uci` made after `ply` half-moves (shown a moment).
    pub fn fen_after(&self, ply: u32, uci: String) -> Option<String> {
        self.with(|t| {
            let mut pos = self.position(t, ply as usize);
            let m = uci.parse::<UciMove>().ok()?.to_move(&pos).ok()?;
            pos.play_unchecked(m);
            Some(fen(&pos))
        })
    }

    /// Shows the move of half-move `ply` (the run is then «with a hint»).
    pub fn show(&self, ply: u32) {
        self.with(|t| t.show(ply as usize))
    }

    /// The same line again from the start.
    pub fn again(&self) {
        self.with(|t| t.again())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_trained_on_the_phone() {
        let dir = std::env::temp_dir().join(format!("cp-mtrain-{}", std::process::id()));
        trainer_set_file(dir.join("training.json").to_string_lossy().into_owned());
        let s = GameSession::from_pgn("[White \"A\"]\n\n1. e4 e5 2. Nf3 Nc6 *".into()).unwrap();
        let d = s.data();
        let nc6 = d.nodes.iter().find(|n| n.san == "Nc6").unwrap().id;
        let t = s.trainer(nc6).unwrap();
        assert_eq!(t.up_to(), "2…Nc6");
        assert_eq!(t.state(0).length, 4);
        assert_eq!(t.judge(0, "d2d4".into()), TrainVerdict::Wrong);
        assert!(
            t.fen_after(0, "d2d4".into())
                .unwrap()
                .starts_with("rnbqkbnr/pppppppp/8/8/3P4")
        );
        assert_eq!(t.state(0).mistakes_here, 1);
        for (k, m) in ["e2e4", "e7e5", "g1f3"].iter().enumerate() {
            assert_eq!(t.judge(k as u32, (*m).into()), TrainVerdict::Right);
        }
        assert_eq!(t.state(3).played, ["e4", "e5", "Nf3"]);
        assert_eq!(t.judge(3, "b8c6".into()), TrainVerdict::Finished);
        let st = t.state(4);
        assert!(st.done);
        assert_eq!(st.after.unwrap().runs, 1);
        assert_eq!(s.training_runs(nc6), Some(1));
        // Castling is shown on the king's squares.
        let c = GameSession::from_pgn("1. e4 e5 2. Nf3 Nc6 3. Bc4 Bc5 4. O-O *".into()).unwrap();
        let oo = c.data().nodes.iter().find(|n| n.san == "O-O").unwrap().id;
        let tc = c.trainer(oo).unwrap();
        let last = tc.state(7);
        assert_eq!(
            (last.from_square.as_deref(), last.to_square.as_deref()),
            (Some("e1"), Some("g1"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
