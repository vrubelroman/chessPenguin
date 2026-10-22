//! Engine process, handshake and the supervisor thread.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::EngineError;
use crate::parse::{EngineLine, parse_line};
use crate::types::{
    EngineInfo, EnginePosition, OptionKind, SearchId, SearchInfo, SearchLimits, Timeouts,
};

/// Callback invoked (from the supervisor thread) after every new event; the
/// UI passes `ctx.request_repaint()` here.
pub type Waker = Box<dyn Fn() + Send + 'static>;

/// Maximum stored length of one output line; longer lines are truncated.
const MAX_LINE_BYTES: usize = 64 * 1024;
/// Number of stderr lines kept for diagnostics.
const STDERR_TAIL_LINES: usize = 256;
/// Maximum number of `Warning` events emitted per engine (flood protection).
const MAX_WARNINGS: usize = 100;

/// Event delivered to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// `position` + `go` for this id were sent to the engine.
    SearchStarted { search_id: SearchId },
    /// The request was replaced by a newer one (or cancelled by `stop`)
    /// before it was sent to the engine; no events will follow for this id.
    Superseded { search_id: SearchId },
    /// Search output. Only lines with a score or a PV are forwarded.
    Info {
        search_id: SearchId,
        info: SearchInfo,
    },
    /// Final `bestmove` of a started search. Exactly one per `SearchStarted`
    /// unless the engine terminates first.
    BestMove {
        search_id: SearchId,
        best: Option<String>,
        ponder: Option<String>,
    },
    /// `info string`, attributed to the running search if any.
    InfoString {
        search_id: Option<SearchId>,
        text: String,
    },
    /// Malformed or unexpected engine output; the line was skipped.
    Warning(String),
    /// The supervisor has stopped; the handle is no longer usable.
    Terminated(TerminationReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationReason {
    /// `quit()` or drop of the handle.
    Quit { status: Option<ExitStatus> },
    /// The process exited on its own (crash, EOF on stdout, write failure).
    Exited { status: Option<ExitStatus> },
    /// The engine did not answer `stop` or `isready` in time and was killed.
    Hung { waiting_for: &'static str },
}

/// Spawn configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub path: PathBuf,
    pub args: Vec<OsString>,
    /// Working directory of the engine. `None` = directory of the executable
    /// (engines often look for weights/books next to themselves).
    pub working_dir: Option<PathBuf>,
    pub timeouts: Timeouts,
    /// Added to the engine's nice value on Unix (0 = as the app): a phone keeps its
    /// interface smooth while the engine thinks. Every thread of the engine has it.
    pub nice: i32,
}

impl EngineConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        EngineConfig {
            path: path.into(),
            args: Vec::new(),
            working_dir: None,
            timeouts: Timeouts::default(),
            nice: 0,
        }
    }
}

/// Namespace for spawning engines.
pub struct Engine;

impl Engine {
    /// Starts the engine and performs `uci`/`uciok` and `isready`/`readyok`.
    ///
    /// **Blocks** for up to `uciok + readyok` timeouts (defaults: 10 s each);
    /// call it from a worker thread, not from the UI frame.
    pub fn spawn<I, S>(
        path: impl AsRef<Path>,
        args: I,
        waker: Waker,
    ) -> Result<EngineHandle, EngineError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cfg = EngineConfig::new(path.as_ref());
        cfg.args = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        Self::spawn_with(cfg, waker)
    }

    pub fn spawn_with(cfg: EngineConfig, waker: Waker) -> Result<EngineHandle, EngineError> {
        spawn_impl(cfg, waker)
    }
}

enum Input {
    Cmd(Cmd),
    Stdout(String),
    StdoutClosed,
}

enum Cmd {
    Search {
        position: EnginePosition,
        id: SearchId,
        limits: SearchLimits,
    },
    Stop,
    /// A complete `setoption ...` / `ucinewgame` line, applied while idle.
    Setup(String),
    Quit,
}

/// Handle owned by the UI. Dropping it quits (and if needed kills) the engine.
pub struct EngineHandle {
    info: EngineInfo,
    pid: u32,
    tx: Sender<Input>,
    events: Receiver<EngineEvent>,
    alive: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    supervisor: Option<JoinHandle<()>>,
    done: Receiver<()>,
    timeouts: Timeouts,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("name", &self.info.name)
            .field("pid", &self.pid)
            .field("alive", &self.is_running())
            .finish()
    }
}

