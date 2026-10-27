//! Playing against Maia (docs/research/play-mode.md, docs/design/play-mode.md): time
//! controls as on Lichess, the clock, the end of a game, and Maia's moves computed in a
//! background thread with a human-like pause.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chess_core::shakmaty::{Chess, Color, Move, Position};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;
use crate::maia_download::Published;
pub use ::play::rodent::{Gen, STYLES, Style, style};
pub use ::play::{
    Ending, HOPELESS_TURNS, Judge, MAIA_MAX, MAIA_MIN, ending, hopeless, pause, random_unit,
};
use maia::strength::Model;

// ----- time controls --------------------------------------------------------------------

/// A clock: base seconds and increment seconds; `None` in the settings is "no clock".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeControl {
    pub base: u32,
    pub increment: u32,
}

/// The pools of Lichess (lila `PoolList.scala`) from 3+0: no bullet (the owner's choice:
/// against Maia the fastest is 3+0).
pub const PRESETS: [TimeControl; 9] = [
    TimeControl {
        base: 180,
        increment: 0,
    },
    TimeControl {
        base: 180,
        increment: 2,
    },
    TimeControl {
        base: 300,
        increment: 0,
    },
    TimeControl {
        base: 300,
        increment: 3,
    },
    TimeControl {
        base: 600,
        increment: 0,
    },
    TimeControl {
        base: 600,
        increment: 5,
    },
    TimeControl {
        base: 900,
        increment: 10,
    },
    TimeControl {
        base: 1800,
        increment: 0,
    },
    TimeControl {
        base: 1800,
        increment: 20,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Blitz,
    Rapid,
    Classical,
}

impl Speed {
    pub fn name(self) -> &'static str {
        match self {
            Speed::Blitz => tr("Blitz"),
            Speed::Rapid => tr("Rapid"),
            Speed::Classical => tr("Classical"),
        }
    }
}

impl TimeControl {
    /// The category by the estimated length, base + 40 × increment (scalachess
    /// `Speed.scala`; nothing here is faster than 3+0, a blitz).
    pub fn speed(self) -> Speed {
        match self.base + 40 * self.increment {
            0..480 => Speed::Blitz,
            480..1500 => Speed::Rapid,
            _ => Speed::Classical,
        }
    }

    /// «5+3» as Lichess writes it (minutes + seconds).
    pub fn label(self) -> String {
        format!("{}+{}", self.base / 60, self.increment)
    }

    /// The fastest control offered: 3+0.
    pub fn allowed(self) -> bool {
        self.base >= 180
    }

    /// The PGN tag `TimeControl`: seconds + increment.
    pub fn pgn(self) -> String {
        format!("{}+{}", self.base, self.increment)
    }
}

/// The minutes the custom control offers (Lichess `timeControl.ts` from 3), in seconds.
pub const CUSTOM_BASES: &[u32] = &[
    180, 240, 300, 360, 420, 480, 540, 600, 660, 720, 780, 840, 900, 960, 1020, 1080, 1140, 1200,
    1500, 1800, 2100, 2400, 2700, 3600, 4500, 5400, 6300, 7200, 8100, 9000, 9900, 10800,
];
pub const CUSTOM_INCREMENTS: &[u32] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 25, 30, 35, 40, 45,
    60, 90, 120, 150, 180,
];

/// The colour the user asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorChoice {
    White,
    #[default]
    Random,
    Black,
}

/// The last choices of the play dialog (settings).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayPrefs {
    pub color: ColorChoice,
    /// `None`: no clock.
    pub clock: Option<TimeControl>,
    /// The opponent's strength on its own scale: Maia's rating, Rodent's UCI_Elo
    /// (decisions «Сила соперника — по шкале самого движка»). Kept as `level`.
    #[serde(rename = "level")]
    pub strength: u32,
    /// The opponent: Maia or Rodent, and Rodent's style (a personality file's name).
    pub rival: Rival,
    pub style: String,
}

/// Who plays against the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rival {
    /// Maia: human moves of a strength, human mistakes (docs/research/maia-strength.md).
    #[default]
    Maia,
    /// Rodent V: a strong engine with a style (docs/research/rodent.md).
    Rodent,
}

/// Downloading each Rodent (the engines' tables are the shared crate's, `play::rodent`).
pub trait RodentFiles {
    fn available(self) -> bool;
    fn installed(self) -> Option<std::path::PathBuf>;
    fn size_mb(self) -> u64;
    fn start(self, ctx: &egui::Context) -> crate::engine_download::Download;
    /// A file that does not start goes: the dialog offers the download again.
    fn remove(self);
}

impl RodentFiles for Gen {
    fn available(self) -> bool {
        match self {
            Gen::V => crate::rodent_download::available(),
            Gen::IV => crate::rodent4_download::available(),
        }
    }

    fn installed(self) -> Option<std::path::PathBuf> {
        match self {
            Gen::V => crate::rodent_download::installed(),
            Gen::IV => crate::rodent4_download::installed(),
        }
    }

    fn size_mb(self) -> u64 {
        match self {
            Gen::V => crate::rodent_download::size_mb(),
            Gen::IV => crate::rodent4_download::size_mb(),
        }
    }

    fn start(self, ctx: &egui::Context) -> crate::engine_download::Download {
        match self {
            Gen::V => crate::rodent_download::start(ctx),
            Gen::IV => crate::rodent4_download::start(ctx),
        }
    }

    fn remove(self) {
        let _ = match self {
            Gen::V => crate::rodent_download::remove(),
            Gen::IV => crate::rodent4_download::remove(),
        };
    }
}

/// «Attacking — Ampere»; «Balanced», «Tal»: no name twice.
fn style_label(st: &Style) -> String {
    let title = tr(st.title());
    // The default one is Rodent himself, the players' are named by their title, and so
    // are Rodent IV's characters (their titles are their names, translated).
    if st.player || st.id == "rodent" || st.title() == st.name {
        title.to_owned()
    } else {
        format!("{title} — {}", st.name)
    }
}

impl Default for PlayPrefs {
    fn default() -> Self {
        PlayPrefs {
            color: ColorChoice::Random,
            clock: Some(TimeControl {
                base: 600,
                increment: 5,
            }),
            strength: 1500,
            rival: Rival::Maia,
            style: STYLES[0].id.to_owned(),
        }
    }
}

// ----- the opponent's strength ---------------------------------------------------------

// Each engine on its own scale, as its authors give it (the owner's decision, after the
// measurements of docs/research/maia-strength.md and rodent.md): these are not a
// person's Elo, and the dialog says so. The scales are the shared crate's (`play`).

/// The strengths `rival` plays (Rodent's by the engine of the style chosen).
pub fn range(rival: Rival, style_id: &str) -> (u32, u32) {
    match rival {
        Rival::Maia => (MAIA_MIN, MAIA_MAX),
        Rival::Rodent => style(style_id).rodent.range(),
    }
}

/// The Maia that plays, with its file: the large one when it is there, else the small
/// one. `Err`: the model to download.
pub fn model_for() -> Result<(Model, std::path::PathBuf), Model> {
    use crate::maia_download::installed;
    if let Some(p) = installed(Model::Large) {
        return Ok((Model::Large, p));
    }
    installed(Model::Small)
        .map(|p| (Model::Small, p))
        .ok_or(Model::Large)
}

// ----- the clock -----------------------------------------------------------------------

/// Both clocks. Time runs for the side to move from the moment its opponent moved; a
/// side's first move takes no time (Lichess); the increment comes after the move.
#[derive(Clone, Debug)]
pub struct Clock {
    pub control: TimeControl,
    left: [Duration; 2],
    moved: [bool; 2],
    running: Option<(Color, Instant)>,
}

fn idx(c: Color) -> usize {
    match c {
        Color::White => 0,
        Color::Black => 1,
    }
}

impl Clock {
    pub fn new(control: TimeControl) -> Clock {
        // A base of 0 starts from the increment, at least 3 s (scalachess).
        let start = if control.base == 0 {
            Duration::from_secs(u64::from(control.increment.max(3)))
        } else {
            Duration::from_secs(u64::from(control.base))
        };
        Clock {
            control,
            left: [start; 2],
            moved: [false; 2],
            running: None,
        }
    }

