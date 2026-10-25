//! A game against Maia (docs/design/play-mode.md, android-play.md §31): how it ends,
//! how long Maia "thinks", and her resignation judged by an engine. Shared by the
//! desktop and the Android apps; nothing here draws.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use shakmaty::zobrist::Zobrist64;
use shakmaty::{Chess, Color, EnPassantMode, Position};

// ----- the end of a game ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Checkmate {
        winner: Color,
    },
    Stalemate,
    InsufficientMaterial,
    Threefold,
    FiftyMoves,
    /// `loser` ran out of time; a draw when the other side cannot mate.
    Time {
        loser: Color,
        draw: bool,
    },
    Resigned {
        loser: Color,
    },
}

impl Ending {
    pub fn result(self) -> chess_core::GameResult {
        use chess_core::GameResult as R;
        let win = |c: Color| match c {
            Color::White => R::WhiteWins,
            Color::Black => R::BlackWins,
        };
        match self {
            Ending::Checkmate { winner } => win(winner),
            Ending::Time { draw: true, .. } => R::Draw,
            Ending::Time { loser, .. } | Ending::Resigned { loser } => win(!loser),
            Ending::Stalemate
            | Ending::InsufficientMaterial
            | Ending::Threefold
            | Ending::FiftyMoves => R::Draw,
        }
    }

    /// The tag `Termination` (as Lichess writes it).
    pub fn termination(self) -> &'static str {
        match self {
            Ending::Time { .. } => "Time forfeit",
            _ => "Normal",
        }
    }
}

/// How the game stands after the last of `positions` (the positions of the main line
/// from the start): mate, stalemate, no material, threefold repetition, fifty moves.
pub fn ending(positions: &[Chess]) -> Option<Ending> {
    let pos = positions.last()?;
    if pos.is_checkmate() {
        return Some(Ending::Checkmate {
            winner: !pos.turn(),
        });
    }
    if pos.is_stalemate() {
        return Some(Ending::Stalemate);
    }
    if pos.is_insufficient_material() {
        return Some(Ending::InsufficientMaterial);
    }
    let key = |p: &Chess| p.zobrist_hash::<Zobrist64>(EnPassantMode::Legal);
    let now = key(pos);
    // Only positions since the last capture or pawn move can repeat.
    let span = (pos.halfmoves() as usize + 1).min(positions.len());
    let repeats = positions[positions.len() - span..]
        .iter()
        .filter(|p| key(p) == now)
        .count();
    if repeats >= 3 {
        return Some(Ending::Threefold);
    }
    if pos.halfmoves() >= 100 {
        return Some(Ending::FiftyMoves);
    }
    None
}

/// A number in [0, 1).
pub fn random_unit() -> f32 {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    (u32::from_le_bytes(b) >> 8) as f32 / (1u32 << 24) as f32
}

/// How long Maia "thinks" (it computes in a tenth of a second): about 1/30 of its time
/// plus most of the increment, varied at random; fast in the opening and with one
/// legal move; never so long that it could lose on time (docs/research/play-mode.md R15).
pub fn pause(clock: Option<(Duration, u32)>, ply: usize, only_move: bool) -> Duration {
    let r = random_unit();
    // A log-normal-ish factor between 0.4 and 2.5.
    let factor = (0.4f32 * (2.5f32 / 0.4).powf(r)).clamp(0.4, 2.5);
    let secs = match clock {
        None => 0.5 + 1.5 * r,
        Some((left, inc)) => {
            let left = left.as_secs_f32();
            let inc = inc as f32;
            let mut t = (left / 30.0 + 0.75 * inc) * factor;
            if ply < 10 {
                t *= 0.3;
            }
            // Never more than a fortieth of the time left plus most of the increment:
            // the clock only shrinks geometrically and never runs out, however long the
            // game (a move also costs about 0.1 s of computing).
            let most = (left / 40.0 + 0.8 * inc).min(60.0);
            t.min(most).max(0.4f32.min(most))
        }
    };
    let secs = if only_move {
        secs.min(0.3 + 0.5 * r)
    } else {
        secs
    };
    Duration::from_secs_f32(secs)
}

// ----- Maia resigns --------------------------------------------------------------------

/// Nodes the judge searches a position: a tenth of a second or two, enough to see that
/// a position is lost by twelve pawns.
pub const JUDGE_NODES: u64 = 150_000;

/// Maia resigns when her position is worse than this (centipawns) or she is mated…
const HOPELESS_CP: i32 = -1200;
/// …on this many of her turns in a row (one tactic in passing is not a lost game).
pub const HOPELESS_TURNS: u32 = 2;

/// A position lost for the side to move: twelve pawns down, or mated by force.
pub fn hopeless(score: uci::Score) -> bool {
    match score {
        uci::Score::Cp(cp) => cp <= HOPELESS_CP,
        uci::Score::Mate(n) => n < 0,
    }
}

