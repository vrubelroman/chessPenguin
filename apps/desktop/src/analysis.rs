//! Engine analysis controller: owns the engine (a UCI process; in a browser a Web
//! Worker), binds results to the analysed position.

use crate::i18n::{tr, trf};

use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Move, Position};
use uci::{
    EngineEvent, EngineHandle, EnginePosition, SearchId, SearchInfo, SearchLimits, Side,
    TerminationReason, WhiteEval, WhiteScore,
};

use crate::settings::EngineConfig;

#[derive(Clone, Debug)]
pub struct PvLine {
    pub multipv: u32,
    pub depth: u32,
    pub score: WhiteScore,
    /// Legal moves of the PV from the analysed position (stops at the first illegal move).
    pub moves: Vec<Move>,
    pub san: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Off,
    Starting,
    Running,
    Failed(String),
}

/// A position the engine is (or should be) analysing.
struct Target {
    key: String,
    root: Chess,
    engine_pos: EnginePosition,
}

/// An engine on its way: its process started on a thread of its own (the handshake
/// blocks for up to ~20 s); in a browser its worker, the handshake by its messages.
enum Launch {
    #[cfg(not(web))]
    Thread(std::sync::mpsc::Receiver<Result<EngineHandle, uci::EngineError>>),
    /// The worker and the options to set once it answered.
    #[cfg(web)]
    Worker(uci::Starting, EngineConfig),
}

impl Launch {
    /// The engine once it answered, its options set (or why it did not answer).
    fn poll(&mut self) -> Option<Result<EngineHandle, uci::EngineError>> {
        match self {
            #[cfg(not(web))]
            Launch::Thread(rx) => rx.try_recv().ok(),
            #[cfg(web)]
            Launch::Worker(s, cfg) => s.poll().map(|r| r.inspect(|h| set_options(h, cfg))),
        }
    }
}

/// Threads, hash and lines of `cfg` (unknown options are not fatal: some engines lack
/// Threads/Hash/MultiPV).
fn set_options(h: &EngineHandle, cfg: &EngineConfig) {
    let _ = h.set_threads(cfg.threads);
    let _ = h.set_hash_mb(cfg.hash_mb);
    let _ = h.set_multipv(cfg.multipv.max(1));
}

pub struct Analysis {
    engine: Option<EngineHandle>,
    spawning: Option<Launch>,
    pub status: Status,
    pub engine_name: String,
    next_id: u64,
    current: Option<(SearchId, Target)>,
    /// Requested while the engine is still starting.
    queued: Option<Target>,
    pub lines: Vec<PvLine>,
    pub nodes: u64,
    pub nps: u64,
    multipv: u32,
    /// Search time per position; `None` = infinite analysis.
    limit: Option<u32>,
    /// The current search ended (time limit reached); lines stay as the final result.
    pub finished: bool,
    /// Executable of the running engine (to detect a switch of the active engine).
    pub engine_path: Option<crate::engines::EngineLocation>,
}

impl Analysis {
    /// Lines the engine is asked for (the panel keeps their room while they come).
    pub fn line_count(&self) -> usize {
        self.multipv.max(1) as usize
    }
}

impl Analysis {
    pub fn new() -> Self {
        Self {
            engine: None,
            spawning: None,
            status: Status::Off,
            engine_name: String::new(),
            next_id: 1,
            current: None,
            queued: None,
            lines: Vec::new(),
            nodes: 0,
            nps: 0,
            multipv: 1,
            limit: None,
            finished: false,
            engine_path: None,
        }
    }

    pub fn is_on(&self) -> bool {
        matches!(self.status, Status::Starting | Status::Running)
    }

    /// Starts the engine in the background: its options are set once it answered.
    pub fn start(&mut self, cfg: &EngineConfig, ctx: &egui::Context) {
        self.stop();
        let Some(path) = cfg.executable() else {
            self.status =
                Status::Failed(tr("The built-in engine is not part of this build").into());
            return;
        };
        self.engine_name = cfg.name.clone();
        self.multipv = cfg.multipv.max(1);
        self.limit = cfg.search_secs;
        self.engine_path = Some(path.clone());
        match launch(path, cfg.clone(), ctx) {
            Ok(l) => {
                self.spawning = Some(l);
                self.status = Status::Starting;
            }
            Err(e) => self.status = Status::Failed(e),
        }
    }