    /// A clock going on from a game put aside: the time each side had left, whether it
    /// has moved (its first move is free), the side to move's clock running from `now`.
    pub fn resumed(
        control: TimeControl,
        left: [Option<Duration>; 2],
        to_move: Color,
        now: Instant,
    ) -> Clock {
        let mut c = Clock::new(control);
        for side in [Color::White, Color::Black] {
            if let Some(d) = left[idx(side)] {
                c.left[idx(side)] = d;
                c.moved[idx(side)] = true;
            }
        }
        c.running = Some((to_move, now));
        c
    }

    /// The time left of `c` at `now`.
    pub fn left(&self, c: Color, now: Instant) -> Duration {
        let base = self.left[idx(c)];
        match self.running {
            Some((r, since)) if r == c && self.moved[idx(c)] => base.saturating_sub(now - since),
            _ => base,
        }
    }

    /// `c` has moved: its time is taken (not on its first move), the increment added,
    /// the other side's clock starts. `spent`: the time to take instead of what passed
    /// (Maia's pause: the program's own computing is not her thinking).
    pub fn moved(&mut self, c: Color, now: Instant, spent: Option<Duration>) {
        let i = idx(c);
        if self.moved[i] {
            self.left[i] = match spent {
                Some(d) => self.left[i].saturating_sub(d),
                None => self.left(c, now),
            };
        }
        self.moved[i] = true;
        self.left[i] += Duration::from_secs(u64::from(self.control.increment));
        self.running = Some((!c, now));
    }

    /// A take-back: `side` moves now (its clock runs from `now`); `moved`: it has
    /// moved before in the game left (else its next move is its first, free one).
    pub fn take_back(&mut self, side: Color, moved: bool, now: Instant) {
        if let Some((c, since)) = self.running
            && self.moved[idx(c)]
        {
            self.left[idx(c)] = self.left[idx(c)].saturating_sub(now - since);
        }
        self.moved[idx(side)] = moved;
        self.running = Some((side, now));
    }

    /// The side to move, its clock running (after its first move).
    pub fn running(&self) -> Option<Color> {
        self.running.map(|(c, _)| c).filter(|c| self.moved[idx(*c)])
    }

    /// The clocks waited (a question about leaving was open): the running one is moved
    /// on by `d`, as if no time passed.
    pub fn pause(&mut self, d: Duration) {
        if let Some((_, since)) = self.running.as_mut() {
            *since += d;
        }
    }

    pub fn stop(&mut self, now: Instant) {
        if let Some((c, _)) = self.running {
            self.left[idx(c)] = self.left(c, now);
        }
        self.running = None;
    }

    /// The low-time threshold: min(60, max(10, base / 8)) seconds (scalachess).
    pub fn low(&self) -> Duration {
        Duration::from_secs(u64::from((self.control.base / 8).clamp(10, 60)))
    }
}

