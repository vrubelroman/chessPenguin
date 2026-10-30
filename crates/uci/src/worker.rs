//! The engine as a Web Worker (WebAssembly in a browser, which has no processes):
//! the engine's script runs in the worker, a line of UCI is one message each way,
//! and the protocol ([`crate::protocol`]) is driven by the worker's messages and by
//! timers set for its deadlines. Everything happens on the page's thread, so nothing
//! here blocks: the handshake is finished by the worker's messages and its outcome
//! taken by [`Starting::poll`].

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_time::Instant;

use crate::error::EngineError;
use crate::events::{EngineEvent, TerminationReason, Waker};
use crate::protocol::{Cmd, Exit, Gone, Handshake, HandshakeStep, Link, Session, option_line};
use crate::types::{EngineInfo, EnginePosition, SearchId, SearchLimits, Timeouts};

#[wasm_bindgen]
extern "C" {
    // The global timer functions: a page's and a worker's alike.
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(f: &js_sys::Function, ms: f64) -> JsValue;
    #[wasm_bindgen(js_name = clearTimeout)]
    fn clear_timeout(id: &JsValue);
}

/// How to start an engine in a worker.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// The URL of the engine's worker script.
    pub script: String,
    pub timeouts: Timeouts,
}

impl WorkerConfig {
    pub fn new(script: impl Into<String>) -> Self {
        WorkerConfig {
            script: script.into(),
            timeouts: Timeouts::default(),
        }
    }
}

/// The worker side of the session: the worker itself and the events' queue.
struct Port {
    worker: web_sys::Worker,
    events: VecDeque<EngineEvent>,
    waker: Waker,
}

impl Link for Port {
    fn write(&mut self, line: &str) -> Result<(), Gone> {
        self.worker
            .post_message(&JsValue::from_str(line))
            .map_err(|_| Gone)
    }

    fn emit(&mut self, ev: EngineEvent) {
        self.events.push_back(ev);
        (self.waker)();
    }
}

enum Phase {
    Handshake(Handshake),
    Running(Session),
    Ended,
}

struct Inner {
    port: Port,
    phase: Phase,
    timeouts: Timeouts,
    /// The outcome of the handshake until [`Starting::poll`] takes it.
    started: Option<Result<EngineInfo, EngineError>>,
    /// The timer of the protocol's next deadline.
    timer: Option<JsValue>,
    /// The worker's handlers and the timer's callback, alive as long as the engine.
    callbacks: Vec<Closure<dyn FnMut(JsValue)>>,
    on_timer: Option<Closure<dyn FnMut()>>,
}

