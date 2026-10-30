//! A game against the computer (docs/design/play-mode.md, android-play.md §31): how it
//! ends, how long the opponent "thinks", its resignation judged by an engine, and Rodent
//! (`rodent`). Shared by the desktop and the Android apps; nothing here draws. The
//! engines that play or judge run as processes, each driven from a thread: a browser
//! cannot start them.

pub mod rodent;
mod rodent4_files;

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
use std::{sync::mpsc::channel, time::Instant};

use shakmaty::zobrist::Zobrist64;
use shakmaty::{Chess, Color, EnPassantMode, Position};

/// Maia's ratings: what her network was trained on (her own scale, not a person's Elo;
/// decisions «Сила соперника — по шкале самого движка»).
pub const MAIA_MIN: u32 = 600;
pub const MAIA_MAX: u32 = 2600;

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
    /// The user offered a draw and Maia accepted.
    DrawAgreed,
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
            | Ending::FiftyMoves
            | Ending::DrawAgreed => R::Draw,
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

// ----- the game's tags -----------------------------------------------------------------

/// The tags of a game against a program (as Lichess writes one): `event` in the app's
/// language, the user's name ("?" unknown) and rating, the program's name with its
/// strength («Maia 1500», «Rodent V 2400 (Tal)»: the engine's own scale, so no Elo tag
/// for it — the owner's decision), the time control in PGN ("-": none). The rest (date,
/// result) is the game's.
pub fn game_tags(
    event: &str,
    user: Color,
    user_name: &str,
    user_elo: Option<u32>,
    program: &str,
    time_control: &str,
) -> Vec<(String, String)> {
    let maia = program.to_owned();
    let you = if user_name.trim().is_empty() {
        "?".to_owned()
    } else {
        user_name.trim().to_owned()
    };
    let (white, black, we, be, wt, bt) = match user {
        Color::White => (you, maia, user_elo, None, "human", "program"),
        Color::Black => (maia, you, None, user_elo, "program", "human"),
    };
    let mut tags = vec![
        ("Event".to_owned(), event.to_owned()),
        ("Site".to_owned(), "ChessPenguin".to_owned()),
        ("White".to_owned(), white),
        ("Black".to_owned(), black),
    ];
    if let Some(e) = we {
        tags.push(("WhiteElo".to_owned(), e.to_string()));
    }
    if let Some(e) = be {
        tags.push(("BlackElo".to_owned(), e.to_string()));
    }
    tags.push(("WhiteType".to_owned(), wt.to_owned()));
    tags.push(("BlackType".to_owned(), bt.to_owned()));
    tags.push(("TimeControl".to_owned(), time_control.to_owned()));
    tags
}

// ----- a draw offered -------------------------------------------------------------------

/// Maia's turns whose scores must agree for a level position to be called settled.
pub const DRAW_STABLE_TURNS: usize = 3;
/// She accepts a draw when she is not better by more than this (centipawns)…
const DRAW_MAX_CP: i32 = 20;
/// …the position settled (her last scores no further apart than this)…
const DRAW_SPREAD_CP: i32 = 30;
/// …or at once when she is worse by this much.
const DRAW_WORSE_CP: i32 = -50;
/// After a refusal, her turns before the next offer counts.
pub const DRAW_AGAIN_AFTER: usize = 2;

/// Whether Maia accepts a draw, from the scores of her latest turns (oldest first, each
/// for her as the side to move): when clearly worse or mated, at once; when about level
/// (at most +0.2 for her), if the game has kept its character — her last three scores
/// within 0.3 of each other. Never when she mates or is better by more.
pub fn accepts_draw(scores: &[uci::Score]) -> bool {
    let Some(&last) = scores.last() else {
        return false;
    };
    match last {
        uci::Score::Mate(n) => return n < 0,
        uci::Score::Cp(cp) if cp <= DRAW_WORSE_CP => return true,
        uci::Score::Cp(cp) if cp > DRAW_MAX_CP => return false,
        uci::Score::Cp(_) => {}
    }
    if scores.len() < DRAW_STABLE_TURNS {
        return false;
    }
    let recent: Option<Vec<i32>> = scores[scores.len() - DRAW_STABLE_TURNS..]
        .iter()
        .map(|s| match s {
            uci::Score::Cp(cp) => Some(*cp),
            uci::Score::Mate(_) => None,
        })
        .collect();
    recent.is_some_and(|v| {
        let (lo, hi) = (v.iter().min().unwrap(), v.iter().max().unwrap());
        hi - lo <= DRAW_SPREAD_CP
    })
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
    /// The scores of Maia's turns so far (for a draw offered), oldest first.
    scores: Vec<uci::Score>,
    /// A draw refused when this many scores were in.
    refused_at: Option<usize>,
}

impl Judge {
    /// The engine of `config` with `options` set (an Android Stockfish: its `EvalFile`);
    /// `wake` is called when an answer comes (the app draws again).
    /// (A process driven from a thread: not in a browser.)
    #[cfg(not(target_family = "wasm"))]
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
            scores: Vec::new(),
            refused_at: None,
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
        if let Some(s) = out {
            self.scores.push(s);
        }
        out
    }

    /// Moves were taken back: the scores of positions gone are forgotten.
    pub fn forget_scores(&mut self) {
        self.scores.clear();
        self.refused_at = None;
    }

    /// A draw may be offered (not right after a refusal: two of her turns later).
    pub fn may_offer_draw(&self) -> bool {
        self.refused_at
            .is_none_or(|n| self.scores.len() >= n + DRAW_AGAIN_AFTER)
    }

    /// The user offers a draw: whether Maia accepts (by [`accepts_draw`]).
    pub fn offer_draw(&mut self) -> bool {
        let yes = accepts_draw(&self.scores);
        if !yes {
            self.refused_at = Some(self.scores.len());
        }
        yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_has_its_strength_in_its_name_and_no_elo() {
        let tags = game_tags("Game", Color::Black, "Roman", Some(1500), "Maia 1550", "-");
        let get = |k: &str| tags.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("White"), Some("Maia 1550"));
        assert_eq!(get("WhiteElo"), None);
        assert_eq!(get("BlackElo"), Some("1500"));
        assert_eq!(get("WhiteType"), Some("program"));
    }

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
    fn maia_accepts_a_draw_when_level_and_settled_or_worse() {
        use uci::Score::{Cp, Mate};
        // Level and settled: yes; level but just changed: no.
        assert!(accepts_draw(&[Cp(10), Cp(-5), Cp(15)]));
        assert!(!accepts_draw(&[Cp(-120), Cp(-5), Cp(15)]));
        // Too few turns to call it settled.
        assert!(!accepts_draw(&[Cp(0), Cp(0)]));
        // Better by more than 0.2: no; clearly worse or mated: at once.
        assert!(!accepts_draw(&[Cp(25), Cp(25), Cp(25)]));
        assert!(accepts_draw(&[Cp(-60)]));
        assert!(accepts_draw(&[Mate(-4)]));
        assert!(!accepts_draw(&[Mate(3)]));
        assert!(!accepts_draw(&[]));
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
