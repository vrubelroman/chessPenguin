//! Learning an opening: the user replays from memory the moves of both sides of a
//! line of a game, from its start to the chosen move (docs/design/opening-trainer.md,
//! docs/research/opening-trainer.md).
//!
//! Only the move of the line is right: another move — a move of a variation or one
//! transposing — is a mistake; it is shown for a moment and taken back. From the third
//! mistake on a move the user may ask to see it (the run is then marked «with a hint»).
//! The runs of each line are kept in `training.json`, by the line itself (its start
//! position and moves), so they survive edits of the game and are shared between games
//! with the same line, in the file the app names ([`set_file`]).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use web_time::Instant;

use chess_core::shakmaty::fen::Fen;
use chess_core::shakmaty::{CastlingMode, Chess, EnPassantMode, Move, Square};
use chess_core::{Game, MoveKind, NodeId};
use serde::{Deserialize, Serialize};

/// How long a wrong move stays on the board before it is taken back.
pub const WRONG_SHOWN: Duration = Duration::from_millis(400);

/// From this mistake on one move, the user may ask to see it.
pub const HINT_AFTER: u32 = 3;

/// The runs of a line remembered for its «recent» row.
const RECENT: usize = 10;

/// What a training starts from: the line and how to name it.
#[derive(Clone, Debug)]
pub struct TrainStart {
    /// The FEN of the game's start (`None`: the standard position).
    pub start_fen: Option<String>,
    pub line: Vec<Move>,
    /// «Carlsen — Caruana», when the game names its players.
    pub game: Option<String>,
    /// «Wijk aan Zee 2024», when the game says so.
    pub event: Option<String>,
    /// The last move of the line («12…Rd8», SAN in Latin letters: the app shows it its
    /// own way).
    pub up_to: String,
    /// Its move number.
    pub last_number: u32,
    /// The board as the game showed it.
    pub flipped: bool,
}

impl TrainStart {
    /// The line from the start of `game` to `at` (`None`: no move there, or a null
    /// move on the way — it cannot be played on the board).
    pub fn from_game(game: &Game, at: NodeId) -> Option<TrainStart> {
        let path = game.path(at);
        let mut line = Vec::new();
        for &n in &path[1..] {
            match game.game_move(n)?.kind {
                MoveKind::Normal(m) => line.push(m),
                MoveKind::Null => return None,
            }
        }
        if line.is_empty() {
            return None;
        }
        let (n, side) = game.move_number(at)?;
        let san = &game.game_move(at)?.san;
        let up_to = match side {
            chess_core::shakmaty::Color::White => format!("{n}.{san}"),
            chess_core::shakmaty::Color::Black => format!("{n}…{san}"),
        };
        let name = |t: &str| {
            game.tag(t)
                .map(str::trim)
                .filter(|v| !v.is_empty() && *v != "?")
        };
        let players = match (name("White"), name("Black")) {
            (None, None) => None,
            (w, b) => Some(format!("{} — {}", w.unwrap_or("?"), b.unwrap_or("?"))),
        };
        let year = name("Date")
            .map(|d| d.chars().take(4).collect::<String>())
            .filter(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()));
        let event = [name("Event").map(str::to_owned), year]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        Some(TrainStart {
            start_fen: (!game.start().is_standard()).then(|| game.start().fen()),
            line,
            game: players,
            event: (!event.is_empty()).then_some(event),
            up_to,
            last_number: n,
            flipped: false,
        })
    }

    /// The key of the line in the statistics.
    pub fn key(&self) -> String {
        let start = match self
            .start_fen
            .as_deref()
            .and_then(|f| f.parse::<Fen>().ok())
        {
            Some(fen) => fen
                .into_position::<Chess>(CastlingMode::Standard)
                .unwrap_or_default(),
            None => Chess::default(),
        };
        line_key(&start, &self.line)
    }
}

/// The key of a line: its start position (no counters) and its moves in UCI.
pub fn line_key(start: &Chess, line: &[Move]) -> String {
    let fen = Fen::from_position(start, EnPassantMode::Legal).to_string();
    let pos: Vec<&str> = fen.split(' ').take(4).collect();
    let moves: Vec<String> = line
        .iter()
        .map(|m| m.to_uci(CastlingMode::Standard).to_string())
        .collect();
    format!("{} | {}", pos.join(" "), moves.join(" "))
}

/// A wrong move shown on the board for a moment.
#[derive(Clone, Debug)]
pub struct Flash {
    /// The position after it.
    pub after: Chess,
    pub from: Square,
    pub to: Square,
    pub until: Instant,
}

/// A training in progress (or done).
pub struct Training {
    pub start: TrainStart,
    pub key: String,
    /// Mistakes on each half-move of the line in this run.
    pub mistakes: Vec<u32>,
    /// The move was shown (a hint) on each half-move.
    pub shown: Vec<bool>,
    /// The hint is on the board now (the arrow of the right move).
    pub hint: bool,
    pub flash: Option<Flash>,
    /// The run is over: all the moves made.
    pub done: bool,
    /// The line's runs before this one.
    pub before: LineStats,
    /// …and after it, once it is done.
    pub after: Option<LineStats>,
}

