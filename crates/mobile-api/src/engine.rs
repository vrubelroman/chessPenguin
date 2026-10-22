//! The engine for the Android app: Stockfish as a child process supervised by the `uci`
//! crate (timeouts, `stop`, killing a hung engine), as on the desktop. The app asks for
//! the analysis of a position and polls; lines of an older position are never shown
//! for a newer one: every snapshot names its search, and events of earlier searches are
//! dropped.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Color, Position};
use uci::{
    Engine, EngineConfig, EngineEvent, EngineHandle, EnginePosition, Score, SearchId, Side,
    WhiteEval,
};

/// Why the engine cannot serve.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineFailure {
    /// The engine did not start or did not answer the handshake.
    #[error("{detail}")]
    Start { detail: String },
    /// The position cannot be sent (a bad FEN or move).
    #[error("{detail}")]
    Input { detail: String },
    /// The engine has stopped (it crashed, hung and was killed, or was quit).
    #[error("{detail}")]
    Stopped { detail: String },
}

/// One line of the analysis, from White's point of view.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EngineLine {
    /// 1 for the best line.
    pub multipv: u32,
    /// "+0.45", "−1.20", "0.00", "#3" (White mates), "#−2" (Black mates).
    pub eval: String,
    /// Centipawns for White, when the score is not a mate.
    pub cp: Option<i32>,
    /// Moves to mate: positive when White mates, negative when Black mates.
    pub mate: Option<i32>,
    /// The line in SAN with move numbers: "14.Nd5 Bxd5 15.exd5".
    pub text: String,
    /// The squares of its first move, for an arrow on the board.
    pub from_square: Option<String>,
    pub to_square: Option<String>,
    /// The line move by move, to show it on the board without touching the game.
    pub steps: Vec<LineStep>,
}

/// A move of an engine line and the position after it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LineStep {
    /// "14.Nd5", "14…Bxd5".
    pub label: String,
    pub fen: String,
    pub from_square: String,
    pub to_square: String,
}

/// What the engine says about the position of `search`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Analysis {
    pub search: u64,
    pub depth: u32,
    pub nodes_per_second: u64,
    /// Ordered by `multipv`.
    pub lines: Vec<EngineLine>,
}

struct Current {
    id: SearchId,
    root: Chess,
    depth: u32,
    nps: u64,
    lines: BTreeMap<u32, EngineLine>,
    changed: bool,
}

struct State {
    next: u64,
    multipv: u32,
    current: Option<Current>,
    stopped: Option<String>,
}

/// A running engine.
#[derive(uniffi::Object)]
pub struct Analyzer {
    handle: Mutex<Option<EngineHandle>>,
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[uniffi::export]
impl Analyzer {
    /// Starts the engine at `path` with its network `eval_file`. Blocks for the
    /// handshake (seconds at most): call it off the main thread.
    #[uniffi::constructor]
    pub fn start(
        path: String,
        eval_file: String,
        threads: u32,
        hash_mb: u32,
    ) -> Result<Arc<Self>, EngineFailure> {
        let start = |e: uci::EngineError| EngineFailure::Start {
            detail: e.to_string(),
        };
        // Below the app (nice 10): the phone's interface comes first while it thinks.
        let config = EngineConfig {
            nice: 10,
            ..EngineConfig::new(path)
        };
        let handle = Engine::spawn_with(config, Box::new(|| {})).map_err(start)?;
        if !eval_file.is_empty() {
            handle
                .set_option("EvalFile", Some(&eval_file))
                .map_err(start)?;
        }
        handle.set_threads(threads.max(1)).map_err(start)?;
        handle.set_hash_mb(hash_mb.max(1)).map_err(start)?;
        Ok(Arc::new(Analyzer {
            handle: Mutex::new(Some(handle)),
            state: Mutex::new(State {
                next: 0,
                multipv: 1,
                current: None,
                stopped: None,
            }),
        }))
    }