/// The engine (Stockfish, unseen by the user) judging the positions where Maia is to
/// move, in its own thread for the whole game: the newest request is searched, older
/// ones are dropped; an engine that does not start or dies leaves the game without
/// resignations.
pub struct Judge {
    tx: Sender<(u64, uci::EnginePosition)>,
    rx: Receiver<(u64, uci::Score)>,
    generation: u64,
}

impl Judge {
    /// The engine of `config` with `options` set (an Android Stockfish: its `EvalFile`);
    /// `wake` is called when an answer comes (the app draws again).
    pub fn start(
        config: uci::EngineConfig,
        options: Vec<(String, String)>,
        wake: Box<dyn Fn() + Send>,
    ) -> Judge {
        let (tx, req_rx) = channel::<(u64, uci::EnginePosition)>();
        let (ans_tx, rx) = channel();
        let _ = std::thread::Builder::new()
            .name("maia-judge".into())
            .spawn(move || {
                let Ok(engine) = uci::Engine::spawn_with(config, Box::new(|| {})) else {
                    return;
                };
                for (name, value) in &options {
                    if engine.set_option(name, Some(value)).is_err() {
                        return;
                    }
                }
                let _ = engine.set_threads(1);
                let _ = engine.set_hash_mb(16);
                let _ = engine.set_multipv(1);
                let _ = engine.new_game();
                while let Ok(mut req) = req_rx.recv() {
                    while let Ok(newer) = req_rx.try_recv() {
                        req = newer;
                    }
                    let (generation, pos) = req;
                    let id = uci::SearchId(generation);
                    if engine
                        .search(pos, id, uci::SearchLimits::Nodes(JUDGE_NODES))
                        .is_err()
                    {
                        break;
                    }
                    let started = Instant::now();
                    let mut stopping = false;
                    let mut last = None;
                    loop {
                        if !stopping && started.elapsed() > Duration::from_secs(10) {
                            let _ = engine.stop();
                            stopping = true;
                        }
                        match engine.recv_timeout(Duration::from_millis(100)) {
                            Some(uci::EngineEvent::Info { search_id, info }) if search_id == id => {
                                if let Some(s) = info.score
                                    && (s.bound == uci::ScoreBound::Exact || last.is_none())
                                {
                                    last = Some(s.score);
                                }
                            }
                            Some(uci::EngineEvent::BestMove { search_id, .. })
                                if search_id == id =>
                            {
                                if let Some(score) = last {
                                    if ans_tx.send((generation, score)).is_err() {
                                        engine.quit();
                                        return;
                                    }
                                    wake();
                                }
                                break;
                            }
                            Some(uci::EngineEvent::Terminated(_)) => return,
                            _ => {}
                        }
                    }
                }
                engine.quit();
            });
        Judge {
            tx,
            rx,
            generation: 0,
        }
    }

    /// Judges `pos` (Maia to move); the answer to an older request is dropped.
    pub fn ask(&mut self, pos: uci::EnginePosition) {
        self.generation += 1;
        let _ = self.tx.send((self.generation, pos));
    }

    /// Drops the request in flight: its answer would be about a position gone.
    pub fn forget(&mut self) {
        self.generation += 1;
    }

    /// The score of the position asked last, for the side to move, once it has come.
    pub fn answer(&mut self) -> Option<uci::Score> {
        let mut out = None;
        while let Ok((g, s)) = self.rx.try_recv() {
            if g == self.generation {
                out = Some(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maia_resigns_only_when_hopeless() {
        use uci::Score;
        assert!(hopeless(Score::Cp(-1200)));
        assert!(hopeless(Score::Cp(-3000)));
        assert!(!hopeless(Score::Cp(-1199)));
        assert!(!hopeless(Score::Cp(900)));
        // Mated by force: hopeless; mating: not.
        assert!(hopeless(Score::Mate(-3)));
        assert!(!hopeless(Score::Mate(2)));
    }

    #[test]
    fn games_end_as_the_rules_say() {
        let line = |moves: &[&str]| {
            let mut pos = Chess::default();
            let mut out = vec![pos.clone()];
            for m in moves {
                let m = m
                    .parse::<shakmaty::san::San>()
                    .unwrap()
                    .to_move(&pos)
                    .unwrap();
                pos.play_unchecked(m);
                out.push(pos.clone());
            }
            out
        };
        assert_eq!(
            ending(&line(&["f3", "e5", "g4", "Qh4#"])),
            Some(Ending::Checkmate {
                winner: Color::Black
            })
        );
        assert_eq!(ending(&line(&["e4", "e5"])), None);
        let knights = ["Nf3", "Nf6", "Ng1", "Ng8", "Nf3", "Nf6", "Ng1", "Ng8"];
        assert_eq!(ending(&line(&knights)), Some(Ending::Threefold));
        // An untimed game: Maia thinks between half a second and two.
        let p = pause(None, 20, false);
        assert!(p >= Duration::from_millis(500) && p <= Duration::from_secs(2));
    }
}