impl EngineHandle {
    /// `id name/author` and declared options from the handshake.
    pub fn info(&self) -> &EngineInfo {
        &self.info
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `false` after the engine terminated (quit, crash, hung-kill).
    pub fn is_running(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Non-blocking poll for the next event (call every frame).
    pub fn try_recv(&self) -> Option<EngineEvent> {
        self.events.try_recv().ok()
    }

    /// Drains all currently queued events without blocking.
    pub fn drain_events(&self) -> impl Iterator<Item = EngineEvent> + '_ {
        self.events.try_iter()
    }

    /// Blocking receive with a timeout (tests, non-UI callers).
    pub fn recv_timeout(&self, timeout: Duration) -> Option<EngineEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Last lines the engine wrote to stderr.
    pub fn stderr_tail(&self) -> Vec<String> {
        lock(&self.stderr).iter().cloned().collect()
    }

    /// Infinite analysis (`go infinite`) of `position` under `search_id`.
    pub fn analyze(
        &self,
        position: EnginePosition,
        search_id: SearchId,
    ) -> Result<(), EngineError> {
        self.search(position, search_id, SearchLimits::Infinite)
    }

    /// Requests a search. Never blocks: the request is queued; a running
    /// search is stopped first and only the latest request is started.
    pub fn search(
        &self,
        position: EnginePosition,
        search_id: SearchId,
        limits: SearchLimits,
    ) -> Result<(), EngineError> {
        position.validate()?;
        self.send(Cmd::Search {
            position,
            id: search_id,
            limits,
        })
    }

    /// Stops the running search (its `BestMove` still arrives) and cancels a
    /// queued one.
    pub fn stop(&self) -> Result<(), EngineError> {
        self.send(Cmd::Stop)
    }

    /// `setoption`. `value` is `None` for buttons. Validated against the
    /// declared options. Applied immediately when idle, otherwise at the next
    /// search boundary (UCI forbids `setoption` during a search) — to see the
    /// effect on the current analysis, call [`Self::analyze`] again.
    pub fn set_option(&self, name: &str, value: Option<&str>) -> Result<(), EngineError> {
        let opt = self
            .info
            .option(name)
            .ok_or_else(|| EngineError::UnknownOption(name.to_string()))?;
        let bad = |reason: String| EngineError::InvalidOptionValue {
            name: opt.name.clone(),
            reason,
        };
        if let Some(v) = value
            && v.chars().any(char::is_control)
        {
            return Err(bad("control characters in value".into()));
        }
        let value = match (&opt.kind, value) {
            (OptionKind::Button, None) => None,
            (OptionKind::Button, Some(_)) => return Err(bad("button takes no value".into())),
            (_, None) => return Err(bad("value required".into())),
            (OptionKind::Check { .. }, Some(v)) => match v {
                "true" | "false" => Some(v.to_string()),
                _ => return Err(bad(format!("expected true/false, got {v:?}"))),
            },
            (OptionKind::Spin { min, max, .. }, Some(v)) => {
                let n: i64 = v
                    .trim()
                    .parse()
                    .map_err(|_| bad(format!("not an integer: {v:?}")))?;
                if min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m) {
                    return Err(bad(format!("{n} outside {min:?}..={max:?}")));
                }
                Some(n.to_string())
            }
            (OptionKind::Combo { vars, .. }, Some(v)) => {
                let var = vars
                    .iter()
                    .find(|x| x.eq_ignore_ascii_case(v))
                    .ok_or_else(|| bad(format!("{v:?} is not one of {vars:?}")))?;
                Some(var.clone())
            }
            (OptionKind::String { .. }, Some(v)) => Some(v.to_string()),
        };
        let line = match value {
            Some(v) => format!("setoption name {} value {}", opt.name, v),
            None => format!("setoption name {}", opt.name),
        };
        self.send(Cmd::Setup(line))
    }

    pub fn set_multipv(&self, lines: u32) -> Result<(), EngineError> {
        self.set_option("MultiPV", Some(&lines.to_string()))
    }

    pub fn set_threads(&self, threads: u32) -> Result<(), EngineError> {
        self.set_option("Threads", Some(&threads.to_string()))
    }

    pub fn set_hash_mb(&self, mb: u32) -> Result<(), EngineError> {
        self.set_option("Hash", Some(&mb.to_string()))
    }