impl Training {
    pub fn new(start: TrainStart) -> Training {
        let key = start.key();
        let n = start.line.len();
        Training {
            before: stats(&key),
            key,
            mistakes: vec![0; n],
            shown: vec![false; n],
            hint: false,
            flash: None,
            done: false,
            after: None,
            start,
        }
    }

    /// The same line again from the start.
    pub fn again(&mut self) {
        let n = self.start.line.len();
        self.before = stats(&self.key);
        self.mistakes = vec![0; n];
        self.shown = vec![false; n];
        self.hint = false;
        self.flash = None;
        self.done = false;
        self.after = None;
    }

    pub fn total(&self) -> u32 {
        self.mistakes.iter().sum()
    }

    pub fn hinted(&self) -> bool {
        self.shown.iter().any(|&s| s)
    }

    /// The run is over: it counts.
    pub fn finish(&mut self) {
        self.done = true;
        self.hint = false;
        self.after = Some(record(&self.key, self.total(), self.hinted()));
    }

    /// The move the line wants on half-move `k` (none past its end).
    pub fn wanted(&self, k: usize) -> Option<Move> {
        self.start.line.get(k).copied()
    }

    /// Judges `m` played on half-move `k`: the line's move goes on (the last one ends
    /// the run, counted), any other is a mistake on this move. Nothing once it is done.
    pub fn judge(&mut self, k: usize, m: Move) -> Verdict {
        if self.done {
            return Verdict::Done;
        }
        let Some(want) = self.wanted(k) else {
            return Verdict::Done;
        };
        if m != want {
            self.mistakes[k] += 1;
            return Verdict::Wrong;
        }
        self.hint = false;
        if k + 1 == self.start.line.len() {
            self.finish();
            Verdict::Finished
        } else {
            Verdict::Right
        }
    }

    /// Mistakes on half-move `k` so far in this run.
    pub fn mistakes_on(&self, k: usize) -> u32 {
        self.mistakes.get(k).copied().unwrap_or(0)
    }

    /// The user may ask to see the move of half-move `k` (enough mistakes on it).
    pub fn may_show(&self, k: usize) -> bool {
        !self.done && self.mistakes_on(k) >= HINT_AFTER
    }

    /// Shows the move of half-move `k` (the run is then marked «with a hint»).
    pub fn show(&mut self, k: usize) {
        if let Some(s) = self.shown.get_mut(k) {
            *s = true;
            self.hint = true;
        }
    }
}

/// What a move made in a training was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The line's move: the line goes on.
    Right,
    /// The line's last move: the run is over and counted.
    Finished,
    /// Another move: a mistake, taken back.
    Wrong,
    /// The run was already over.
    Done,
}

/// What is known of the runs of a line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineStats {
    pub runs: u32,
    /// Mistakes of the last run.
    pub last: Option<u32>,
    /// The fewest mistakes of a run without a hint.
    pub best: Option<u32>,
    /// Runs without mistakes (or hints) in a row, to the last one.
    pub clean_streak: u32,
    /// The last runs, oldest first: mistakes, a hint.
    pub recent: Vec<(u32, bool)>,
    /// The day of the last run (for repetition by a schedule later).
    pub last_date: String,
}

/// The runs by line, as in the file: an entry that does not read is left as it is.
static STATS: Mutex<Option<serde_json::Map<String, serde_json::Value>>> = Mutex::new(None);

/// The file of the runs, as the app names it.
static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the runs are kept (`training.json` in the app's data); read again from there
/// when next needed. Without a file nothing is kept between starts.
pub fn set_file(path: PathBuf) {
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    *STATS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn file() -> Option<PathBuf> {
    FILE.lock().ok().and_then(|f| f.clone())
}

/// Writes through a file beside it: a run cut short leaves the old file whole.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn with_stats<R>(f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> R) -> R {
    let mut guard = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(load);
    f(map)
}

/// The file read; one that does not read as a whole is kept aside
/// (`training.json.unreadable`), not overwritten by the next run.
fn load() -> serde_json::Map<String, serde_json::Value> {
    let Some(path) = file() else {
        return Default::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Default::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(map) => map,
        Err(e) => {
            let aside = path.with_extension("json.unreadable");
            eprintln!(
                "training: {} unreadable ({e}), kept as {}",
                path.display(),
                aside.display()
            );
            let _ = std::fs::rename(&path, aside);
            Default::default()
        }
    }
}

/// The runs of the line `key` so far (an entry that does not read: none).
pub fn stats(key: &str) -> LineStats {
    with_stats(|m| {
        m.get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    })
}

/// A run done: counted and written at once.
fn record(key: &str, mistakes: u32, hinted: bool) -> LineStats {
    with_stats(|m| {
        let mut s: LineStats = m
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        s.runs += 1;
        s.last = Some(mistakes);
        if !hinted {
            s.best = Some(s.best.map_or(mistakes, |b| b.min(mistakes)));
        }
        s.clean_streak = if mistakes == 0 && !hinted {
            s.clean_streak + 1
        } else {
            0
        };
        s.recent.push((mistakes, hinted));
        if s.recent.len() > RECENT {
            s.recent.remove(0);
        }
        s.last_date = chrono::Local::now().format("%Y-%m-%d").to_string();
        if let Ok(v) = serde_json::to_value(&s) {
            m.insert(key.to_owned(), v);
        }
        let out = s;
        if let Some(path) = file() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec_pretty(m)
                && let Err(e) = write_atomic(&path, &json)
            {
                eprintln!("training: save failed: {e}");
            }
        }
        out
    })
}

