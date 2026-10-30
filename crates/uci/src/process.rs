//! The engine as a child process: started without a shell, its stdout and stderr
//! read by two threads, the protocol ([`crate::protocol`]) driven by a supervisor
//! thread that waits for input with the session's deadline.

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
use crate::events::{EngineEvent, TerminationReason, Waker};
use crate::protocol::{Cmd, Exit, Gone, Handshake, HandshakeStep, Link, Session, option_line};
use crate::types::{EngineInfo, EnginePosition, SearchId, SearchLimits, Timeouts};

/// Maximum stored length of one output line; longer lines are truncated.
const MAX_LINE_BYTES: usize = 64 * 1024;
/// Number of stderr lines kept for diagnostics.
const STDERR_TAIL_LINES: usize = 256;

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
    /// Variables added to the engine's environment (Rodent IV on Android finds its
    /// personalities and books by `RODENT4HOME`).
    pub env: Vec<(OsString, OsString)>,
}

impl EngineConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        EngineConfig {
            path: path.into(),
            args: Vec::new(),
            working_dir: None,
            timeouts: Timeouts::default(),
            nice: 0,
            env: Vec::new(),
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
    Quit,
    Stdout(String),
    StdoutClosed,
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
        self.search_among(position, search_id, limits, Vec::new())
    }

    /// A search among these moves only (`go … searchmoves`, UCI moves of `position`);
    /// empty: all of them. A move that is not a UCI move is refused.
    pub fn search_among(
        &self,
        position: EnginePosition,
        search_id: SearchId,
        limits: SearchLimits,
        among: Vec<String>,
    ) -> Result<(), EngineError> {
        self.send(Cmd::search(position, search_id, limits, among)?)
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
        self.send(Cmd::Setup(option_line(&self.info, name, value)?))
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
            let _ = self.tx.send(Input::Quit);
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
        .envs(cfg.env.iter().map(|(k, v)| (k, v)))
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
    let (info, warnings) = match handshake(&mut stdin, &rx, &cfg.timeouts) {
        Ok(done) => done,
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
        pipes: Pipes {
            stdin,
            events: ev_tx,
            waker,
        },
        session: Session::new(cfg.timeouts),
        child: Arc::clone(&child),
        alive: Arc::clone(&alive),
        timeouts: cfg.timeouts,
    };
    for w in warnings {
        sup.session.warn(w, &mut sup.pipes);
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

/// The handshake over the pipes: every line read goes to [`Handshake`], until it is
/// complete or its deadline passes.
fn handshake(
    stdin: &mut ChildStdin,
    rx: &Receiver<Input>,
    t: &Timeouts,
) -> Result<(EngineInfo, Vec<String>), EngineError> {
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
    let mut hs = Handshake::new(t, Instant::now());
    write_line(stdin, Handshake::FIRST_LINE).map_err(io)?;
    loop {
        let wait = hs.deadline().saturating_duration_since(Instant::now());
        let line = match rx.recv_timeout(wait) {
            Ok(Input::Stdout(l)) => l,
            Ok(Input::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => return Err(exited()),
            Ok(Input::Cmd(_) | Input::Quit) => continue,
            Err(RecvTimeoutError::Timeout) => return Err(hs.timed_out()),
        };
        match hs.on_line(line, Instant::now()) {
            HandshakeStep::Continue => {}
            HandshakeStep::Write(l) => write_line(stdin, l).map_err(io)?,
            HandshakeStep::Done(info, warnings) => return Ok((info, warnings)),
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

/// The process side of the session: the engine's stdin and the events' channel.
struct Pipes {
    stdin: ChildStdin,
    events: Sender<EngineEvent>,
    waker: Waker,
}

impl Link for Pipes {
    fn write(&mut self, line: &str) -> Result<(), Gone> {
        write_line(&mut self.stdin, line).map_err(|_| Gone)
    }

    fn emit(&mut self, ev: EngineEvent) {
        if self.events.send(ev).is_ok() {
            (self.waker)();
        }
    }
}

struct Supervisor {
    pipes: Pipes,
    session: Session,
    child: Arc<Mutex<Child>>,
    alive: Arc<AtomicBool>,
    timeouts: Timeouts,
}

impl Supervisor {
    fn run(mut self, rx: Receiver<Input>) {
        let result = loop {
            let msg = match self.session.deadline() {
                Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let now = Instant::now();
            let step = match msg {
                Ok(Input::Quit) | Err(RecvTimeoutError::Disconnected) => {
                    self.quit();
                    return;
                }
                Ok(Input::Cmd(cmd)) => self.session.on_cmd(cmd, now, &mut self.pipes),
                Ok(Input::Stdout(line)) => self.session.on_line(&line, now, &mut self.pipes),
                Ok(Input::StdoutClosed) => Err(Exit::Gone),
                Err(RecvTimeoutError::Timeout) => self.session.on_time(now),
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
            Exit::Gone => TerminationReason::Exited {
                status: self.reap(Duration::from_millis(500)),
            },
        };
        self.pipes.emit(EngineEvent::Terminated(reason));
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
        self.session.quit(&mut self.pipes);
        let status = self.reap(self.timeouts.quit);
        self.pipes
            .emit(EngineEvent::Terminated(TerminationReason::Quit { status }));
    }
}