/// «4:58», «1:02:03», «0:09.4» (tenths under 10 s).
pub fn clock_text(d: Duration) -> String {
    let ms = d.as_millis();
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else if ms < 10_000 {
        format!("0:{:02}.{}", s, ms / 100 % 10)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// `[%clk 0:04:58]` for the PGN.
pub fn clk_command(d: Duration) -> String {
    let s = d.as_secs();
    format!("[%clk {}:{:02}:{:02}]", s / 3600, s / 60 % 60, s % 60)
}

/// The time each side had left in a game put aside (white, black; `None`: it has not
/// moved): the `[%clk]` of its last move among those played after `floor` half-moves;
/// the side to move's `to_move` when it is known (its time when the game was put aside).
pub fn times_left(
    game: &chess_core::Game,
    floor: usize,
    to_move: Option<Duration>,
) -> [Option<Duration>; 2] {
    let mut left = [None, None];
    let line = game.mainline();
    for &id in line.iter().skip(floor) {
        let mover = game.position_before(id).map(|p| p.turn());
        let clk = game
            .node(id)
            .and_then(|n| n.comments_after().iter().find_map(|c| clk_of(c)));
        if let (Some(m), Some(t)) = (mover, clk) {
            left[idx(m)] = Some(t);
        }
    }
    let end = line.last().copied().unwrap_or(chess_core::NodeId::ROOT);
    if let (Some(t), Some(p)) = (to_move, game.position(end)) {
        left[idx(p.turn())] = Some(t);
    }
    left
}

/// The game of a PGN text (a journal's: one game, UTF-8).
pub fn parse_game(pgn: &str) -> Option<chess_core::Game> {
    let never = std::sync::atomic::AtomicBool::new(false);
    let db = chess_core::PgnDatabase::from_bytes(
        pgn.as_bytes().to_vec(),
        Some(chess_core::TextEncoding::Utf8),
        &mut |_| {},
        &never,
    )
    .ok()?;
    db.load_game(0).map(|p| p.game)
}

/// The time a `[%clk h:mm:ss]` of a comment says.
pub fn clk_of(comment: &str) -> Option<Duration> {
    // The PGN writer may break the line after «[%clk».
    let rest = &comment[comment.find("[%clk")? + 5..];
    let text = &rest[..rest.find(']')?];
    let mut secs = 0u64;
    for part in text.trim().split(':') {
        // Seconds may have a fraction («0:03:12.4»): whole ones are enough.
        let whole = part.split('.').next()?.parse::<u64>().ok()?;
        secs = secs * 60 + whole;
    }
    Some(Duration::from_secs(secs))
}

// ----- the opponent's moves -------------------------------------------------------------

/// A request for Maia's move: the positions up to now, the ratings, the pause.
struct Request {
    generation: u64,
    positions: Vec<Chess>,
    self_elo: u32,
    oppo_elo: u32,
    temperature: f32,
    pause: Duration,
}

/// What computes the opponent's moves.
enum Worker {
    /// Maia in a thread of her own; her error, if loading failed.
    Maia {
        tx: Sender<Request>,
        rx: Receiver<(u64, Option<Move>, Duration)>,
        generation: u64,
        error: Arc<Mutex<Option<String>>>,
    },
    /// Rodent, a UCI engine (the shared crate's player).
    Rodent(::play::rodent::RodentPlayer),
}

/// The engine in a thread of its own: loads Maia once (or starts Rodent), answers
/// requests after a pause. An answer for an older request (after a take-back, a
/// resignation) is dropped.
pub struct Opponent {
    worker: Worker,
    pub thinking: bool,
}

impl Opponent {
    pub fn start(weights: std::path::PathBuf, ctx: &egui::Context) -> Opponent {
        let (tx, req_rx) = channel::<Request>();
        let (ans_tx, rx) = channel();
        let error = Arc::new(Mutex::new(None));
        let err = error.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("maia".into())
            .spawn(move || {
                let maia = match maia::Maia::load(&weights) {
                    Ok(m) => m,
                    Err(e) => {
                        if let Ok(mut g) = err.lock() {
                            *g = Some(e.to_string());
                        }
                        ctx.request_repaint();
                        return;
                    }
                };
                while let Ok(mut r) = req_rx.recv() {
                    // Only the newest request counts.
                    while let Ok(newer) = req_rx.try_recv() {
                        r = newer;
                    }
                    let started = Instant::now();
                    let p = maia.predict(&r.positions, r.self_elo, r.oppo_elo);
                    let m = maia::strength::choose(&p, r.temperature, random_unit());
                    let rest = r.pause.saturating_sub(started.elapsed());
                    std::thread::sleep(rest);
                    // Her clock: the pause, or the computing when it took longer (a slow
                    // computer, the large model).
                    let spent = r.pause.max(started.elapsed());
                    if ans_tx.send((r.generation, m, spent)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        Opponent {
            worker: Worker::Maia {
                tx,
                rx,
                generation: 0,
                error,
            },
            thinking: false,
        }
    }

    /// Asks for a move in the last of `positions` (`engine_pos`: the same for a UCI
    /// engine).
    pub fn ask(
        &mut self,
        positions: Vec<Chess>,
        engine_pos: Option<uci::EnginePosition>,
        (self_elo, temperature): (u32, f32),
        oppo_elo: u32,
        pause: Duration,
    ) {
        self.thinking = true;
        match &mut self.worker {
            Worker::Maia { tx, generation, .. } => {
                *generation += 1;
                let _ = tx.send(Request {
                    generation: *generation,
                    positions,
                    self_elo,
                    oppo_elo,
                    temperature,
                    pause,
                });
            }
            Worker::Rodent(player) => {
                if let (Some(pos), Some(now)) = (engine_pos, positions.last().cloned()) {
                    player.ask(pos, now, pause);
                }
            }
        }
    }

    /// Rodent (`exe` in its folder) playing in `style` at `uci_elo` (none: full
    /// strength), with its opening book.
    pub fn start_rodent(
        exe: std::path::PathBuf,
        style: String,
        uci_elo: Option<u32>,
        ctx: &egui::Context,
    ) -> Opponent {
        let config = uci::EngineConfig {
            working_dir: exe.parent().map(|d| d.to_path_buf()),
            ..uci::EngineConfig::new(&exe)
        };
        let ctx = ctx.clone();
        let player = ::play::rodent::RodentPlayer::start(
            config,
            &style,
            uci_elo,
            true,
            Box::new(move || ctx.request_repaint()),
        );
        Opponent {
            worker: Worker::Rodent(player),
            thinking: false,
        }
    }

    /// Forgets what was asked (its answer will be dropped).
    pub fn forget(&mut self) {
        match &mut self.worker {
            Worker::Maia { generation, .. } => *generation += 1,
            Worker::Rodent(player) => player.forget(),
        }
        self.thinking = false;
    }

    /// The move asked for last, when it has come, with the time it took (its pause, or
    /// the computing when longer).
    pub fn answer(&mut self) -> Option<(Move, Duration)> {
        let mut out = None;
        match &mut self.worker {
            Worker::Maia { rx, generation, .. } => {
                while let Ok((g, m, pause)) = rx.try_recv() {
                    if g == *generation {
                        self.thinking = false;
                        out = m.map(|m| (m, pause));
                    }
                }
            }
            Worker::Rodent(player) => {
                if let Some((m, pause)) = player.answer() {
                    self.thinking = false;
                    out = m.map(|m| (m, pause));
                }
            }
        }
        out
    }

    pub fn failed(&self) -> Option<String> {
        use ::play::rodent::Failure;
        match &self.worker {
            Worker::Maia { error, .. } => error.lock().ok().and_then(|g| g.clone()),
            Worker::Rodent(player) => player.failure().map(|f| match f {
                Failure::Start(e) => start_failure(&e),
                Failure::Stopped => tr("it stopped while playing").to_owned(),
                Failure::Search(e) => e,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::shakmaty::uci::UciMove;

    #[test]
    fn the_time_of_a_clk_comment() {
        assert_eq!(clk_of("[%clk 0:03:12]"), Some(Duration::from_secs(192)));
        assert_eq!(
            clk_of("Good move [%clk 1:00:05.3] [%cal Ge2e4]"),
            Some(Duration::from_secs(3605))
        );
        assert_eq!(
            clk_of(&clk_command(Duration::from_secs(4321))),
            Some(Duration::from_secs(4321))
        );
        assert_eq!(clk_of("[%clk\n0:04:11]"), Some(Duration::from_secs(251)));
        assert_eq!(clk_of("no clock"), None);
    }

    #[test]
    fn a_clock_goes_on_from_the_times_left() {
        let now = Instant::now();
        let tc = TimeControl {
            base: 300,
            increment: 3,
        };
        let c = Clock::resumed(
            tc,
            [Some(Duration::from_secs(250)), None],
            Color::Black,
            now,
        );
        assert_eq!(c.left(Color::White, now), Duration::from_secs(250));
        // Black has not moved: its first move is free, its time the base.
        assert_eq!(
            c.left(Color::Black, now + Duration::from_secs(9)),
            Duration::from_secs(300)
        );
        assert_eq!(c.running(), None);
        // Both have moved: the side to move's clock runs from the moment it goes on.
        let c = Clock::resumed(
            tc,
            [
                Some(Duration::from_secs(250)),
                Some(Duration::from_secs(200)),
            ],
            Color::White,
            now,
        );
        assert_eq!(c.running(), Some(Color::White));
        assert_eq!(
            c.left(Color::White, now + Duration::from_secs(10)),
            Duration::from_secs(240)
        );
        assert_eq!(
            c.left(Color::Black, now + Duration::from_secs(10)),
            Duration::from_secs(200)
        );
    }

    #[test]
    fn speeds_as_lichess() {
        let s = |base, increment| TimeControl { base, increment }.speed();
        assert_eq!(s(180, 0), Speed::Blitz);
        assert_eq!(s(300, 3), Speed::Blitz);
        assert_eq!(s(420, 0), Speed::Blitz);
        assert_eq!(s(480, 0), Speed::Rapid);
        assert_eq!(s(900, 10), Speed::Rapid);
        assert_eq!(s(1500, 0), Speed::Classical);
        assert_eq!(
            TimeControl {
                base: 300,
                increment: 3
            }
            .label(),
            "5+3"
        );
        // Nothing faster than 3+0; every preset and custom minute allowed.
        assert!(
            !TimeControl {
                base: 120,
                increment: 1
            }
            .allowed()
        );
        assert!(PRESETS.iter().all(|t| t.allowed()));
        assert!(CUSTOM_BASES.iter().all(|&b| {
            TimeControl {
                base: b,
                increment: 0,
            }
            .allowed()
        }));
        assert_eq!(
            PlayPrefs::default().clock.map(|t| t.label()).as_deref(),
            Some("10+5")
        );
        assert_eq!(
            TimeControl {
                base: 300,
                increment: 3
            }
            .pgn(),
            "300+3"
        );
    }

    #[test]
    fn clock_runs_after_first_moves() {
        let mut c = Clock::new(TimeControl {
            base: 60,
            increment: 2,
        });
        let t0 = Instant::now();
        c.moved(Color::White, t0, None); // the first move takes nothing
        assert_eq!(c.left(Color::White, t0), Duration::from_secs(62));
        // Black's first move: no time taken either.
        c.moved(Color::Black, t0 + Duration::from_secs(5), None);
        assert_eq!(
            c.left(Color::Black, t0 + Duration::from_secs(5)),
            Duration::from_secs(62)
        );
        // White thinks 10 s.
        let t1 = t0 + Duration::from_secs(15);
        assert_eq!(c.left(Color::White, t1), Duration::from_secs(52));
        c.moved(Color::White, t1, None);
        assert_eq!(c.left(Color::White, t1), Duration::from_secs(54));
        assert_eq!(c.running(), Some(Color::Black));
        assert_eq!(clock_text(Duration::from_millis(9_450)), "0:09.4");
        assert_eq!(clock_text(Duration::from_secs(298)), "4:58");
        assert_eq!(clock_text(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(clk_command(Duration::from_secs(298)), "[%clk 0:04:58]");
        assert_eq!(
            Clock::new(TimeControl {
                base: 180,
                increment: 0
            })
            .low(),
            Duration::from_secs(22)
        );
    }

    fn play(moves: &[&str]) -> Vec<Chess> {
        let mut pos = Chess::default();
        let mut out = vec![pos.clone()];
        for m in moves {
            let m = m.parse::<UciMove>().unwrap().to_move(&pos).unwrap();
            pos.play_unchecked(m);
            out.push(pos.clone());
        }
        out
    }

    #[test]
    fn endings() {
        let mate = play(&["f2f3", "e7e5", "g2g4", "d8h4"]);
        assert_eq!(
            ending(&mate),
            Some(Ending::Checkmate {
                winner: Color::Black
            })
        );
        let rep = play(&[
            "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
        ]);
        assert_eq!(ending(&rep), Some(Ending::Threefold));
        assert_eq!(ending(&rep[..rep.len() - 1]), None);
        assert_eq!(ending(&play(&["e2e4"])), None);
        assert_eq!(
            Ending::Time {
                loser: Color::White,
                draw: false
            }
            .result(),
            chess_core::GameResult::BlackWins
        );
    }

    #[test]
    fn pauses_never_lose_on_time() {
        // Whole games of 300 moves at 1+0, 3+0, 5+0 with 0.1 s of computing per move.
        for base in [60.0f32, 180.0, 300.0] {
            for _ in 0..20 {
                let mut left = base;
                for ply in 0..300 {
                    let p = pause(Some((Duration::from_secs_f32(left), 0)), ply * 2, false);
                    left -= p.as_secs_f32();
                    assert!(left > 0.0, "flagged at {ply} with {base}");
                }
            }
        }
    }
}

// ----- a game being played ------------------------------------------------------------

/// A game against Maia in progress (lives in the game screen while it is played).
pub struct Session {
    /// The game kept for the player's database (none: not written by itself).
    pub journal: Option<crate::player_games::Journal>,
    pub user: Color,
    /// The opponent's strength (Lichess blitz scale), what plays it and in which style.
    pub strength: u32,
    pub engine: Engine,
    pub style: String,
    pub user_elo: Option<u32>,
    pub clock: Option<Clock>,
    pub opponent: Opponent,
    /// «Resign?» waits for the second click.
    pub resign_asked: bool,
    /// The player's clock got low and was heard (once a game).
    pub low_time_heard: bool,
    /// The engine judging the game for Maia's resignation (none without an engine).
    pub judge: Option<Judge>,
    /// Maia's turns in a row with her position hopeless.
    pub hopeless: u32,
    /// Maia offers to resign: since when the offer is open (the clocks wait).
    pub resign_offer: Option<Instant>,
    /// «Play to the end»: no more offers in this game.
    pub play_on: bool,
    /// The last move of the game it goes on from (`ROOT`: a game from the start): the
    /// moves up to it are not taken back and do not count as played.
    pub floor: chess_core::NodeId,
}

/// What a game against the computer is played with, kept in its journal so that it can
/// go on later (a game put aside, docs/design/desktop-adjourned.md).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub white: bool,
    pub strength: u32,
    pub rival: Rival,
    pub style: String,
    pub user_elo: Option<u32>,
    pub user_name: String,
    pub clock: Option<TimeControl>,
    /// Half-moves of the game it went on from (not played here: not taken back).
    pub floor: usize,
    /// That game and its move, as the note and a rematch name them.
    pub from_game: Option<String>,
    pub from_after: Option<String>,
    pub from_after_shown: Option<String>,
}

impl Saved {
    /// The start of the game again, with the position it went on from (rebuilt from the
    /// game itself) for a rematch.
    pub fn start(&self, from: Option<StartFrom>) -> Start {
        Start {
            user: if self.white {
                Color::White
            } else {
                Color::Black
            },
            strength: self.strength,
            rival: self.rival,
            style: self.style.clone(),
            user_elo: self.user_elo,
            user_name: self.user_name.clone(),
            clock: self.clock,
            from: from.map(|mut f| {
                f.game = self.from_game.clone();
                f.after = self.from_after.clone();
                f.after_shown = self.from_after_shown.clone();
                Box::new(f)
            }),
        }
    }
}

/// What the play dialog starts.
#[derive(Clone, Debug)]
pub struct Start {
    pub user: Color,
    pub strength: u32,
    pub rival: Rival,
    /// Rodent's style (ignored for Maia).
    pub style: String,
    pub user_elo: Option<u32>,
    pub user_name: String,
    pub clock: Option<TimeControl>,
    /// A game from a position of another game (docs/design/play-from-position.md).
    pub from: Option<Box<StartFrom>>,
}

impl Start {
    /// What the game is played with, for its journal; `floor`: half-moves it went on from.
    pub fn saved(&self, floor: usize) -> Saved {
        Saved {
            white: self.user == Color::White,
            strength: self.strength,
            rival: self.rival,
            style: self.style.clone(),
            user_elo: self.user_elo,
            user_name: self.user_name.clone(),
            clock: self.clock,
            floor,
            from_game: self.from.as_ref().and_then(|f| f.game.clone()),
            from_after: self.from.as_ref().and_then(|f| f.after.clone()),
            from_after_shown: self.from.as_ref().and_then(|f| f.after_shown.clone()),
        }
    }

    /// The opponent as the game names it: «Maia 1500», «Rodent V 2400 (Tal)».
    pub fn rival_name(&self) -> String {
        rival_name(self.rival, self.strength, &self.style)
    }
}

/// A text about the opponent in the app's language: Maia's, or Rodent's (their own
/// sentences: in Russian and other languages the words agree with her or him).
pub fn said(rival: Rival, maia: &'static str, rodent: &'static str) -> &'static str {
    tr(match rival {
        Rival::Maia => maia,
        Rival::Rodent => rodent,
    })
}

/// What plays a game: a Maia model, or Rodent V.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Maia(Model),
    Rodent(Gen),
}

impl Engine {
    /// Maia's rating and temperature for `strength`: the rating itself, her moves drawn
    /// as she predicts them (Rodent takes its strength as UCI_Elo when it starts).
    pub fn input(self, strength: u32) -> (u32, f32) {
        match self {
            Engine::Maia(_) => (strength, 1.0),
            Engine::Rodent(_) => (0, 0.0),
        }
    }
}

/// The opponent of `start`, ready to play: its engine and file; `None`: it has to be
/// downloaded first (the dialog offers it).
pub fn engine_for(start: &Start) -> Option<(Engine, std::path::PathBuf)> {
    match start.rival {
        Rival::Maia => model_for().ok().map(|(m, p)| (Engine::Maia(m), p)),
        Rival::Rodent => {
            let g = style(&start.style).rodent;
            g.installed().map(|p| (Engine::Rodent(g), p))
        }
    }
}

/// Dev hook `CP_RIVAL=rodent[:style]`: the opponent of a game started by a hook.
pub fn debug_rival() -> (Rival, String) {
    match std::env::var("CP_RIVAL")
        .ok()
        .as_deref()
        .map(|v| v.split_once(':').unwrap_or((v, STYLES[0].id)))
    {
        Some(("rodent", style)) => (Rival::Rodent, style.to_owned()),
        _ => (Rival::Maia, STYLES[0].id.to_owned()),
    }
}

/// «Maia 1500», «Rodent V 2400», «Rodent V 2400 (Tal)», «Rodent IV 2000 (Petrosian)»
/// (the default style unnamed).
pub fn rival_name(rival: Rival, strength: u32, style_id: &str) -> String {
    match rival {
        Rival::Maia => format!("Maia {strength}"),
        Rival::Rodent => ::play::rodent::name(strength, style_id),
    }
}

impl Session {
    pub fn rival(&self) -> Rival {
        match self.engine {
            Engine::Maia(_) => Rival::Maia,
            Engine::Rodent(_) => Rival::Rodent,
        }
    }

    /// The opponent on its bar: «Maia 1500», «Rodent V 2400».
    pub fn rival_label(&self) -> String {
        match self.engine {
            Engine::Maia(_) => format!("Maia {}", self.strength),
            Engine::Rodent(g) => format!("{} {}", g.name(), self.strength),
        }
    }

    /// Rodent's style, under his name on the bar (none for Maia and the default style).
    pub fn rival_style(&self) -> Option<&'static str> {
        (self.rival() == Rival::Rodent && self.style != STYLES[0].id)
            .then(|| tr(style(&self.style).title()))
    }
}

/// Where a game against Maia starts when it goes on from a game: its start and the
/// moves to the chosen one (the history Maia and the repetitions need).
#[derive(Clone, Debug)]
pub struct StartFrom {
    /// The FEN of the game's start (`None`: the standard position) — after the last
    /// null move of the path, if there is one.
    pub start_fen: Option<String>,
    /// The moves from that start to the chosen position.
    pub moves: Vec<chess_core::shakmaty::Move>,
    /// The position played from.
    pub position: Chess,
    /// Its last move, for the board (from, to).
    pub last: Option<(chess_core::shakmaty::Square, chess_core::shakmaty::Square)>,
    /// «14...Ke7» — the move played from (SAN, as data); `None`: the game's start.
    pub after: Option<String>,
    /// The same as shown («14…Ке7»).
    pub after_shown: Option<String>,
    /// «Carlsen — Caruana, Wijk aan Zee 2024», when the game says so.
    pub game: Option<String>,
    /// Null moves before the position were left out.
    pub null_dropped: bool,
}

// ----- the dialog ---------------------------------------------------------------------

/// «Play against the computer»: name and rating, colour, time, strength.
pub struct Dialog {
    name: String,
    rating: String,
    prefs: PlayPrefs,
    custom: bool,
    download: Option<crate::maia_download::Download>,
    /// Rodent's download (docs/design/rodent-opponent.md §6): which one, and how it goes.
    rodent_download: Option<(Gen, crate::engine_download::Download)>,
    /// «Download and play» was pressed: the game starts once Maia is there.
    play_after: bool,
    /// The game goes on from a position of another game.
    from: Option<StartFrom>,
    /// The colour chosen is this game's, not a habit to remember (from a position, a
    /// rematch).
    keep_color: bool,
    focus: bool,
    /// The number of games of the players' databases read so far (the app fills it).
    pub counts: std::collections::HashMap<std::path::PathBuf, usize>,
    /// Whether the database the games go to is there (the app tells, from the files as
    /// last found: no disk in the frame).
    pub target_there: bool,
    /// Where the games of the name typed go: for that name (found once per name).
    target: Option<(String, std::path::PathBuf)>,
}

pub enum DialogOutcome {
    Play(Start),
    Cancel,
}

impl Dialog {
    pub fn new(settings: &crate::settings::Settings) -> Dialog {
        let mut prefs = settings.play.clone();
        // A bullet control remembered from before: the default instead.
        if prefs.clock.is_some_and(|c| !c.allowed()) {
            prefs.clock = PlayPrefs::default().clock;
        }
        // A remembered strength the opponent does not play (another build's range, an
        // edited file): to the nearest one that is played.
        // A style whose Rodent is not for this system: one that is, else Maia.
        if prefs.rival == Rival::Rodent && !style(&prefs.style).rodent.available() {
            match STYLES.iter().find(|s| s.rodent.available()) {
                Some(s) => prefs.style = s.id.to_owned(),
                None => prefs.rival = Rival::Maia,
            }
        }
        let (min, max) = range(prefs.rival, &prefs.style);
        prefs.strength = prefs.strength.clamp(min, max);
        let custom = prefs.clock.is_some_and(|c| !PRESETS.contains(&c));
        Dialog {
            name: settings.your_name.clone(),
            rating: settings.rating.map(|r| r.to_string()).unwrap_or_default(),
            prefs,
            custom,
            download: None,
            rodent_download: None,
            play_after: false,
            focus: true,
            from: None,
            keep_color: false,
            counts: Default::default(),
            target_there: false,
            target: None,
        }
    }

    /// The dialog for a game from a position: the side to move plays (not remembered).
    pub fn new_from(settings: &crate::settings::Settings, from: StartFrom) -> Dialog {
        let mut d = Dialog::new(settings);
        d.prefs.color = match from.position.turn() {
            Color::White => ColorChoice::White,
            Color::Black => ColorChoice::Black,
        };
        d.from = Some(from);
        d.keep_color = true;
        d
    }

    /// The dialog again for a game that could not start (its Maia missing): the same
    /// position, strength, colour and time.
    pub fn for_start(settings: &crate::settings::Settings, start: &Start) -> Dialog {
        let mut d = match &start.from {
            Some(from) => Dialog::new_from(settings, (**from).clone()),
            None => Dialog::new(settings),
        };
        d.prefs.strength = start.strength;
        d.prefs.rival = start.rival;
        d.prefs.style = start.style.clone();
        d.prefs.clock = start.clock;
        d.custom = start.clock.is_some_and(|c| !PRESETS.contains(&c));
        d.prefs.color = match start.user {
            Color::White => ColorChoice::White,
            Color::Black => ColorChoice::Black,
        };
        d.keep_color = true;
        d
    }

    /// Where the games of the name typed go (player-games.md §3): said before the game.
    fn record_line(&mut self, settings: &crate::settings::Settings) -> String {
        use crate::i18n::trf;
        if !settings.record_games {
            return tr(
                "Games are not written by themselves (Settings → Interface → Your databases).",
            )
            .to_owned();
        }
        let name = crate::player_games::tidy(&self.name);
        if self.target.as_ref().is_none_or(|(n, _)| *n != name) {
            let dir = crate::own_db::default_base_dir(settings.new_database_dir.as_deref());
            let path = crate::player_games::target(&settings.players, &name, &dir);
            self.target = Some((name.clone(), path));
        }
        let path = &self.target.as_ref().expect("target").1;
        let db = crate::own_db::base_name(path);
        if name.is_empty() {
            return trf(
                "Without a name games go to «{db}». Enter a name to have a database of your own.",
                &[("db", &db)],
            );
        }
        if !self.target_there {
            return trf(
                "Games will be written to a new database «{db}».",
                &[("db", &db)],
            );
        }
        match self.counts.get(path) {
            Some(&n) => trf(
                "Games are written to «{db}» · {n}",
                &[("db", &db), ("n", &crate::i18n::games(n))],
            ),
            None => trf("Games are written to «{db}»", &[("db", &db)]),
        }
    }

    /// The database the games of the name typed go to.
    pub fn record_target(&self) -> Option<&std::path::Path> {
        self.target.as_ref().map(|(_, p)| p.as_path())
    }

    fn rating(&self) -> Option<u32> {
        self.rating
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|r| (100..=3500).contains(r))
    }

    /// Draws the dialog; on «Play» the choices are written into `settings`.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut crate::settings::Settings,
    ) -> Option<DialogOutcome> {
        use egui::RichText;
        let mut out = None;
        let rival_before = self.prefs.rival;
        let model = model_for();
        let body_h = (ctx.content_rect().height() - 140.0).max(160.0);
        egui::Modal::new(egui::Id::new("play_dialog")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("Play against the computer"));
            // The body scrolls in a low window; the buttons below it stay in view.
            // A solid bar, seen without hovering: in a low window the time is below it.
            ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
            let rating = egui::ScrollArea::vertical()
                .max_height(body_h)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
            // A game from a position: that position, instead of the introduction.
            if let Some(from) = &self.from {
                position_block(ui, settings, from, self.prefs.color, body_h >= 420.0);
            }

            // The blocks in the order of the choice: who plays, against whom, which
            // colour, how long. Every choice is a framed tile; the tiles of a row share
            // its width.
            const W: f32 = 460.0;
            const GAP: f32 = 6.0;
            const TILE_H: f32 = 32.0;
            let section = |ui: &mut egui::Ui, text: &str| {
                ui.add_space(12.0);
                ui.label(RichText::new(text).strong().size(15.0));
                ui.add_space(2.0);
            };
            let choice = |ui: &mut egui::Ui, on: bool, text: &str, size: [f32; 2]| {
                ui.add_sized(
                    size,
                    egui::Button::selectable(on, RichText::new(text).size(14.0))
                        .frame_when_inactive(true),
                )
            };
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);

            // You.
            section(ui, tr("You"));
            egui::Grid::new("play_you")
                .num_columns(2)
                .spacing(egui::vec2(8.0, GAP))
                .show(ui, |ui| {
                    ui.label(tr("Name:"));
                    ui.horizontal(|ui| {
                        // The players of this computer, for a family (each their database
                        // and rating).
                        let known: Vec<_> = settings
                            .players
                            .iter()
                            .filter(|p| !p.name.is_empty())
                            .cloned()
                            .collect();
                        let menu_w = if known.is_empty() { 0.0 } else { 34.0 };
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.name)
                                .desired_width(ui.available_width() - menu_w),
                        );
                        if std::mem::take(&mut self.focus) && self.name.is_empty() {
                            r.request_focus();
                        }
                        let mut pick = None;
                        // ↓ / ↑ in the field go through the players.
                        if r.has_focus() && !known.is_empty() {
                            let step = ui.input_mut(|i| {
                                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                    1
                                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                    -1
                                } else {
                                    0
                                }
                            });
                            if step != 0 {
                                let n = known.len() as i32;
                                let now = crate::player_games::tidy(&self.name).to_lowercase();
                                let at = known
                                    .iter()
                                    .position(|p| p.name.to_lowercase() == now)
                                    .map_or(if step > 0 { -1 } else { n }, |k| k as i32);
                                pick = Some(known[(at + step).rem_euclid(n) as usize].clone());
                            }
                        }
                        if !known.is_empty() {
                            let size = egui::vec2(menu_w - ui.spacing().item_spacing.x, r.rect.height());
                            let (resp, _) = egui::containers::menu::MenuButton::from_button(
                                egui::Button::new("▾").min_size(size),
                            )
                            .ui(ui, |ui| {
                                for p in &known {
                                    let mut label = p.name.clone();
                                    if let Some(r) = p.rating {
                                        label += &format!(" · {r}");
                                    }
                                    if let Some(n) = p.path.as_ref().and_then(|q| self.counts.get(q)) {
                                        label += &format!(" · {}", crate::i18n::games(*n));
                                    }
                                    if ui.button(label).clicked() {
                                        pick = Some(p.clone());
                                        ui.close();
                                    }
                                }
                            });
                            resp.on_hover_text(tr("Players of this computer (↓ in the name field)"));
                        }
                        if let Some(p) = pick {
                            self.name = p.name.clone();
                            self.rating = p.rating.map(|r| r.to_string()).unwrap_or_default();
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    // As wide as the field: the line wraps, the dialog keeps its width.
                    let line = self.record_line(settings);
                    ui.scope(|ui| {
                        ui.set_max_width(W - 80.0);
                        ui.add(egui::Label::new(RichText::new(line).weak().size(12.5)).wrap());
                    });
                    ui.end_row();
                    ui.label(tr("Rating:"));
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.rating)
                                .desired_width(70.0),
                        );
                        if r.changed() {
                            self.rating.retain(|c| c.is_ascii_digit());
                            self.rating.truncate(4);
                        }
                        ui.label(RichText::new(tr("not required")).weak());
                    });
                    ui.end_row();
                });

            // The opponent (docs/design/rodent-opponent.md): Maia or Rodent V, one strength
            // slider for both, Rodent's style.
            let rodent_ok = Gen::V.available() || Gen::IV.available();
            let rodent = self.prefs.rival == Rival::Rodent;
            section(
                ui,
                &if rodent {
                    crate::i18n::trf("Opponent — Rodent {elo}", &[("elo", &self.prefs.strength)])
                } else {
                    crate::i18n::trf("Opponent — Maia {elo}", &[("elo", &self.prefs.strength)])
                },
            );
            ui.horizontal(|ui| {
                let tile_w = (W - GAP) / 2.0;
                // A selectable tile of two centred lines: the name and what it is.
                let tile = |ui: &mut egui::Ui, on: bool, enabled: bool, name: &str, about: &str| {
                    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(tile_w, 48.0), sense);
                    let v = ui.style().interact_selectable(&resp, on);
                    let fill = if on { ui.visuals().selection.bg_fill } else { v.weak_bg_fill };
                    ui.painter().rect(rect, v.corner_radius, fill, v.bg_stroke, egui::StrokeKind::Inside);
                    let (text, weak) = if enabled {
                        if on {
                            // On the selected fill the caption takes its text colour, softer.
                            let c = ui.visuals().selection.stroke.color;
                            (c, c.gamma_multiply(0.8))
                        } else {
                            (v.text_color(), ui.visuals().weak_text_color())
                        }
                    } else {
                        let c = ui.visuals().weak_text_color();
                        (c, c)
                    };
                    let width = tile_w - 12.0;
                    let line = |t: &str, size: f32, color| {
                        ui.painter().layout(t.to_owned(), egui::FontId::proportional(size), color, width)
                    };
                    let (top, bottom) = (line(name, 14.0, text), line(about, 11.5, weak));
                    let h = top.size().y + bottom.size().y;
                    let y = rect.center().y - h / 2.0;
                    ui.painter().galley(egui::pos2(rect.center().x - top.size().x / 2.0, y), top, text);
                    let y2 = y + h - bottom.size().y;
                    ui.painter().galley(egui::pos2(rect.center().x - bottom.size().x / 2.0, y2), bottom, weak);
                    resp
                };
                if tile(ui, !rodent, true, "Maia", tr("plays like a human")).clicked() && rodent {
                    self.prefs.rival = Rival::Maia;
                    self.prefs.strength = self.prefs.strength.clamp(MAIA_MIN, MAIA_MAX);
                }
                let about = if rodent_ok {
                    tr("strong, with a style")
                } else if cfg!(target_os = "macos") {
                    tr("not yet for macOS")
                } else {
                    tr("not yet for this system")
                };
                let r = tile(ui, rodent, rodent_ok, "Rodent", about).on_hover_text(if rodent_ok { "" } else { tr(
                    "Rodent is not available for this system yet. When it is, it will be here.",
                ) });
                if r.clicked() && !rodent {
                    self.prefs.rival = Rival::Rodent;
                    if !style(&self.prefs.style).rodent.available()
                        && let Some(st) = STYLES.iter().find(|s| s.rodent.available())
                    {
                        self.prefs.style = st.id.to_owned();
                    }
                    let (min, max) = range(Rival::Rodent, &self.prefs.style);
                    self.prefs.strength = self.prefs.strength.clamp(min, max);
                }
            });
            // Each engine on its own scale (its authors'): not a person's Elo, said under it.
            let (min, max) = range(self.prefs.rival, &self.prefs.style);
            ui.horizontal(|ui| {
                let end = |ui: &mut egui::Ui, v: u32| {
                    ui.add_sized([34.0, 18.0], egui::Label::new(RichText::new(v.to_string()).weak()))
                };
                end(ui, min);
                ui.spacing_mut().slider_width = W - 2.0 * 34.0 - 2.0 * GAP;
                ui.add(
                    egui::Slider::new(&mut self.prefs.strength, min..=max)
                        .step_by(50.0)
                        .show_value(false),
                );
                end(ui, max);
            });
            ui.add_space(4.0);
            let note = if self.prefs.rival == Rival::Rodent {
                tr("Rodent's own rating (UCI_Elo): it does not match a person's Elo.")
            } else {
                tr("Maia's own rating: it does not match a person's Elo.")
            };
            ui.add(egui::Label::new(RichText::new(note).weak().size(11.5)).wrap());
            // Rodent's style: always in its place (greyed for Maia), so nothing jumps.
            ui.add_space(4.0);
            ui.add_enabled_ui(rodent, |ui| {
                // As tall as the buttons around it.
                ui.spacing_mut().interact_size.y = 32.0;
                let current = style(&self.prefs.style);
                let shown = if rodent {
                    style_label(current)
                } else {
                    tr("Like a human").to_owned()
                };
                egui::ComboBox::from_id_salt("rodent_style")
                    .width(W)
                    .height(400.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        // Rodent V's styles, Rodent IV's characters, then the players both
                        // imitate; a style whose Rodent is not for this system is greyed.
                        let groups = [
                            (tr("Style"), false, Some(Gen::V)),
                            (tr("Character"), false, Some(Gen::IV)),
                            (tr("As a player"), true, None),
                        ];
                        for (k, (head, player, only)) in groups.into_iter().enumerate() {
                            // Only what this system plays, in the order of the names as shown
                            // (the Tals side by side).
                            let mut group: Vec<&Style> = STYLES
                                .iter()
                                .filter(|st| st.player == player && only.is_none_or(|g| st.rodent == g) && st.rodent.available())
                                .collect();
                            if group.is_empty() {
                                continue;
                            }
                            if player {
                                group.sort_by_key(|st| style_label(st).to_lowercase());
                            }
                            if k > 0 {
                                ui.separator();
                            }
                            ui.label(RichText::new(head).weak().size(11.5));
                            for st in group {
                                ui.selectable_value(&mut self.prefs.style, st.id.to_owned(), style_label(st))
                                    .on_hover_text(tr(st.about));
                            }
                        }
                    })
                    .response
                    .on_disabled_hover_text(tr("The style is chosen for Rodent"));
            });
            // Another Rodent may play another range.
            if rodent {
                let (min, max) = range(self.prefs.rival, &self.prefs.style);
                self.prefs.strength = self.prefs.strength.clamp(min, max);
            }
            let about = if rodent {
                tr(style(&self.prefs.style).about).to_owned()
            } else {
                tr("Maia does not pick a style: she plays like people of her rating.").to_owned()
            };
            ui.add(egui::Label::new(RichText::new(about).weak().size(11.5)).truncate());
            let rating = self.rating();

            // Colour.
            section(ui, tr("Your colour"));
            ui.horizontal(|ui| {
                for (c, label) in [
                    (ColorChoice::White, tr("♔ White")),
                    (ColorChoice::Random, tr("Random")),
                    (ColorChoice::Black, tr("♚ Black")),
                ] {
                    if choice(ui, self.prefs.color == c, label, [(W - 2.0 * GAP) / 3.0, TILE_H]).clicked() {
                        self.prefs.color = c;
                    }
                }
            });

            // Time: the presets four in a row, then «Custom» and «Unlimited».
            section(ui, tr("Time"));
            let w = (W - 3.0 * GAP) / 4.0;
            let (rows, last) = PRESETS.split_at(8);
            for row in rows.chunks(4).chain([last]) {
                ui.horizontal(|ui| {
                    for tc in row {
                        let on = !self.custom && self.prefs.clock == Some(*tc);
                        if choice(ui, on, &tc.label(), [w, TILE_H]).clicked() {
                            self.prefs.clock = Some(*tc);
                            self.custom = false;
                        }
                    }
                    if row.len() < 4 {
                        if choice(ui, self.custom, tr("Custom"), [w, TILE_H]).clicked() {
                            self.custom = true;
                            if self.prefs.clock.is_none() {
                                self.prefs.clock = Some(TimeControl { base: 600, increment: 0 });
                            }
                        }
                        // No clock at all: as long as you like (Maia answers in a second
                        // or two).
                        let none = self.prefs.clock.is_none();
                        if choice(ui, none, tr("Unlimited"), [2.0 * w + GAP, TILE_H])
                            .on_hover_text(tr("No clocks: think as long as you like"))
                            .clicked()
                        {
                            self.prefs.clock = None;
                            self.custom = false;
                        }
                    }
                });
            }
            if self.custom
                && let Some(tc) = self.prefs.clock.as_mut()
            {
                let mut b = CUSTOM_BASES.iter().position(|&x| x >= tc.base).unwrap_or(7);
                let mut i = CUSTOM_INCREMENTS
                    .iter()
                    .position(|&x| x >= tc.increment)
                    .unwrap_or(0);
                ui.add_space(4.0);
                egui::Grid::new("play_custom")
                    .num_columns(3)
                    .spacing(egui::vec2(10.0, GAP))
                    .show(ui, |ui| {
                        ui.spacing_mut().slider_width = W - 90.0 - 28.0 - 20.0;
                        // Both labels flush left in one column.
                        let label = |ui: &mut egui::Ui, text: &str| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(90.0, 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(90.0);
                                    ui.label(text);
                                },
                            );
                        };
                        label(ui, tr("Minutes:"));
                        ui.add(egui::Slider::new(&mut b, 0..=CUSTOM_BASES.len() - 1).show_value(false));
                        ui.add_sized([28.0, 18.0], egui::Label::new(RichText::new((CUSTOM_BASES[b] / 60).to_string()).strong()));
                        ui.end_row();
                        label(ui, tr("Increment, s:"));
                        ui.add(egui::Slider::new(&mut i, 0..=CUSTOM_INCREMENTS.len() - 1).show_value(false));
                        ui.add_sized([28.0, 18.0], egui::Label::new(RichText::new(CUSTOM_INCREMENTS[i].to_string()).strong()));
                        ui.end_row();
                    });
                tc.base = CUSTOM_BASES[b];
                tc.increment = CUSTOM_INCREMENTS[i];
            }
            rating
                })
                .inner;

            // The opponent's file, downloaded once: under the body, by the buttons, always
            // seen. The line stays while either Maia's large model or Rodent is missing,
            // so that switching the opponent does not change the dialog's height; it is
            // about the opponent chosen.
            // The opponent changed: a game asked for by «Download and play» is not started
            // for the other one when its file comes.
            if self.prefs.rival != rival_before {
                self.play_after = false;
            }
            let rodent = self.prefs.rival == Rival::Rodent;
            let stage = self.download.as_ref().map(|d| d.stage());
            // The Rodent of the style chosen: its download, if that one is going on.
            let which = style(&self.prefs.style).rodent;
            let rstage = self
                .rodent_download
                .as_ref()
                .filter(|(g, _)| *g == which)
                .map(|(_, d)| d.stage());
            let other_busy = self.rodent_download.as_ref().is_some_and(|(g, d)| {
                *g != which
                    && matches!(d.stage(), crate::engine_download::Stage::Downloading { .. } | crate::engine_download::Stage::Unpacking)
            });
            let rodent_there = which.installed().is_some();
            let mut ready = if rodent { rodent_there } else { model.is_ok() };
            // A download going on: no game meanwhile (closing the dialog would cancel it).
            let busy = matches!(
                stage,
                Some(crate::maia_download::Stage::Downloading { .. } | crate::maia_download::Stage::Checking)
            ) || matches!(
                rstage,
                Some(crate::engine_download::Stage::Downloading { .. } | crate::engine_download::Stage::Unpacking)
            ) || other_busy;
            let large = crate::maia_download::installed(Model::Large).is_some();
            let rodent_missing = which.available() && !rodent_there;
            if !ready || !large || rodent_missing {
                ui.add_space(6.0);
                if rodent {
                    match &rstage {
                        None | Some(crate::engine_download::Stage::Cancelled) if !ready => {
                            let note = match which {
                                Gen::V => crate::i18n::trf(
                                    "Rodent V is downloaded once ({mb} MB, from github.com, the authors' own file).",
                                    &[("mb", &which.size_mb())],
                                ),
                                Gen::IV => crate::i18n::trf(
                                    "Rodent IV is downloaded once ({mb} MB): the engine built from its authors' sources, its characters and books from their repository.",
                                    &[("mb", &which.size_mb())],
                                ),
                            };
                            ui.label(RichText::new(note).weak());
                        }
                        Some(crate::engine_download::Stage::Downloading { done, total }) => {
                            ui.add(egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32).show_percentage());
                        }
                        Some(crate::engine_download::Stage::Unpacking) => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label(tr("Unpacking…"));
                            });
                        }
                        Some(crate::engine_download::Stage::Done(_)) => ready = true,
                        Some(crate::engine_download::Stage::Failed(f)) => {
                            ui.label(RichText::new(crate::i18n::trf(
                                "Could not download Rodent: {error}",
                                &[("error", &download_failure(f))],
                            )).color(ui.visuals().error_fg_color));
                        }
                        // Ready, but Maia's file is missing: the line keeps its place.
                        _ => {
                            ui.label(" ");
                        }
                    }
                } else if ready && large {
                    ui.label(" ");
                } else {
                    match stage {
                        None => {
                            ui.label(RichText::new(download_note(model.as_ref().err().copied())).weak());
                        }
                        Some(crate::maia_download::Stage::Downloading { done }) => {
                            let size = self.download.as_ref().map_or(1, |d| d.model.size());
                            ui.add(
                                egui::ProgressBar::new(done as f32 / size as f32).show_percentage(),
                            );
                        }
                        Some(crate::maia_download::Stage::Checking) => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label(tr("Checking…"));
                            });
                        }
                        Some(crate::maia_download::Stage::Done(_)) => ready = true,
                        Some(crate::maia_download::Stage::Failed(ref e)) => {
                            ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                        }
                    }
                }
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if ready { tr("Play") } else { tr("Download and play") };
                // As wide as the longer label: «Cancel» stays in its place.
                let width = [tr("Play"), tr("Download and play")]
                    .iter()
                    .map(|t| crate::game_view::text_width(ui, t, egui::TextStyle::Button))
                    .fold(0.0, f32::max)
                    + 2.0 * ui.spacing().button_padding.x
                    + 4.0;
                let main = ui
                    .add_enabled_ui(!busy, |ui| crate::widgets::primary_wide(ui, label, width))
                    .inner;
                // Enter plays unless it is meant for a focused control or the open list.
                let free = ctx.memory(|m| m.focused().is_none()) && !ctx.any_popup_open();
                let pressed = !busy
                    && (main.clicked() || (free && ui.input(|i| i.key_pressed(egui::Key::Enter))));
                if pressed && !ready {
                    if rodent {
                        self.rodent_download = Some((which, which.start(ctx)));
                        self.play_after = true;
                    } else if let Err(m) = model {
                        self.download = Some(crate::maia_download::Download::start(ctx, m));
                        self.play_after = true;
                    }
                }
                // After a download: the game starts when it is done, only if that opponent
                // is still the one chosen (not when the choice moved meanwhile).
                let downloaded = if rodent {
                    matches!(rstage, Some(crate::engine_download::Stage::Done(_)))
                } else {
                    matches!(stage, Some(crate::maia_download::Stage::Done(_)))
                };
                if ready && (pressed || (self.play_after && downloaded)) {
                    self.play_after = false;
                    let user = match self.prefs.color {
                        ColorChoice::White => Color::White,
                        ColorChoice::Black => Color::Black,
                        ColorChoice::Random => {
                            if random_unit() < 0.5 {
                                Color::White
                            } else {
                                Color::Black
                            }
                        }
                    };
                    settings.your_name = self.name.trim().to_owned();
                    settings.rating = rating;
                    if !self.name.trim().is_empty() {
                        crate::player_games::remember(&mut settings.players, &self.name, rating);
                    }
                    // From a position the colour is the side to move's, not a habit.
                    let color = settings.play.color;
                    settings.play = self.prefs.clone();
                    if self.keep_color {
                        settings.play.color = color;
                    }
                    out = Some(DialogOutcome::Play(Start {
                        rival: self.prefs.rival,
                        style: self.prefs.style.clone(),
                        user,
                        strength: self.prefs.strength,
                        user_elo: rating,
                        user_name: self.name.trim().to_owned(),
                        clock: self.prefs.clock,
                        from: self.from.clone().map(Box::new),
                    }));
                }
                if ui.add(egui::Button::new(tr("Cancel")).min_size(egui::vec2(0.0, 30.0))).clicked() {
                    out = Some(DialogOutcome::Cancel);
                }
            });
        });
        if out.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            out = Some(DialogOutcome::Cancel);
        }
        out
    }
}