/// The runs of the line from the start of `game` to `at`, for the move's menu
/// (`None`: no line there).
pub fn runs_to(game: &Game, at: NodeId) -> Option<u32> {
    TrainStart::from_game(game, at).map(|t| stats(&t.key()).runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_line_is_kept_by_its_moves_and_counted() {
        let dir = std::env::temp_dir().join(format!("cp-train-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        set_file(dir.join("training.json"));
        let a = chess_core::parse_str("[White \"A\"]\n\n1. e4 c5 2. Nf3 d6 *").game;
        let b = chess_core::parse_str("1. e4 c5 2. Nf3 d6 3. d4 (3. Bb5+) *").game;
        let at = |g: &Game, k: usize| g.mainline()[k];
        let ta = TrainStart::from_game(&a, at(&a, 3)).unwrap();
        let tb = TrainStart::from_game(&b, at(&b, 3)).unwrap();
        // The same line in two games: one key.
        assert_eq!(ta.key(), tb.key());
        assert_eq!(ta.game.as_deref(), Some("A — ?"));
        assert_eq!(ta.up_to, "2…d6");
        let mut t = Training::new(ta);
        t.mistakes[1] = 2;
        t.finish();
        let s = stats(&t.key);
        assert_eq!(
            (s.runs, s.last, s.best, s.clean_streak),
            (1, Some(2), Some(2), 0)
        );
        t.again();
        t.shown[0] = true;
        t.finish();
        let s = stats(&t.key);
        // A hint: counted, not the best.
        assert_eq!((s.runs, s.best), (2, Some(2)));
        assert_eq!(s.recent, [(2, false), (0, true)]);
        t.again();
        t.finish();
        let s = stats(&t.key);
        assert_eq!((s.best, s.clean_streak), (Some(0), 1));
        // Written to the file.
        let back: HashMap<String, LineStats> =
            serde_json::from_slice(&std::fs::read(dir.join("training.json")).unwrap()).unwrap();
        assert_eq!(back[&t.key].runs, 3);
        // An entry that does not read stays in the file as it is.
        let mut raw: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join("training.json")).unwrap()).unwrap();
        raw.insert("x".into(), serde_json::json!({"runs": "bad"}));
        std::fs::write(dir.join("training.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
        *STATS.lock().unwrap() = None;
        t.again();
        t.finish();
        let raw: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join("training.json")).unwrap()).unwrap();
        assert_eq!(raw["x"], serde_json::json!({"runs": "bad"}));
        assert_eq!(stats(&t.key).runs, 4);
        // A file that does not read is kept aside, not lost.
        std::fs::write(dir.join("training.json"), b"{ broken").unwrap();
        *STATS.lock().unwrap() = None;
        t.again();
        t.finish();
        assert_eq!(
            std::fs::read(dir.join("training.json.unreadable")).unwrap(),
            b"{ broken"
        );
        assert_eq!(stats(&t.key).runs, 1);
        // The verdict of a move: a mistake counts on its move, the last right move ends.
        let mut t = Training::new(tb);
        let line = t.start.line.clone();
        assert_eq!(t.judge(0, line[1]), Verdict::Wrong);
        assert_eq!(t.mistakes_on(0), 1);
        assert!(!t.may_show(0));
        t.judge(0, line[1]);
        t.judge(0, line[1]);
        assert!(t.may_show(0));
        t.show(0);
        assert_eq!(t.judge(0, line[0]), Verdict::Right);
        assert!(!t.hint);
        for (k, m) in line.iter().enumerate().skip(1).take(line.len() - 2) {
            assert_eq!(t.judge(k, *m), Verdict::Right);
        }
        assert_eq!(
            t.judge(line.len() - 1, line[line.len() - 1]),
            Verdict::Finished
        );
        assert!(t.done && t.hinted());
        assert_eq!(t.judge(0, line[0]), Verdict::Done);
        // A null move on the way: nothing to train.
        let n = chess_core::parse_str("1. e4 -- 2. d4 *").game;
        assert!(TrainStart::from_game(&n, n.mainline()[2]).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