impl Inner {
    fn deadline(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Handshake(h) => Some(h.deadline()),
            Phase::Running(s) => s.deadline(),
            Phase::Ended => None,
        }
    }

    /// Sets the timer for the protocol's next deadline (clearing the old one).
    fn arm(&mut self) {
        if let Some(id) = self.timer.take() {
            clear_timeout(&id);
        }
        let (Some(at), Some(f)) = (self.deadline(), &self.on_timer) else {
            return;
        };
        let ms = at.saturating_duration_since(Instant::now()).as_secs_f64() * 1000.0;
        // A millisecond late rather than early: the deadline must have passed.
        self.timer = Some(set_timeout(f.as_ref().unchecked_ref(), ms.ceil() + 1.0));
    }

    fn on_line(&mut self, line: String) {
        let now = Instant::now();
        match &mut self.phase {
            Phase::Handshake(h) => match h.on_line(line, now) {
                HandshakeStep::Continue => {}
                HandshakeStep::Write(l) => {
                    if self.port.write(l).is_err() {
                        self.fail_start(gone(Vec::new()));
                    }
                }
                HandshakeStep::Done(info, warnings) => {
                    let mut session = Session::new(self.timeouts);
                    for w in warnings {
                        session.warn(w, &mut self.port);
                    }
                    self.phase = Phase::Running(session);
                    self.started = Some(Ok(info));
                    (self.port.waker)();
                }
            },
            Phase::Running(s) => {
                if let Err(e) = s.on_line(&line, now, &mut self.port) {
                    self.end(e);
                }
            }
            Phase::Ended => {}
        }
        self.arm();
    }

    fn on_cmd(&mut self, cmd: Cmd) -> Result<(), EngineError> {
        let Phase::Running(s) = &mut self.phase else {
            return Err(EngineError::NotRunning);
        };
        if let Err(e) = s.on_cmd(cmd, Instant::now(), &mut self.port) {
            self.end(e);
        }
        self.arm();
        Ok(())
    }

    fn on_timer(&mut self) {
        self.timer = None;
        let now = Instant::now();
        match &mut self.phase {
            Phase::Handshake(h) if now >= h.deadline() => {
                let Phase::Handshake(h) = std::mem::replace(&mut self.phase, Phase::Ended) else {
                    unreachable!("matched above");
                };
                self.port.worker.terminate();
                self.started = Some(Err(h.timed_out()));
                (self.port.waker)();
            }
            Phase::Running(s) => {
                if let Err(e) = s.on_time(now) {
                    self.end(e);
                }
            }
            Phase::Handshake(_) | Phase::Ended => {}
        }
        self.arm();
    }

    /// The worker reported an error: its script did not load or the engine failed.
    fn on_error(&mut self, message: String) {
        match self.phase {
            Phase::Handshake(_) => self.fail_start(gone(vec![message])),
            Phase::Running(_) => self.end(Exit::Gone),
            Phase::Ended => {}
        }
        self.arm();
    }

    fn fail_start(&mut self, e: EngineError) {
        self.port.worker.terminate();
        self.phase = Phase::Ended;
        self.started = Some(Err(e));
        (self.port.waker)();
    }

    /// The session ended on its own; the worker is stopped.
    fn end(&mut self, exit: Exit) {
        self.port.worker.terminate();
        self.phase = Phase::Ended;
        let reason = match exit {
            Exit::Hung(waiting_for) => TerminationReason::Hung { waiting_for },
            Exit::Gone => TerminationReason::Exited { status: None },
        };
        self.port.emit(EngineEvent::Terminated(reason));
    }

    fn quit(&mut self) {
        if let Phase::Running(s) = &mut self.phase {
            s.quit(&mut self.port);
            self.port.worker.terminate();
            self.phase = Phase::Ended;
            self.port
                .emit(EngineEvent::Terminated(TerminationReason::Quit { status: None }));
        } else {
            self.port.worker.terminate();
            self.phase = Phase::Ended;
        }
        self.arm();
        self.port.worker.set_onmessage(None);
        self.port.worker.set_onerror(None);
    }
}

fn gone(stderr: Vec<String>) -> EngineError {
    EngineError::ProcessExited {
        status: None,
        stderr,
    }
}

/// An engine whose handshake is under way.
pub struct Starting {
    inner: Option<Rc<RefCell<Inner>>>,
}

impl Starting {
    /// The engine once its handshake is over (`None`: not yet). After a result it
    /// keeps returning `None`.
    pub fn poll(&mut self) -> Option<Result<EngineHandle, EngineError>> {
        let started = self.inner.as_ref()?.borrow_mut().started.take()?;
        let inner = self.inner.take()?;
        Some(started.map(|info| EngineHandle { info, inner }))
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            inner.borrow_mut().quit();
        }
    }
}

/// Namespace for starting engines.
pub struct Engine;

impl Engine {
    /// Starts the engine of `cfg.script` in a new worker, and its handshake. The
    /// worker's events wake the UI through `waker`; [`Starting::poll`] gives the
    /// engine once it answered `readyok`.
    pub fn start_worker(cfg: WorkerConfig, waker: Waker) -> Result<Starting, EngineError> {
        start(cfg, waker)
    }
}