/// The position a game goes on from: a small board (turned to the colour chosen) and
/// where it comes from. `board`: room for the board (not in a low window).
fn position_block(
    ui: &mut egui::Ui,
    settings: &crate::settings::Settings,
    from: &StartFrom,
    color: ColorChoice,
    board: bool,
) {
    use egui::RichText;
    let bottom = match color {
        ColorChoice::White => Color::White,
        ColorChoice::Black => Color::Black,
        ColorChoice::Random => from.position.turn(),
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::same(8))
        .corner_radius(4)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                if board {
                    ui.allocate_ui(egui::vec2(112.0, 112.0), |ui| {
                        let view = crate::board::BoardView {
                            pos: &from.position,
                            orientation: bottom,
                            last_move: from.last,
                            arrows: &[],
                            circles: &[],
                            badge: None,
                            interactive: false,
                            show_coords: false,
                            theme: settings.board_theme.theme(),
                            pieces: settings.piece_set,
                            animation: 0.0,
                            premove: None,
                            auto_queen: false,
                            alert: None,
                        };
                        let mut state = crate::board::BoardState::default();
                        view.show(ui, &mut state);
                    });
                    ui.add_space(12.0 - ui.spacing().item_spacing.x);
                }
                ui.vertical(|ui| {
                    let top = ui.cursor().top();
                    let title = match &from.after_shown {
                        Some(m) => {
                            crate::i18n::trf("From the position after {move}", &[("move", m)])
                        }
                        None => tr("From the game's starting position").to_owned(),
                    };
                    ui.label(RichText::new(title).strong().size(15.0));
                    if let Some(g) = &from.game {
                        // Two lines at most; the whole of it under the pointer.
                        let text = crate::i18n::trf("from the game {game}", &[("game", g)]);
                        let mut job = egui::text::LayoutJob::simple(
                            text.clone(),
                            egui::FontId::proportional(13.0),
                            ui.visuals().weak_text_color(),
                            ui.available_width(),
                        );
                        job.wrap.max_rows = 2;
                        ui.add(egui::Label::new(job)).on_hover_text(text);
                    }
                    if from.null_dropped {
                        ui.label(
                            RichText::new(tr(
                                "The moves before the null move (--) are not carried over",
                            ))
                            .weak()
                            .size(13.0),
                        );
                    }
                    // The side to move level with the bottom of the board.
                    let room = if board {
                        top + 112.0 - ui.cursor().top() - 17.0
                    } else {
                        4.0
                    };
                    ui.add_space(room.max(4.0));
                    ui.label(
                        RichText::new(match from.position.turn() {
                            Color::White => tr("White to move"),
                            Color::Black => tr("Black to move"),
                        })
                        .size(13.0),
                    );
                });
            });
        });
}