    /// Stops analysis and terminates the engine.
    pub fn stop(&mut self) {
        if let Some(e) = self.engine.take() {
            // quit() may wait up to the quit timeout; do it off the UI thread. A worker
            // stops at once.
            #[cfg(not(web))]
            let _ = std::thread::Builder::new()
                .name("engine-quit".into())
                .spawn(move || {
                    e.quit();
                });
            #[cfg(web)]
            e.quit();
        }
        self.spawning = None;
        self.current = None;
        self.queued = None;
        self.lines.clear();
        self.engine_path = None;
        self.status = Status::Off;
    }

    /// Applies Threads/Hash/MultiPV/time of the running engine and restarts the current search.
    pub fn reconfigure(&mut self, cfg: &EngineConfig, ctx: &egui::Context) {
        if self.engine_path != cfg.executable() || self.spawning.is_some() {
            self.start(cfg, ctx);
            return;
        }
        let Some(engine) = &self.engine else { return };
        let _ = engine.set_threads(cfg.threads);
        let _ = engine.set_hash_mb(cfg.hash_mb);
        let _ = engine.set_multipv(cfg.multipv.max(1));
        self.multipv = cfg.multipv.max(1);
        self.limit = cfg.search_secs;
        if let Some((_, t)) = self.current.take() {
            self.set_position(&t.root, t.engine_pos);
        }
    }

    /// Requests analysis of `root`; no-op if that exact history is already being analysed.
    pub fn set_position(&mut self, root: &Chess, engine_pos: EnginePosition) {
        if !self.is_on() {
            return;
        }
        let key = format!("{:?}|{}", engine_pos.base, engine_pos.moves.join(" "));
        if self.current.as_ref().is_some_and(|(_, t)| t.key == key) {
            return;
        }
        if self.queued.as_ref().is_some_and(|t| t.key == key) {
            return;
        }
        let target = Target {
            key,
            root: root.clone(),
            engine_pos,
        };
        self.lines.clear();
        self.nodes = 0;
        self.nps = 0;
        self.finished = false;
        let Some(engine) = &self.engine else {
            self.queued = Some(target);
            return;
        };
        let id = SearchId(self.next_id);
        self.next_id += 1;
        let res = match self.limit {
            None => engine.analyze(target.engine_pos.clone(), id),
            Some(secs) => engine.search(
                target.engine_pos.clone(),
                id,
                SearchLimits::MoveTime(std::time::Duration::from_secs(u64::from(secs))),
            ),
        };
        match res {
            Ok(()) => self.current = Some((id, target)),
            Err(e) => {
                self.current = None;
                self.status = Status::Failed(e.to_string());
            }
        }
    }

    /// Drains engine events; call once per frame.
    pub fn poll(&mut self) {
        if let Some(res) = self.spawning.as_mut().and_then(Launch::poll) {
            self.spawning = None;
            match res {
                Ok(h) => {
                    if let Some(name) = &h.info().name {
                        self.engine_name = name.clone();
                    }
                    self.engine = Some(h);
                    self.status = Status::Running;
                    if let Some(t) = self.queued.take() {
                        let (root, pos) = (t.root, t.engine_pos);
                        self.set_position(&root, pos);
                    }
                }
                Err(e) => {
                    self.lines.clear();
                    self.status = Status::Failed(e.to_string());
                }
            }
        }
        let Some(engine) = &self.engine else { return };
        let events: Vec<EngineEvent> = engine.drain_events().collect();
        for ev in events {
            match ev {
                EngineEvent::Info { search_id, info } => {
                    if let Some((id, target)) = &self.current
                        && *id == search_id
                    {
                        let side = side(target.root.turn());
                        if let Some(line) = make_line(&target.root, side, &info) {
                            self.nodes = info.nodes.unwrap_or(self.nodes);
                            self.nps = info.nps.unwrap_or(self.nps);
                            upsert(&mut self.lines, line, self.multipv);
                        }
                    }
                }
                EngineEvent::BestMove { search_id, .. } => {
                    if self
                        .current
                        .as_ref()
                        .is_some_and(|(id, _)| *id == search_id)
                    {
                        self.finished = true;
                    }
                }
                EngineEvent::Terminated(reason) => {
                    self.engine = None;
                    self.current = None;
                    self.lines.clear();
                    self.status = Status::Failed(termination_message(&reason));
                    return;
                }
                _ => {}
            }
        }
    }