    /// `ucinewgame` (applied at the next idle point, followed by `isready`).
    pub fn new_game(&self) -> Result<(), EngineError> {
        self.send(Cmd::Setup("ucinewgame".into()))
    }

    /// Sends `stop`/`quit`, waits up to the quit timeout, then kills. Returns
    /// the exit status if the process was reaped.
    pub fn quit(mut self) -> Option<ExitStatus> {
        self.shutdown()
    }

    fn send(&self, cmd: Cmd) -> Result<(), EngineError> {
        if !self.is_running() {
            return Err(EngineError::NotRunning);
        }
        self.tx
            .send(Input::Cmd(cmd))
            .map_err(|_| EngineError::NotRunning)
    }

    fn shutdown(&mut self) -> Option<ExitStatus> {
        if let Some(join) = self.supervisor.take() {
            let _ = self.tx.send(Input::Cmd(Cmd::Quit));
            // The supervisor bounds its own shutdown by stop+quit timeouts;
            // the margin covers scheduling. If it is still stuck (e.g. a
            // blocked write), kill the process from here.
            let limit = self.timeouts.quit + self.timeouts.stop + Duration::from_millis(500);
            match self.done.recv_timeout(limit) {
                Err(RecvTimeoutError::Timeout) => {
                    let mut child = lock(&self.child);
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(child);
                    // Killing closes the pipes, so the supervisor will finish.
                    let _ = self.done.recv_timeout(Duration::from_secs(1));
                    if join.is_finished() {
                        let _ = join.join();
                    }
                }
                _ => {
                    let _ = join.join();
                }
            }
            self.alive.store(false, Ordering::Release);
        }
        let mut child = lock(&self.child);
        match child.try_wait() {
            Ok(Some(status)) => Some(status),
            _ => {
                // Should not happen; make sure no zombie is left behind.
                let _ = child.kill();
                child.wait().ok()
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn spawn_impl(cfg: EngineConfig, waker: Waker) -> Result<EngineHandle, EngineError> {
    let path = cfg.path.clone();
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(EngineError::NotFound(path));
        }
        Err(e) => {
            return Err(EngineError::NotExecutable {
                path,
                reason: e.to_string(),
            });
        }
        Ok(m) if m.is_dir() => {
            return Err(EngineError::NotExecutable {
                path,
                reason: "is a directory".into(),
            });
        }
        Ok(_) => {}
    }
    let workdir = cfg.working_dir.clone().or_else(|| {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
    });
    let mut command = Command::new(&path);
    command
        .args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = workdir {
        command.current_dir(dir);
    }
    #[cfg(unix)]
    if cfg.nice != 0 {
        use std::os::unix::process::CommandExt;
        let nice = cfg.nice;
        // SAFETY: nice(2) is async-signal-safe and touches no memory of the parent. Its
        // result is not checked: an engine at the app's priority still works.
        unsafe {
            command.pre_exec(move || {
                libc::nice(nice);
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        // Console engines would otherwise open a terminal window per process.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| {
        use std::io::ErrorKind as K;
        match e.kind() {
            K::NotFound => EngineError::NotFound(path.clone()),
            K::PermissionDenied => EngineError::NotExecutable {
                path: path.clone(),
                reason: e.to_string(),
            },
            // ENOEXEC: exec format error (wrong architecture, text file).
            _ if e.raw_os_error() == Some(8) => EngineError::NotExecutable {
                path: path.clone(),
                reason: e.to_string(),
            },
            _ => EngineError::Spawn(e),
        }
    })?;
    let pid = child.id();
    // Unwraps are safe: all three were configured as piped.
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (tx, rx) = mpsc::channel::<Input>();
    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));

    {
        let tx = tx.clone();
        thread::Builder::new()
            .name(format!("uci-stdout-{pid}"))
            .spawn(move || {
                read_lines(BufReader::new(stdout), |line| {
                    tx.send(Input::Stdout(line)).is_ok()
                });
                let _ = tx.send(Input::StdoutClosed);
            })
            .map_err(EngineError::Spawn)?;
    }
    {
        let tail = Arc::clone(&stderr_tail);
        thread::Builder::new()
            .name(format!("uci-stderr-{pid}"))
            .spawn(move || {
                read_lines(BufReader::new(stderr), |line| {
                    let mut t = lock(&tail);
                    if t.len() == STDERR_TAIL_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                    true
                });
            })
            .map_err(EngineError::Spawn)?;
    }

    let mut stdin = stdin;
    let mut warnings = Vec::new();
    let handshake = handshake(&mut stdin, &rx, &cfg.timeouts, &mut warnings);
    let info = match handshake {
        Ok(info) => info,
        Err(mut e) => {
            let _ = child.kill();
            let status = child.wait().ok();
            if let EngineError::ProcessExited {
                status: s,
                stderr: err,
            } = &mut e
            {
                *s = status;
                // Give the stderr thread a moment to collect the last words.
                thread::sleep(Duration::from_millis(20));
                *err = lock(&stderr_tail).iter().cloned().collect();
            }
            return Err(e);
        }
    };

    let (ev_tx, ev_rx) = mpsc::channel();
    let alive = Arc::new(AtomicBool::new(true));
    let child = Arc::new(Mutex::new(child));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let mut sup = Supervisor {
        stdin,
        child: Arc::clone(&child),
        events: ev_tx,
        waker,
        alive: Arc::clone(&alive),
        timeouts: cfg.timeouts,
        state: State::Idle,
        pending: None,
        pending_setup: Vec::new(),
        warnings: 0,
    };
    for w in warnings {
        sup.warn(w);
    }
    let supervisor = thread::Builder::new()
        .name(format!("uci-supervisor-{pid}"))
        .spawn(move || {
            let _done = done_tx;
            sup.run(rx);
        })
        .map_err(EngineError::Spawn)?;

    Ok(EngineHandle {
        info,
        pid,
        tx,
        events: ev_rx,
        alive,
        child,
        stderr: stderr_tail,
        supervisor: Some(supervisor),
        done: done_rx,
        timeouts: cfg.timeouts,
    })
}

fn write_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    stdin.write_all(&buf)?;
    stdin.flush()
}

fn handshake(
    stdin: &mut ChildStdin,
    rx: &Receiver<Input>,
    t: &Timeouts,
    warnings: &mut Vec<String>,
) -> Result<EngineInfo, EngineError> {
    let exited = || EngineError::ProcessExited {
        status: None,
        stderr: Vec::new(),
    };
    let io = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            exited()
        } else {
            EngineError::Io(e)
        }
    };
    write_line(stdin, "uci").map_err(io)?;
    let mut info = EngineInfo::default();
    let mut seen_uci = false;
    let mut output = Vec::new();
    let deadline = Instant::now() + t.uciok;
    loop {
        let line = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Input::Stdout(l)) => l,
            Ok(Input::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => return Err(exited()),
            Ok(Input::Cmd(_)) => continue,
            Err(RecvTimeoutError::Timeout) => {
                return Err(if seen_uci {
                    EngineError::HandshakeTimeout {
                        waiting_for: "uciok",
                    }
                } else {
                    EngineError::NotUci { output }
                });
            }
        };
        if output.len() < 20 {
            output.push(line.clone());
        }
        match parse_line(&line) {
            Ok(EngineLine::IdName(n)) => {
                seen_uci = true;
                info.name = Some(n);
            }
            Ok(EngineLine::IdAuthor(a)) => {
                seen_uci = true;
                info.author = Some(a);
            }
            Ok(EngineLine::Option(o)) => {
                seen_uci = true;
                info.options.push(o);
            }
            Ok(EngineLine::UciOk) => break,
            Ok(_) => {}
            Err(e) => {
                if line.starts_with("option") {
                    seen_uci = true;
                }
                warnings.push(format!("handshake: {e}"));
            }
        }
    }
    write_line(stdin, "isready").map_err(io)?;
    let deadline = Instant::now() + t.readyok;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Input::Stdout(l)) => {
                if matches!(parse_line(&l), Ok(EngineLine::ReadyOk)) {
                    return Ok(info);
                }
            }
            Ok(Input::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => return Err(exited()),
            Ok(Input::Cmd(_)) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Err(EngineError::HandshakeTimeout {
                    waiting_for: "readyok",
                });
            }
        }
    }
}