/// The opponent of a game put aside, downloaded so that the game can go on (it was
/// removed meanwhile): the same files and words as in the play dialog.
pub enum Fetch {
    Maia(crate::maia_download::Download),
    Rodent(crate::engine_download::Download),
}

/// How a `Fetch` goes.
#[derive(PartialEq)]
pub enum Fetched {
    Going,
    Done,
    Failed,
}

impl Fetch {
    pub fn start(saved: &Saved, ctx: &egui::Context) -> Fetch {
        match saved.rival {
            Rival::Maia => Fetch::Maia(crate::maia_download::Download::start(
                ctx,
                model_for().err().unwrap_or(Model::Large),
            )),
            Rival::Rodent => Fetch::Rodent(style(&saved.style).rodent.start(ctx)),
        }
    }

    /// What is downloaded, said before it is.
    pub fn note(saved: &Saved) -> String {
        match saved.rival {
            Rival::Maia => download_note(model_for().err()),
            Rival::Rodent => {
                let which = style(&saved.style).rodent;
                let mb = which.size_mb();
                match which {
                    Gen::V => crate::i18n::trf(
                        "Rodent V is downloaded once ({mb} MB, from github.com, the authors' own file).",
                        &[("mb", &mb)],
                    ),
                    Gen::IV => crate::i18n::trf(
                        "Rodent IV is downloaded once ({mb} MB): the engine built from its authors' sources, its characters and books from their repository.",
                        &[("mb", &mb)],
                    ),
                }
            }
        }
    }

