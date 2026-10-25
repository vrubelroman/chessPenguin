//! Playing against Maia (docs/research/play-mode.md, docs/design/play-mode.md): time
//! controls as on Lichess, the clock, the end of a game, and Maia's moves computed in a
//! background thread with a human-like pause.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chess_core::shakmaty::zobrist::Zobrist64;
use chess_core::shakmaty::{Chess, Color, EnPassantMode, Move, Position};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

// ----- time controls --------------------------------------------------------------------

/// A clock: base seconds and increment seconds; `None` in the settings is "no clock".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeControl {
    pub base: u32,
    pub increment: u32,
}

/// The pools of Lichess (lila `PoolList.scala`).
pub const PRESETS: [TimeControl; 11] = [
    TimeControl {
        base: 60,
        increment: 0,
    },
    TimeControl {
        base: 120,
        increment: 1,
    },
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
    UltraBullet,
    Bullet,
    Blitz,
    Rapid,
    Classical,
}

impl Speed {
    pub fn name(self) -> &'static str {
        match self {
            Speed::UltraBullet => tr("UltraBullet"),
            Speed::Bullet => tr("Bullet"),
            Speed::Blitz => tr("Blitz"),
            Speed::Rapid => tr("Rapid"),
            Speed::Classical => tr("Classical"),
        }
    }
}

impl TimeControl {
    /// The category by the estimated length, base + 40 × increment (scalachess
    /// `Speed.scala`).
    pub fn speed(self) -> Speed {
        match self.base + 40 * self.increment {
            0..30 => Speed::UltraBullet,
            30..180 => Speed::Bullet,
            180..480 => Speed::Blitz,
            480..1500 => Speed::Rapid,
            _ => Speed::Classical,
        }
    }

    /// «5+3», «½+0» as Lichess writes it (minutes + seconds).
    pub fn label(self) -> String {
        let min = match self.base {
            15 => "¼".to_owned(),
            30 => "½".to_owned(),
            45 => "¾".to_owned(),
            90 => "1½".to_owned(),
            b => (b / 60).to_string(),
        };
        format!("{min}+{}", self.increment)
    }

    /// The PGN tag `TimeControl`: seconds + increment.
    pub fn pgn(self) -> String {
        format!("{}+{}", self.base, self.increment)
    }
}

/// The minutes the custom control offers (Lichess `timeControl.ts`), in seconds.
pub const CUSTOM_BASES: &[u32] = &[
    0, 15, 30, 45, 60, 90, 120, 180, 240, 300, 360, 420, 480, 540, 600, 660, 720, 780, 840, 900,
    960, 1020, 1080, 1140, 1200, 1500, 1800, 2100, 2400, 2700, 3600, 4500, 5400, 6300, 7200, 8100,
    9000, 9900, 10800,
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
    /// Maia's rating, 600–2600.
    pub strength: u32,
}

impl Default for PlayPrefs {
    fn default() -> Self {
        PlayPrefs {
            color: ColorChoice::Random,
            clock: Some(TimeControl {
                base: 300,
                increment: 3,
            }),
            strength: 1500,
        }
    }
}

pub const STRENGTH_MIN: u32 = 600;
pub const STRENGTH_MAX: u32 = 2600;

