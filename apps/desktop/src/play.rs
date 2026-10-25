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
pub use ::play::{Ending, HOPELESS_TURNS, Judge, ending, hopeless, pause, random_unit};
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
    /// Maia's strength: a rating on the Lichess blitz scale (`level`: before 1.5 the
    /// field `strength` held her rating input, a different scale, and is dropped).
    #[serde(rename = "level")]
    pub strength: u32,
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
        }
    }
}

pub const STRENGTH_MIN: u32 = 1100;
pub const STRENGTH_MAX: u32 = 2100;

// ----- Maia's strength ----------------------------------------------------------------

pub use maia::strength::{SMALL_MAX, rating_input, temperature};

/// The model to play at `strength`, with its file: the large one when it is there,
/// else the small one up to its ceiling. `Err`: the model to download.
pub fn model_for(strength: u32) -> Result<(Model, std::path::PathBuf), Model> {
    use crate::maia_download::installed;
    if let Some(p) = installed(Model::Large) {
        return Ok((Model::Large, p));
    }
    match installed(Model::Small) {
        Some(p) if strength <= SMALL_MAX => Ok((Model::Small, p)),
        _ => Err(Model::Large),
    }
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

// ----- Maia's moves -------------------------------------------------------------------

/// A request for a move: the positions up to now, the ratings, the pause.
struct Request {
    generation: u64,
    positions: Vec<Chess>,
    self_elo: u32,
    oppo_elo: u32,
    temperature: f32,
    pause: Duration,
}

/// The engine in a thread of its own: loads Maia once, answers requests after a pause.
/// An answer for an older generation (after a take-back, a resignation) is dropped.
pub struct Opponent {
    tx: Sender<Request>,
    rx: Receiver<(u64, Option<Move>, Duration)>,
    generation: u64,
    /// Loading failed: why (shown to the user).
    pub error: Arc<Mutex<Option<String>>>,
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
            tx,
            rx,
            generation: 0,
            error,
            thinking: false,
        }
    }

    /// Asks for a move in the last of `positions`.
    pub fn ask(
        &mut self,
        positions: Vec<Chess>,
        (self_elo, temperature): (u32, f32),
        oppo_elo: u32,
        pause: Duration,
    ) {
        self.generation += 1;
        self.thinking = true;
        let _ = self.tx.send(Request {
            generation: self.generation,
            positions,
            self_elo,
            oppo_elo,
            temperature,
            pause,
        });
    }

    /// Forgets what was asked (its answer will be dropped).
    pub fn forget(&mut self) {
        self.generation += 1;
        self.thinking = false;
    }

    /// The move asked for last, when it has come, with the time it took (its pause, or
    /// the computing when longer).
    pub fn answer(&mut self) -> Option<(Move, Duration)> {
        let mut out = None;
        while let Ok((g, m, pause)) = self.rx.try_recv() {
            if g == self.generation {
                self.thinking = false;
                out = m.map(|m| (m, pause));
            }
        }
        out
    }

    pub fn failed(&self) -> Option<String> {
        self.error.lock().ok().and_then(|g| g.clone())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn strength_to_rating_input() {
        use maia::strength::Model;
        // The measured points themselves, in proportion between them, the ends held.
        assert_eq!(rating_input(Model::Large, 1605), 1500);
        assert_eq!(rating_input(Model::Large, 2000), 2600);
        // Above the top input: surer choices, down to the most likely move.
        assert_eq!(temperature(Model::Large, 1900), 1.0);
        assert!((temperature(Model::Large, 2053) - 0.5).abs() < 1e-6);
        assert!(temperature(Model::Large, STRENGTH_MAX) < 0.5);
        assert!(temperature(Model::Small, SMALL_MAX) < 1.0);
        assert_eq!(rating_input(Model::Small, 1485), 1500);
        assert_eq!(rating_input(Model::Small, SMALL_MAX), 2600);
        let mid = rating_input(Model::Large, (1605 + 1794) / 2);
        assert!((1690..=1710).contains(&mid), "{mid}");
        // The bottom of the slider is within what both models measured.
        assert!(rating_input(Model::Large, STRENGTH_MIN) > 600);
        assert!(rating_input(Model::Small, STRENGTH_MIN) > 600);
        // Stronger always means a higher input.
        for model in [Model::Small, Model::Large] {
            let mut last = 0;
            for level in (STRENGTH_MIN..=STRENGTH_MAX).step_by(50) {
                let v = rating_input(model, level);
                assert!(v >= last, "{model:?} {level}");
                last = v;
            }
        }
    }

    use super::*;
    use chess_core::shakmaty::uci::UciMove;

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
    pub user: Color,
    /// Maia's strength (Lichess blitz scale) and the model playing it.
    pub strength: u32,
    pub model: Model,
    pub user_elo: Option<u32>,
    pub clock: Option<Clock>,
    pub opponent: Opponent,
    /// «Resign?» waits for the second click.
    pub resign_asked: bool,
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

/// What the play dialog starts.
#[derive(Clone, Debug)]
pub struct Start {
    pub user: Color,
    pub strength: u32,
    pub user_elo: Option<u32>,
    pub user_name: String,
    pub clock: Option<TimeControl>,
    /// A game from a position of another game (docs/design/play-from-position.md).
    pub from: Option<Box<StartFrom>>,
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
    /// «Download and play» was pressed: the game starts once Maia is there.
    play_after: bool,
    /// The game goes on from a position of another game.
    from: Option<StartFrom>,
    /// The colour chosen is this game's, not a habit to remember (from a position, a
    /// rematch).
    keep_color: bool,
    focus: bool,
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
        let custom = prefs.clock.is_some_and(|c| !PRESETS.contains(&c));
        Dialog {
            name: settings.your_name.clone(),
            rating: settings.rating.map(|r| r.to_string()).unwrap_or_default(),
            prefs,
            custom,
            download: None,
            play_after: false,
            focus: true,
            from: None,
            keep_color: false,
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
        d.prefs.clock = start.clock;
        d.custom = start.clock.is_some_and(|c| !PRESETS.contains(&c));
        d.prefs.color = match start.user {
            Color::White => ColorChoice::White,
            Color::Black => ColorChoice::Black,
        };
        d.keep_color = true;
        d
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
        let model = model_for(self.prefs.strength);
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
            } else if body_h >= 420.0 {
                // In a low window the choices matter more than the introduction.
                ui.label(
                    RichText::new(tr(
                        "Maia plays like a human of the chosen rating: it makes the moves people of that level make, mistakes included.",
                    ))
                    .weak(),
                );
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
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.focus) && self.name.is_empty() {
                        r.request_focus();
                    }
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

            // The opponent: Maia's rating, or relative to the user's.
            section(
                ui,
                &crate::i18n::trf("Opponent — Maia {elo}", &[("elo", &self.prefs.strength)]),
            );
            ui.horizontal(|ui| {
                let end = |ui: &mut egui::Ui, v: u32| {
                    ui.add_sized([34.0, 18.0], egui::Label::new(RichText::new(v.to_string()).weak()))
                };
                end(ui, STRENGTH_MIN);
                ui.spacing_mut().slider_width = W - 2.0 * 34.0 - 2.0 * GAP;
                ui.add(
                    egui::Slider::new(&mut self.prefs.strength, STRENGTH_MIN..=STRENGTH_MAX)
                        .step_by(50.0)
                        .show_value(false),
                );
                end(ui, STRENGTH_MAX);
            });
            ui.label(
                RichText::new(tr("Her measured strength, on the Lichess blitz scale."))
                    .weak()
                    .size(11.5),
            );
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

            // Maia itself, downloaded once: under the body, by the buttons, always seen.
            let stage = self.download.as_ref().map(|d| d.stage());
            let mut ready = model.is_ok();
            // A download going on: no game meanwhile (closing the dialog would cancel it).
            let busy = matches!(
                    stage,
                    Some(crate::maia_download::Stage::Downloading { .. } | crate::maia_download::Stage::Checking)
                );
            // While the large model is missing the line stays (also when the small one
            // plays the strength chosen), so that the dialog does not jump with the slider.
            let large = crate::maia_download::installed(Model::Large).is_some();
            if !ready || !large {
                ui.add_space(6.0);
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
                let pressed = !busy
                    && (main.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if pressed && !ready && let Err(m) = model {
                    self.download = Some(crate::maia_download::Download::start(ctx, m));
                    self.play_after = true;
                }
                // After a download: the game starts when it is done (not when the slider
                // moves down to what another model plays).
                let downloaded = matches!(stage, Some(crate::maia_download::Stage::Done(_)));
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
                    // From a position the colour is the side to move's, not a habit.
                    let color = settings.play.color;
                    settings.play = self.prefs.clone();
                    if self.keep_color {
                        settings.play.color = color;
                    }
                    out = Some(DialogOutcome::Play(Start {
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

/// What the download button brings: Maia herself, or (the small one being there) the
/// large one for the strength chosen.
fn download_note(model: Option<Model>) -> String {
    let model = model.unwrap_or(Model::Large);
    let mb = model.size().div_ceil(1_000_000);
    if crate::maia_download::installed(Model::Small).is_some() {
        crate::i18n::trf(
            "Stronger than {elo} plays the large Maia: {mb} MB, downloaded once.",
            &[("elo", &SMALL_MAX), ("mb", &mb)],
        )
    } else {
        crate::i18n::trf(
            "Maia is downloaded once ({mb} MB, from huggingface.co, the authors' own file).",
            &[("mb", &mb)],
        )
    }
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
            Ending::Time { draw: true, loser } if loser == self.user => {
                tr("time is up, but Maia cannot checkmate")
            }
            Ending::Time { draw: true, .. } => tr("Maia's time is up, but you cannot checkmate"),
            Ending::Time { .. } => tr("time is up"),
            Ending::Resigned { loser } if loser == self.user => "",
            Ending::Resigned { .. } => tr("Maia resigned"),
        }
    }
}