/// Reads `\n`-terminated lines with a length cap; stops at EOF/error or when
/// `f` returns false.
fn read_lines<R: Read>(mut r: BufReader<R>, mut f: impl FnMut(String) -> bool) {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let mut eof = false;
        loop {
            let avail = match r.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    eof = true;
                    break;
                }
            };
            if avail.is_empty() {
                eof = true;
                break;
            }
            let (chunk, used, done) = match avail.iter().position(|&b| b == b'\n') {
                Some(i) => (&avail[..i], i + 1, true),
                None => (avail, avail.len(), false),
            };
            let room = MAX_LINE_BYTES.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
            r.consume(used);
            if done {
                break;
            }
        }
        if eof && buf.is_empty() {
            return;
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        if !f(String::from_utf8_lossy(&buf).into_owned()) || eof {
            return;
        }
    }
}

/// Supervisor state. Invariant: `position`/`go` is only written in
/// `Syncing → Searching`, i.e. after the previous search's `bestmove` *and* a
/// fresh `readyok`; therefore every `info` read while `Searching`/`Stopping`
/// belongs to the current search, and `info` read while `Idle`/`Syncing` is
/// stale and dropped.
#[derive(Debug, Clone, Copy)]
enum State {
    /// No search running, nothing pending.
    Idle,
    /// `isready` sent before starting the pending search; waiting `readyok`.
    Syncing {
        deadline: Instant,
    },
    Searching {
        id: SearchId,
    },
    /// `stop` sent; waiting for the final `bestmove` of `id`.
    Stopping {
        id: SearchId,
        deadline: Instant,
    },
}