    /// Its progress, or why it failed.
    pub fn show(&self, ui: &mut egui::Ui) -> Fetched {
        use crate::engine_download::Stage as R;
        use crate::maia_download::Stage as M;
        let failed = |ui: &mut egui::Ui, text: String| {
            ui.label(egui::RichText::new(text).color(ui.visuals().error_fg_color));
            Fetched::Failed
        };
        match self {
            Fetch::Maia(d) => match d.stage() {
                M::Downloading { done } => {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / d.model.size() as f32)
                            .show_percentage(),
                    );
                    Fetched::Going
                }
                M::Checking => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(tr("Checking…"));
                    });
                    Fetched::Going
                }
                M::Done(_) => Fetched::Done,
                M::Failed(e) => failed(ui, e),
            },
            Fetch::Rodent(d) => match d.stage() {
                R::Downloading { done, total } => {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32).show_percentage(),
                    );
                    Fetched::Going
                }
                R::Unpacking => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(tr("Unpacking…"));
                    });
                    Fetched::Going
                }
                R::Done(_) => Fetched::Done,
                R::Failed(f) => failed(
                    ui,
                    crate::i18n::trf(
                        "Could not download Rodent: {error}",
                        &[("error", &download_failure(&f))],
                    ),
                ),
                R::Cancelled => Fetched::Failed,
            },
        }
    }
}