    /// Best line of the current search, already bound to the position it was computed for.
    pub fn best(&self) -> Option<&PvLine> {
        if self.status == Status::Running {
            self.lines.first()
        } else {
            None
        }
    }
}

/// Starts the engine of `cfg` at `path` with its options.
#[cfg(not(web))]
fn launch(
    location: crate::engines::EngineLocation,
    cfg: EngineConfig,
    ctx: &egui::Context,
) -> Result<Launch, String> {
    let crate::engines::EngineLocation::Program(path) = location;
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx_w = ctx.clone();
    let ctx_done = ctx.clone();
    std::thread::Builder::new()
        .name("engine-spawn".into())
        .spawn(move || {
            let res = uci::Engine::spawn_with(
                uci::EngineConfig::new(&path),
                Box::new(move || ctx_w.request_repaint()),
            )
            .inspect(|h| set_options(h, &cfg));
            let _ = tx.send(res);
            ctx_done.request_repaint();
        })
        .map(|_| Launch::Thread(rx))
        .map_err(|e| e.to_string())
}

/// Starts the engine of `cfg` in its Web Worker.
#[cfg(web)]
fn launch(
    location: crate::engines::EngineLocation,
    cfg: EngineConfig,
    ctx: &egui::Context,
) -> Result<Launch, String> {
    let crate::engines::EngineLocation::Worker(script) = location;
    let ctx = ctx.clone();
    uci::Engine::start_worker(
        uci::WorkerConfig::new(script),
        Box::new(move || ctx.request_repaint()),
    )
    .map(|s| Launch::Worker(s, cfg))
    .map_err(|e| e.to_string())
}

impl Drop for Analysis {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Human-readable reason of an engine process ending.
pub fn termination_message(reason: &TerminationReason) -> String {
    match reason {
        TerminationReason::Hung { .. } => tr("The engine hung and was stopped").into(),
        TerminationReason::Exited { status } | TerminationReason::Quit { status } => match status {
            Some(s) => trf("The engine exited ({s})", &[("s", s)]),
            None => tr("The engine exited").into(),
        },
    }
}

fn side(c: Color) -> Side {
    match c {
        Color::White => Side::White,
        Color::Black => Side::Black,
    }
}

fn make_line(root: &Chess, side: Side, info: &SearchInfo) -> Option<PvLine> {
    let score = info.score?.to_white(side);
    if info.pv.is_empty() {
        return None;
    }
    let mut pos = root.clone();
    let mut moves = Vec::new();
    let mut san = Vec::new();
    for u in &info.pv {
        let Ok(m) = u
            .parse::<UciMove>()
            .map_err(|_| ())
            .and_then(|um| um.to_move(&pos).map_err(|_| ()))
        else {
            break;
        };
        san.push(SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string());
        moves.push(m);
    }
    if moves.is_empty() {
        return None;
    }
    Some(PvLine {
        multipv: info.multipv_index(),
        depth: info.depth.unwrap_or(0),
        score,
        moves,
        san,
    })
}

fn upsert(lines: &mut Vec<PvLine>, line: PvLine, multipv: u32) {
    if line.multipv > multipv {
        return;
    }
    match lines.iter_mut().find(|l| l.multipv == line.multipv) {
        Some(l) => *l = line,
        None => lines.push(line),
    }
    lines.sort_by_key(|l| l.multipv);
}

/// "+0.35", "-1.20", "#3", "#-2" from White's point of view.
pub fn format_eval(s: &WhiteScore) -> String {
    match s.eval {
        WhiteEval::Cp(cp) => format!("{:+.2}", cp as f32 / 100.0),
        WhiteEval::Mate { winner, moves } => match winner {
            Side::White => format!("#{moves}"),
            Side::Black => format!("#-{moves}"),
        },
    }
}

/// Evaluation squashed into 0..1 for the eval bar (1 = White winning).
pub fn bar_fraction(s: &WhiteScore) -> f32 {
    match s.eval {
        WhiteEval::Cp(cp) => {
            let x = cp as f32 / 100.0;
            0.5 + 0.5 * (2.0 / (1.0 + (-0.37 * x).exp()) - 1.0)
        }
        WhiteEval::Mate {
            winner: Side::White,
            ..
        } => 1.0,
        WhiteEval::Mate {
            winner: Side::Black,
            ..
        } => 0.0,
    }
}