impl State {
    fn deadline(self) -> Option<Instant> {
        match self {
            State::Syncing { deadline } | State::Stopping { deadline, .. } => Some(deadline),
            _ => None,
        }
    }
}

struct PendingSearch {
    position: EnginePosition,
    id: SearchId,
    limits: SearchLimits,
}

struct Supervisor {
    stdin: ChildStdin,
    child: Arc<Mutex<Child>>,
    events: Sender<EngineEvent>,
    waker: Waker,
    alive: Arc<AtomicBool>,
    timeouts: Timeouts,
    state: State,
    pending: Option<PendingSearch>,
    pending_setup: Vec<String>,
    warnings: usize,
}

/// Why the loop must end.
enum Exit {
    ProcessGone,
    Hung(&'static str),
}

impl Supervisor {
    fn emit(&self, ev: EngineEvent) {
        if self.events.send(ev).is_ok() {
            (self.waker)();
        }
    }

    fn warn(&mut self, msg: String) {
        self.warnings += 1;
        if self.warnings < MAX_WARNINGS {
            self.emit(EngineEvent::Warning(msg));
        } else if self.warnings == MAX_WARNINGS {
            self.emit(EngineEvent::Warning(
                "too many warnings; further warnings suppressed".into(),
            ));
        }
    }

    fn write(&mut self, line: &str) -> Result<(), Exit> {
        write_line(&mut self.stdin, line).map_err(|_| Exit::ProcessGone)
    }