/// Why Rodent's download failed, in words.
fn download_failure(f: &crate::engine_download::Failure) -> String {
    use crate::engine_download::Failure as F;
    match f {
        F::Network(e) | F::Disk(e) => e.clone(),
        F::Server(code) => format!("HTTP {code}"),
        F::Broken => tr("it does not start on this computer").to_owned(),
        F::Checksum => tr("The file is not the published one").to_owned(),
        F::Unsupported => tr("not yet for this system").to_owned(),
    }
}

/// Why an engine did not start, in words: the details go to the log.
fn start_failure(detail: &str) -> String {
    eprintln!("rodent: {detail}");
    tr("it does not start on this computer").to_owned()
}

/// What the download button brings: Maia herself, or (the small one being there) the
/// large one for the strength chosen.
fn download_note(model: Option<Model>) -> String {
    let mb = model.unwrap_or(Model::Large).size().div_ceil(1_000_000);
    crate::i18n::trf(
        "Maia is downloaded once ({mb} MB, from huggingface.co, the authors' own file).",
        &[("mb", &mb)],
    )
}

/// A game just played: what the result strip says.
#[derive(Clone, Debug)]
pub struct End {
    pub ending: Ending,
    pub user: Color,
    pub start: Start,
    /// The clocks at the end (user, Maia).
    pub left: Option<(Duration, Duration)>,
}