fn start(cfg: WorkerConfig, waker: Waker) -> Result<Starting, EngineError> {
    let script_error = |e: JsValue| {
        EngineError::Spawn(std::io::Error::other(
            e.as_string()
                .unwrap_or_else(|| format!("cannot start the worker {}", cfg.script)),
        ))
    };
    let worker = web_sys::Worker::new(&cfg.script).map_err(script_error)?;
    let inner = Rc::new(RefCell::new(Inner {
        port: Port {
            worker: worker.clone(),
            events: VecDeque::new(),
            waker,
        },
        phase: Phase::Handshake(Handshake::new(&cfg.timeouts, Instant::now())),
        timeouts: cfg.timeouts,
        started: None,
        timer: None,
        callbacks: Vec::new(),
        on_timer: None,
    }));
    let weak = Rc::downgrade(&inner);
    let with = move |f: &dyn Fn(&mut Inner)| {
        if let Some(inner) = Weak::upgrade(&weak) {
            f(&mut inner.borrow_mut());
        }
    };
    let on_message = {
        let with = with.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            let data = ev.unchecked_into::<web_sys::MessageEvent>().data();
            // A message is a line; some builds join several with `\n`.
            let text = data.as_string().unwrap_or_default();
            with(&|inner| {
                for line in text.lines() {
                    inner.on_line(line.to_owned());
                }
            });
        })
    };
    let on_error = {
        let with = with.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            let message = ev
                .dyn_ref::<web_sys::ErrorEvent>()
                .map(web_sys::ErrorEvent::message)
                .unwrap_or_else(|| "the engine's worker failed".into());
            with(&|inner| inner.on_error(message.clone()));
        })
    };
    let on_timer = Closure::<dyn FnMut()>::new(move || with(&|inner| inner.on_timer()));
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    {
        let mut i = inner.borrow_mut();
        i.callbacks = vec![on_message, on_error];
        i.on_timer = Some(on_timer);
        if i.port.write(Handshake::FIRST_LINE).is_err() {
            i.fail_start(gone(Vec::new()));
        }
        i.arm();
    }
    Ok(Starting { inner: Some(inner) })
}

/// Handle owned by the UI. Dropping it quits the engine (its worker is stopped).
pub struct EngineHandle {
    info: EngineInfo,
    inner: Rc<RefCell<Inner>>,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("name", &self.info.name)
            .field("alive", &self.is_running())
            .finish()
    }
}

impl EngineHandle {
    /// `id name/author` and declared options from the handshake.
    pub fn info(&self) -> &EngineInfo {
        &self.info
    }

    /// `false` after the engine terminated (quit, crash, hung).
    pub fn is_running(&self) -> bool {
        matches!(self.inner.borrow().phase, Phase::Running(_))
    }

    /// Non-blocking poll for the next event (call every frame).
    pub fn try_recv(&self) -> Option<EngineEvent> {
        self.inner.borrow_mut().port.events.pop_front()
    }

    /// Drains all currently queued events.
    pub fn drain_events(&self) -> impl Iterator<Item = EngineEvent> + '_ {
        std::iter::from_fn(|| self.try_recv())
    }

    /// Infinite analysis (`go infinite`) of `position` under `search_id`.
    pub fn analyze(
        &self,
        position: EnginePosition,
        search_id: SearchId,
    ) -> Result<(), EngineError> {
        self.search(position, search_id, SearchLimits::Infinite)
    }

    /// Requests a search: a running search is stopped first and only the latest
    /// request is started.
    pub fn search(
        &self,
        position: EnginePosition,
        search_id: SearchId,
        limits: SearchLimits,
    ) -> Result<(), EngineError> {
        self.search_among(position, search_id, limits, Vec::new())
    }

    /// A search among these moves only (UCI moves of `position`); empty: all.
    pub fn search_among(
        &self,
        position: EnginePosition,
        search_id: SearchId,
        limits: SearchLimits,
        among: Vec<String>,
    ) -> Result<(), EngineError> {
        let cmd = Cmd::search(position, search_id, limits, among)?;
        self.inner.borrow_mut().on_cmd(cmd)
    }

    /// Stops the running search (its `BestMove` still arrives) and cancels a
    /// queued one.
    pub fn stop(&self) -> Result<(), EngineError> {
        self.inner.borrow_mut().on_cmd(Cmd::Stop)
    }

    /// `setoption`, checked against the declared options; applied at the next
    /// search boundary when a search runs.
    pub fn set_option(&self, name: &str, value: Option<&str>) -> Result<(), EngineError> {
        let line = option_line(&self.info, name, value)?;
        self.inner.borrow_mut().on_cmd(Cmd::Setup(line))
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
        self.inner
            .borrow_mut()
            .on_cmd(Cmd::Setup("ucinewgame".into()))
    }

    /// Sends `stop`/`quit` and stops the worker at once (a worker needs no grace
    /// time: nothing of it outlives `terminate`).
    pub fn quit(self) {}
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.inner.borrow_mut().quit();
    }
}