    /// Analyses the position reached from `start_fen` by `moves` (UCI), with `lines`
    /// lines, until [`Analyzer::stop`] or the next call. Returns the search's number:
    /// only snapshots with it belong to this position.
    pub fn analyze(
        &self,
        start_fen: String,
        moves: Vec<String>,
        lines: u32,
    ) -> Result<u64, EngineFailure> {
        let input = |detail: String| EngineFailure::Input { detail };
        let fen: Fen = start_fen.parse().map_err(|e| input(format!("{e}")))?;
        let mut root: Chess = fen
            .into_position(CastlingMode::Standard)
            .map_err(|e| input(format!("{e}")))?;
        for u in &moves {
            let m = u
                .parse::<UciMove>()
                .ok()
                .and_then(|m| m.to_move(&root).ok())
                .ok_or_else(|| input(format!("illegal move {u}")))?;
            root.play_unchecked(m);
        }
        let guard = lock(&self.handle);
        let handle = guard.as_ref().ok_or_else(|| self.stopped())?;
        let mut st = lock(&self.state);
        let lines = lines.clamp(1, 5);
        if st.multipv != lines {
            handle.set_multipv(lines).map_err(|e| self.failed(e))?;
            st.multipv = lines;
        }
        st.next += 1;
        let id = SearchId(st.next);
        handle
            .analyze(EnginePosition::fen(start_fen, moves), id)
            .map_err(|e| self.failed(e))?;
        st.current = Some(Current {
            id,
            root,
            depth: 0,
            nps: 0,
            lines: BTreeMap::new(),
            changed: false,
        });
        Ok(id.0)
    }

    /// Stops the search (its last lines stay valid for its position).
    pub fn stop(&self) {
        if let Some(h) = lock(&self.handle).as_ref() {
            let _ = h.stop();
        }
        lock(&self.state).current = None;
    }

    /// Waits up to `timeout_ms` for the engine; returns the analysis of the current
    /// search when it has changed, `None` otherwise. Fails once the engine has stopped.
    pub fn poll(&self, timeout_ms: u32) -> Result<Option<Analysis>, EngineFailure> {
        let mut events = Vec::new();
        {
            let guard = lock(&self.handle);
            let handle = guard.as_ref().ok_or_else(|| self.stopped())?;
            if let Some(e) = handle.recv_timeout(Duration::from_millis(timeout_ms.into())) {
                events.push(e);
                events.extend(handle.drain_events());
            }
        }
        let mut st = lock(&self.state);
        for e in events {
            match e {
                EngineEvent::Info { search_id, info } => {
                    let Some(cur) = st.current.as_mut().filter(|c| c.id == search_id) else {
                        continue;
                    };
                    if let Some(d) = info.depth {
                        cur.depth = d;
                    }
                    if let Some(n) = info.nps {
                        cur.nps = n;
                    }
                    if let (Some(score), false) = (info.score, info.pv.is_empty()) {
                        let k = info.multipv_index();
                        let line = engine_line(&cur.root, k, score.score, &info.pv);
                        cur.lines.insert(k, line);
                        cur.changed = true;
                    }
                }
                EngineEvent::Terminated(reason) => {
                    st.stopped = Some(format!("{reason:?}"));
                }
                _ => {}
            }
        }
        if let Some(reason) = st.stopped.clone() {
            *lock(&self.handle) = None;
            return Err(EngineFailure::Stopped { detail: reason });
        }
        let multipv = st.multipv;
        Ok(st.current.as_mut().filter(|c| c.changed).map(|c| {
            c.changed = false;
            Analysis {
                search: c.id.0,
                depth: c.depth,
                nodes_per_second: c.nps,
                lines: c
                    .lines
                    .values()
                    .filter(|l| l.multipv <= multipv)
                    .cloned()
                    .collect(),
            }
        }))
    }

    /// Quits the engine (and kills it if it does not leave in time).
    pub fn quit(&self) {
        if let Some(h) = lock(&self.handle).take() {
            h.quit();
        }
    }
}

impl Analyzer {
    fn stopped(&self) -> EngineFailure {
        EngineFailure::Stopped {
            detail: lock(&self.state)
                .stopped
                .clone()
                .unwrap_or_else(|| "the engine has stopped".into()),
        }
    }