impl End {
    /// «You won» / «You lost» / «Draw» / «You resigned».
    pub fn heading(&self) -> &'static str {
        match self.ending {
            Ending::Resigned { loser } if loser == self.user => tr("You resigned"),
            Ending::Resigned { .. } => tr("You won"),
            e => match e.result() {
                chess_core::GameResult::Draw => tr("Draw"),
                r if r == Ending::Checkmate { winner: self.user }.result() => tr("You won"),
                _ => tr("You lost"),
            },
        }
    }

    pub fn detail(&self) -> &'static str {
        match self.ending {
            Ending::Checkmate { .. } => tr("checkmate"),
            Ending::Stalemate => tr("stalemate"),
            Ending::InsufficientMaterial => tr("neither side can checkmate"),
            Ending::Threefold => tr("the position occurred three times"),
            Ending::FiftyMoves => tr("50 moves without a capture or a pawn move"),
            Ending::Time { draw: true, loser } if loser == self.user => said(
                self.start.rival,
                "time is up, but Maia cannot checkmate",
                "time is up, but Rodent cannot checkmate",
            ),
            Ending::Time { draw: true, .. } => said(
                self.start.rival,
                "Maia's time is up, but you cannot checkmate",
                "Rodent's time is up, but you cannot checkmate",
            ),
            Ending::Time { .. } => tr("time is up"),
            Ending::Resigned { loser } if loser == self.user => "",
            Ending::Resigned { .. } => said(self.start.rival, "Maia resigned", "Rodent resigned"),
            Ending::DrawAgreed => tr("by agreement"),
        }
    }
}