    fn run(mut self, rx: Receiver<Input>) {
        let result = loop {
            let msg = match self.state.deadline() {
                Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let step = match msg {
                Ok(Input::Cmd(Cmd::Quit)) | Err(RecvTimeoutError::Disconnected) => {
                    self.quit();
                    return;
                }
                Ok(Input::Cmd(cmd)) => self.on_cmd(cmd),
                Ok(Input::Stdout(line)) => self.on_line(&line),
                Ok(Input::StdoutClosed) => Err(Exit::ProcessGone),
                Err(RecvTimeoutError::Timeout) => match self.state {
                    State::Syncing { deadline } if Instant::now() >= deadline => {
                        Err(Exit::Hung("readyok"))
                    }
                    State::Stopping { deadline, .. } if Instant::now() >= deadline => {
                        Err(Exit::Hung("bestmove"))
                    }
                    _ => Ok(()),
                },
            };
            if let Err(e) = step {
                break e;
            }
        };
        self.alive.store(false, Ordering::Release);
        let reason = match result {
            Exit::Hung(waiting_for) => {
                let mut child = lock(&self.child);
                let _ = child.kill();
                let _ = child.wait();
                TerminationReason::Hung { waiting_for }
            }
            Exit::ProcessGone => TerminationReason::Exited {
                status: self.reap(Duration::from_millis(500)),
            },
        };
        self.emit(EngineEvent::Terminated(reason));
    }

    /// Waits up to `grace` for the process to exit, then kills it. Always reaps.
    fn reap(&self, grace: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + grace;
        loop {
            {
                let mut child = lock(&self.child);
                match child.try_wait() {
                    Ok(Some(s)) => return Some(s),
                    Ok(None) if Instant::now() < deadline => {}
                    _ => {
                        let _ = child.kill();
                        return child.wait().ok();
                    }
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn quit(&mut self) {
        self.alive.store(false, Ordering::Release);
        if matches!(self.state, State::Searching { .. }) {
            let _ = self.write("stop");
        }
        let _ = self.write("quit");
        let status = self.reap(self.timeouts.quit);
        self.emit(EngineEvent::Terminated(TerminationReason::Quit { status }));
    }

    fn on_cmd(&mut self, cmd: Cmd) -> Result<(), Exit> {
        match cmd {
            Cmd::Search {
                position,
                id,
                limits,
            } => {
                if let Some(old) = self.pending.replace(PendingSearch {
                    position,
                    id,
                    limits,
                }) {
                    self.emit(EngineEvent::Superseded { search_id: old.id });
                }
                match self.state {
                    State::Idle => self.begin_sync()?,
                    State::Searching { id } => self.send_stop(id)?,
                    State::Syncing { .. } | State::Stopping { .. } => {}
                }
            }
            Cmd::Stop => {
                if let Some(old) = self.pending.take() {
                    self.emit(EngineEvent::Superseded { search_id: old.id });
                }
                if let State::Searching { id } = self.state {
                    self.send_stop(id)?;
                }
            }
            Cmd::Setup(line) => match self.state {
                // The engine is not searching: safe to send right away; the
                // `isready` before the next `go` fences it.
                State::Idle | State::Syncing { .. } => self.write(&line)?,
                State::Searching { .. } | State::Stopping { .. } => self.pending_setup.push(line),
            },
            Cmd::Quit => unreachable!("handled in run"),
        }
        Ok(())
    }

    fn send_stop(&mut self, id: SearchId) -> Result<(), Exit> {
        self.write("stop")?;
        self.state = State::Stopping {
            id,
            deadline: Instant::now() + self.timeouts.stop,
        };
        Ok(())
    }

    /// Engine is idle: flush queued setup, then `isready` if a search waits.
    fn begin_sync(&mut self) -> Result<(), Exit> {
        for line in std::mem::take(&mut self.pending_setup) {
            self.write(&line)?;
        }
        if self.pending.is_some() {
            self.write("isready")?;
            self.state = State::Syncing {
                deadline: Instant::now() + self.timeouts.readyok,
            };
        } else {
            self.state = State::Idle;
        }
        Ok(())
    }

    fn current_search(&self) -> Option<SearchId> {
        match self.state {
            State::Searching { id } | State::Stopping { id, .. } => Some(id),
            _ => None,
        }
    }

    fn on_line(&mut self, line: &str) -> Result<(), Exit> {
        let parsed = match parse_line(line) {
            Ok(p) => p,
            Err(e) => {
                self.warn(e.to_string());
                return Ok(());
            }
        };
        match parsed {
            EngineLine::Info(info) => {
                if let Some(id) = self.current_search()
                    && (info.score.is_some() || !info.pv.is_empty())
                {
                    self.emit(EngineEvent::Info {
                        search_id: id,
                        info,
                    });
                }
                // Otherwise: stale output of a finished search, dropped.
            }
            EngineLine::InfoString(text) => {
                let search_id = self.current_search();
                self.emit(EngineEvent::InfoString { search_id, text });
            }
            EngineLine::BestMove { best, ponder } => match self.current_search() {
                Some(id) => {
                    self.emit(EngineEvent::BestMove {
                        search_id: id,
                        best,
                        ponder,
                    });
                    self.state = State::Idle;
                    self.begin_sync()?;
                }
                None => self.warn(format!("unexpected bestmove ignored: {line:?}")),
            },
            EngineLine::ReadyOk => {
                if let State::Syncing { .. } = self.state {
                    match self.pending.take() {
                        Some(p) => {
                            self.write(&p.position.to_command())?;
                            self.write(&p.limits.to_command())?;
                            self.state = State::Searching { id: p.id };
                            self.emit(EngineEvent::SearchStarted { search_id: p.id });
                        }
                        None => self.state = State::Idle,
                    }
                }
            }
            EngineLine::Empty => {}
            EngineLine::UciOk
            | EngineLine::IdName(_)
            | EngineLine::IdAuthor(_)
            | EngineLine::Option(_)
            | EngineLine::Unknown(_) => {
                self.warn(format!("unexpected engine output: {line:?}"));
            }
        }
        Ok(())
    }
}