    fn failed(&self, e: uci::EngineError) -> EngineFailure {
        EngineFailure::Stopped {
            detail: e.to_string(),
        }
    }
}

/// A line of the engine in SAN, its score for White.
fn engine_line(root: &Chess, multipv: u32, score: Score, pv: &[String]) -> EngineLine {
    let side = if root.turn() == Color::White {
        Side::White
    } else {
        Side::Black
    };
    let white = uci::EngineScore {
        score,
        bound: uci::ScoreBound::Exact,
    }
    .to_white(side);
    let (eval, cp, mate) = match white.eval {
        WhiteEval::Cp(cp) => (pawns(cp), Some(cp), None),
        WhiteEval::Mate { winner, moves } => {
            let n = moves as i32;
            if winner == Side::White {
                (format!("#{n}"), None, Some(n))
            } else {
                (format!("#\u{2212}{n}"), None, Some(-n))
            }
        }
    };
    let mut pos = root.clone();
    let mut words = Vec::new();
    let mut steps = Vec::new();
    let mut squares = None;
    for (i, u) in pv.iter().enumerate() {
        let Some(m) = u.parse::<UciMove>().ok().and_then(|m| m.to_move(&pos).ok()) else {
            break;
        };
        if i == 0 {
            squares = Some((u[0..2].to_owned(), u[2..4].to_owned()));
        }
        let number = pos.fullmoves().get();
        let white = pos.turn() == Color::White;
        let san = SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string();
        let label = if white {
            format!("{number}.{san}")
        } else {
            format!("{number}\u{2026}{san}")
        };
        words.push(if white || i == 0 { label.clone() } else { san });
        steps.push(LineStep {
            label,
            fen: Fen::from_position(&pos, shakmaty::EnPassantMode::Legal).to_string(),
            from_square: u[0..2].to_owned(),
            to_square: u[2..4].to_owned(),
        });
    }
    EngineLine {
        multipv,
        eval,
        cp,
        mate,
        text: words.join(" "),
        from_square: squares.as_ref().map(|s| s.0.clone()),
        to_square: squares.map(|s| s.1),
        steps,
    }
}

/// Centipawns as pawns with a sign: "+0.45", "−1.20", "0.00".
fn pawns(cp: i32) -> String {
    let sign = match cp.signum() {
        1 => "+",
        -1 => "\u{2212}",
        _ => "",
    };
    let a = cp.unsigned_abs();
    format!("{sign}{}.{:02}", a / 100, a % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_read_for_white() {
        let black_to_move: Chess = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"
            .parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap();
        let l = engine_line(
            &black_to_move,
            1,
            Score::Cp(-30),
            &["e7e5".into(), "g1f3".into()],
        );
        assert_eq!(l.eval, "+0.30");
        assert_eq!(l.text, "1\u{2026}e5 2.Nf3");
        assert_eq!(
            (l.from_square.as_deref(), l.to_square.as_deref()),
            (Some("e7"), Some("e5"))
        );
        let m = engine_line(&black_to_move, 1, Score::Mate(2), &[]);
        assert_eq!((m.eval.as_str(), m.mate), ("#\u{2212}2", Some(-2)));
        assert_eq!(pawns(0), "0.00");
        assert_eq!(pawns(-120), "\u{2212}1.20");
    }

    /// The fake engine of the `uci` crate (`cargo build -p uci --bin fake_uci`) answers
    /// like Stockfish; lines of an older search never show up for a newer one.
    #[test]
    fn analysis_belongs_to_its_position() {
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/debug/fake_uci");
        assert!(
            std::path::Path::new(fake).exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        let a = Analyzer::start(fake.into(), String::new(), 1, 16).unwrap();
        let start = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1".to_owned();
        let first = a.analyze(start.clone(), vec![], 1).unwrap();
        let second = a.analyze(start, vec!["e2e4".into()], 1).unwrap();
        assert!(second > first);
        let mut seen = 0;
        for _ in 0..50 {
            if let Some(s) = a.poll(100).unwrap() {
                assert_eq!(s.search, second);
                seen += 1;
            }
            if seen >= 3 {
                break;
            }
        }
        assert!(seen >= 3, "no analysis arrived");
        a.stop();
        a.quit();
        assert!(a.poll(10).is_err());
    }
}