/// «As me»: the user's rating rounded to 100, within Maia's range.
pub fn strength_like(rating: u32) -> u32 {
    (((rating + 50) / 100) * 100).clamp(STRENGTH_MIN, STRENGTH_MAX)
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
    /// the other side's clock starts.
    pub fn moved(&mut self, c: Color, now: Instant) {
        let i = idx(c);
        if self.moved[i] {
            self.left[i] = self.left(c, now);
        }
        self.moved[i] = true;
        self.left[i] += Duration::from_secs(u64::from(self.control.increment));
        self.running = Some((!c, now));
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

// ----- Maia's moves -------------------------------------------------------------------

/// A request for a move: the positions up to now, the ratings, the pause.
struct Request {
    generation: u64,
    positions: Vec<Chess>,
    self_elo: u32,
    oppo_elo: u32,
    pause: Duration,
}

/// The engine in a thread of its own: loads Maia once, answers requests after a pause.
/// An answer for an older generation (after a take-back, a resignation) is dropped.
pub struct Opponent {
    tx: Sender<Request>,
    rx: Receiver<(u64, Option<Move>)>,
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
                    let m = maia::pick(&p, 1.0, random_unit());
                    let rest = r.pause.saturating_sub(started.elapsed());
                    std::thread::sleep(rest);
                    if ans_tx.send((r.generation, m)).is_err() {
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
    pub fn ask(&mut self, positions: Vec<Chess>, self_elo: u32, oppo_elo: u32, pause: Duration) {
        self.generation += 1;
        self.thinking = true;
        let _ = self.tx.send(Request {
            generation: self.generation,
            positions,
            self_elo,
            oppo_elo,
            pause,
        });
    }

    /// Forgets what was asked (its answer will be dropped).
    pub fn forget(&mut self) {
        self.generation += 1;
        self.thinking = false;
    }

    /// The move asked for last, when it has come.
    pub fn answer(&mut self) -> Option<Move> {
        let mut out = None;
        while let Ok((g, m)) = self.rx.try_recv() {
            if g == self.generation {
                self.thinking = false;
                out = m;
            }
        }
        out
    }

    pub fn failed(&self) -> Option<String> {
        self.error.lock().ok().and_then(|g| g.clone())
    }
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
            if left < 10.0 {
                0.2 + 0.8 * r
            } else {
                let mut t = (left / 30.0 + 0.75 * inc as f32) * factor;
                if ply < 10 {
                    t *= 0.3;
                }
                t.clamp(0.4, (left / 8.0).min(60.0))
            }
        }
    };
    let secs = if only_move {
        secs.min(0.3 + 0.5 * r)
    } else {
        secs
    };
    Duration::from_secs_f32(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::shakmaty::uci::UciMove;

    #[test]
    fn speeds_as_lichess() {
        let s = |base, increment| TimeControl { base, increment }.speed();
        assert_eq!(s(60, 0), Speed::Bullet);
        assert_eq!(s(120, 1), Speed::Bullet);
        assert_eq!(s(180, 0), Speed::Blitz);
        assert_eq!(s(300, 3), Speed::Blitz);
        assert_eq!(s(420, 0), Speed::Blitz);
        assert_eq!(s(480, 0), Speed::Rapid);
        assert_eq!(s(900, 10), Speed::Rapid);
        assert_eq!(s(1500, 0), Speed::Classical);
        assert_eq!(s(15, 0), Speed::UltraBullet);
        assert_eq!(
            TimeControl {
                base: 300,
                increment: 3
            }
            .label(),
            "5+3"
        );
        assert_eq!(
            TimeControl {
                base: 30,
                increment: 0
            }
            .label(),
            "½+0"
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
        c.moved(Color::White, t0); // the first move takes nothing
        assert_eq!(c.left(Color::White, t0), Duration::from_secs(62));
        // Black's first move: no time taken either.
        c.moved(Color::Black, t0 + Duration::from_secs(5));
        assert_eq!(
            c.left(Color::Black, t0 + Duration::from_secs(5)),
            Duration::from_secs(62)
        );
        // White thinks 10 s.
        let t1 = t0 + Duration::from_secs(15);
        assert_eq!(c.left(Color::White, t1), Duration::from_secs(52));
        c.moved(Color::White, t1);
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
        for _ in 0..200 {
            let p = pause(Some((Duration::from_secs(20), 0)), 30, false);
            assert!(p <= Duration::from_millis(2500), "{p:?}");
            let p = pause(Some((Duration::from_secs(5), 0)), 30, false);
            assert!(p <= Duration::from_millis(1000));
        }
        assert_eq!(strength_like(1350), 1400);
        assert_eq!(strength_like(300), 600);
    }
}

// ----- a game being played ------------------------------------------------------------

/// A game against Maia in progress (lives in the game screen while it is played).
pub struct Session {
    pub user: Color,
    pub strength: u32,
    pub user_elo: Option<u32>,
    pub clock: Option<Clock>,
    pub opponent: Opponent,
    /// The low-time warning was given (once).
    pub warned: bool,
    /// «Resign?» waits for the second click.
    pub resign_asked: bool,
}

/// What the play dialog starts.
#[derive(Clone, Debug)]
pub struct Start {
    pub user: Color,
    pub strength: u32,
    pub user_elo: Option<u32>,
    pub user_name: String,
    pub clock: Option<TimeControl>,
}

// ----- the dialog ---------------------------------------------------------------------

/// «Play against the computer»: name and rating, colour, time, strength.
pub struct Dialog {
    name: String,
    rating: String,
    prefs: PlayPrefs,
    custom: bool,
    download: Option<crate::maia_download::Download>,
    focus: bool,
}

pub enum DialogOutcome {
    Play(Start),
    Cancel,
}

impl Dialog {
    pub fn new(settings: &crate::settings::Settings) -> Dialog {
        let prefs = settings.play.clone();
        let custom = prefs.clock.is_some_and(|c| !PRESETS.contains(&c));
        Dialog {
            name: settings.your_name.clone(),
            rating: settings.rating.map(|r| r.to_string()).unwrap_or_default(),
            prefs,
            custom,
            download: None,
            focus: true,
        }
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
        let installed = crate::maia_download::installed();
        egui::Modal::new(egui::Id::new("play_dialog")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("Play against the computer"));
            ui.label(
                RichText::new(tr(
                    "Maia plays like a human of the chosen rating: it makes the moves people of that level make, mistakes included.",
                ))
                .weak(),
            );
            ui.add_space(10.0);

            // You.
            ui.label(RichText::new(tr("You")).strong());
            ui.horizontal(|ui| {
                ui.label(tr("Name:"));
                let r = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(200.0));
                if std::mem::take(&mut self.focus) && self.name.is_empty() {
                    r.request_focus();
                }
                ui.label(tr("Rating:"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.rating)
                        .desired_width(60.0)
                        .hint_text("1500"),
                );
                if r.changed() {
                    self.rating.retain(|c| c.is_ascii_digit());
                    self.rating.truncate(4);
                }
            });
            ui.label(
                RichText::new(tr("Not required; the opponent's strength is chosen relative to it."))
                    .weak()
                    .size(11.5),
            );
            ui.add_space(10.0);

            // Colour.
            ui.label(RichText::new(tr("Your colour")).strong());
            ui.horizontal(|ui| {
                for (c, label) in [
                    (ColorChoice::White, tr("♔ White")),
                    (ColorChoice::Random, tr("Random")),
                    (ColorChoice::Black, tr("♚ Black")),
                ] {
                    ui.selectable_value(&mut self.prefs.color, c, label);
                }
            });
            ui.add_space(10.0);

            // Time.
            ui.label(RichText::new(tr("Time")).strong());
            egui::Grid::new("play_presets")
                .spacing(egui::vec2(6.0, 6.0))
                .show(ui, |ui| {
                    for (k, tc) in PRESETS.iter().enumerate() {
                        let on = !self.custom && self.prefs.clock == Some(*tc);
                        let text = format!("{}\n{}", tc.label(), tc.speed().name());
                        if ui
                            .add_sized([96.0, 40.0], egui::Button::selectable(on, text))
                            .clicked()
                        {
                            self.prefs.clock = Some(*tc);
                            self.custom = false;
                        }
                        if k % 4 == 3 {
                            ui.end_row();
                        }
                    }
                    if ui
                        .add_sized([96.0, 40.0], egui::Button::selectable(self.custom, tr("Custom")))
                        .clicked()
                    {
                        self.custom = true;
                        if self.prefs.clock.is_none() {
                            self.prefs.clock = Some(TimeControl { base: 600, increment: 0 });
                        }
                    }
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                let none = self.prefs.clock.is_none();
                if ui.selectable_label(none, tr("No clock")).clicked() {
                    self.prefs.clock = None;
                    self.custom = false;
                }
            });
            if self.custom
                && let Some(tc) = self.prefs.clock.as_mut()
            {
                let mut b = CUSTOM_BASES.iter().position(|&x| x >= tc.base).unwrap_or(14);
                let mut i = CUSTOM_INCREMENTS
                    .iter()
                    .position(|&x| x >= tc.increment)
                    .unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.label(tr("Minutes:"));
                    ui.add(egui::Slider::new(&mut b, 0..=CUSTOM_BASES.len() - 1).show_value(false));
                    ui.label(TimeControl { base: CUSTOM_BASES[b], increment: 0 }.label().trim_end_matches("+0"));
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Increment, s:"));
                    ui.add(
                        egui::Slider::new(&mut i, 0..=CUSTOM_INCREMENTS.len() - 1).show_value(false),
                    );
                    ui.label(CUSTOM_INCREMENTS[i].to_string());
                });
                tc.base = CUSTOM_BASES[b];
                tc.increment = CUSTOM_INCREMENTS[i];
                if tc.base == 0 && tc.increment == 0 {
                    tc.increment = 1;
                }
                ui.label(
                    RichText::new(format!("{} · {}", tc.label(), tc.speed().name())).weak(),
                );
            }
            ui.add_space(10.0);

            // Strength.
            ui.label(RichText::new(tr("Opponent")).strong());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Slider::new(&mut self.prefs.strength, STRENGTH_MIN..=STRENGTH_MAX)
                        .step_by(100.0)
                        .show_value(false),
                );
                ui.label(RichText::new(format!("Maia {}", self.prefs.strength)).strong());
            });
            let rating = self.rating();
            ui.add_enabled_ui(rating.is_some(), |ui| {
                ui.horizontal(|ui| {
                    for (label, delta) in [(tr("Weaker"), -200), (tr("As me"), 0), (tr("Stronger"), 200)] {
                        if ui.button(label).clicked()
                            && let Some(r) = rating
                        {
                            self.prefs.strength =
                                strength_like((r as i32 + delta).max(0) as u32);
                        }
                    }
                });
            })
            .response
            .on_disabled_hover_text(tr("Enter your rating to choose relative to it"));
            ui.label(
                RichText::new(tr("Maia's ratings are Lichess ratings: club and federation ratings are usually lower."))
                    .weak()
                    .size(11.5),
            );
            ui.add_space(10.0);

            // Maia itself: downloaded once.
            let mut ready = installed.is_some();
            if !ready {
                match self.download.as_ref().map(|d| d.stage()) {
                    None => {
                        ui.label(trf_size());
                        if ui.button(tr("Download Maia")).clicked() {
                            self.download = Some(crate::maia_download::Download::start(ctx));
                        }
                    }
                    Some(crate::maia_download::Stage::Downloading { done }) => {
                        ui.add(
                            egui::ProgressBar::new(done as f32 / crate::maia_download::SIZE as f32)
                                .show_percentage(),
                        );
                    }
                    Some(crate::maia_download::Stage::Checking) => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(tr("Checking…"));
                        });
                    }
                    Some(crate::maia_download::Stage::Done(_)) => ready = true,
                    Some(crate::maia_download::Stage::Failed(e)) => {
                        ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                        if ui.button(tr("Try again")).clicked() {
                            self.download = Some(crate::maia_download::Download::start(ctx));
                        }
                    }
                }
            }

            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let play = ui
                    .add_enabled_ui(ready, |ui| crate::widgets::primary(ui, tr("Play")))
                    .inner;
                let enter = ready && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if play.clicked() || enter {
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
                    settings.play = self.prefs.clone();
                    out = Some(DialogOutcome::Play(Start {
                        user,
                        strength: self.prefs.strength,
                        user_elo: rating,
                        user_name: self.name.trim().to_owned(),
                        clock: self.prefs.clock,
                    }));
                }
                if ui.button(tr("Cancel")).clicked() {
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

fn trf_size() -> String {
    crate::i18n::trf(
        "Maia is downloaded once ({mb} MB, from huggingface.co, the authors' own file).",
        &[("mb", &crate::maia_download::SIZE.div_ceil(1_000_000))],
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
            Ending::Resigned { .. } => tr("You resigned"),
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
            Ending::Time { draw: true, .. } => tr("time is up, but Maia cannot checkmate"),
            Ending::Time { .. } => tr("time is up"),
            Ending::Resigned { .. } => "",
        }
    }
}
