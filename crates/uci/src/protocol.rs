//! The UCI protocol as a state machine without I/O: the handshake and the session
//! after it. A transport (a child process on a computer, a Web Worker in a browser)
//! feeds it the engine's lines, the user's commands and the time, and carries out
//! what it asks: lines to write, events to deliver. Waiting is the transport's
//! business — the machine never blocks, it only names its next deadline.

use web_time::Instant;

use crate::error::EngineError;
use crate::events::EngineEvent;
use crate::parse::{EngineLine, parse_line};
use crate::types::{EngineInfo, EnginePosition, SearchId, SearchLimits, Timeouts};

/// Maximum number of `Warning` events emitted per engine (flood protection).
const MAX_WARNINGS: usize = 100;

/// A command from the handle to the session.
pub(crate) enum Cmd {
    Search {
        position: EnginePosition,
        id: SearchId,
        limits: SearchLimits,
        among: Vec<String>,
    },
    Stop,
    /// A complete `setoption ...` / `ucinewgame` line, applied while idle.
    Setup(String),
}

/// The engine could not be written to: it is gone.
#[derive(Debug)]
pub(crate) struct Gone;

/// What the session does to the outside world.
pub(crate) trait Link {
    /// Writes one line (without its `\n`) to the engine.
    fn write(&mut self, line: &str) -> Result<(), Gone>;
    /// Delivers an event to the handle (and wakes the UI).
    fn emit(&mut self, ev: EngineEvent);
}

/// The step of the handshake: `uci` → `uciok`, then `isready` → `readyok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Uci,
    Ready,
}

/// `uci`/`uciok` and `isready`/`readyok`: the engine's name, author and options.
pub(crate) struct Handshake {
    phase: Phase,
    deadline: Instant,
    readyok: std::time::Duration,
    info: EngineInfo,
    seen_uci: bool,
    /// First lines of output, for [`EngineError::NotUci`].
    output: Vec<String>,
    /// Malformed lines, reported once the session runs.
    warnings: Vec<String>,
}

/// How a line moved the handshake on.
pub(crate) enum HandshakeStep {
    /// Keep feeding lines.
    Continue,
    /// Write this line, then keep feeding lines.
    Write(&'static str),
    /// Complete: the engine is ready for the session.
    Done(EngineInfo, Vec<String>),
}

impl Handshake {
    /// Starts the handshake at `now`; the caller writes [`Self::FIRST_LINE`].
    pub(crate) fn new(t: &Timeouts, now: Instant) -> Self {
        Handshake {
            phase: Phase::Uci,
            deadline: now + t.uciok,
            readyok: t.readyok,
            info: EngineInfo::default(),
            seen_uci: false,
            output: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub(crate) const FIRST_LINE: &'static str = "uci";

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// One line of the engine's output, read at `now`.
    pub(crate) fn on_line(&mut self, line: String, now: Instant) -> HandshakeStep {
        match self.phase {
            Phase::Uci => {
                if self.output.len() < 20 {
                    self.output.push(line.clone());
                }
                match parse_line(&line) {
                    Ok(EngineLine::IdName(n)) => {
                        self.seen_uci = true;
                        self.info.name = Some(n);
                    }
                    Ok(EngineLine::IdAuthor(a)) => {
                        self.seen_uci = true;
                        self.info.author = Some(a);
                    }
                    Ok(EngineLine::Option(o)) => {
                        self.seen_uci = true;
                        self.info.options.push(o);
                    }
                    Ok(EngineLine::UciOk) => {
                        self.phase = Phase::Ready;
                        self.deadline = now + self.readyok;
                        return HandshakeStep::Write("isready");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if line.starts_with("option") {
                            self.seen_uci = true;
                        }
                        self.warnings.push(format!("handshake: {e}"));
                    }
                }
                HandshakeStep::Continue
            }
            Phase::Ready => {
                if matches!(parse_line(&line), Ok(EngineLine::ReadyOk)) {
                    HandshakeStep::Done(
                        std::mem::take(&mut self.info),
                        std::mem::take(&mut self.warnings),
                    )
                } else {
                    HandshakeStep::Continue
                }
            }
        }
    }

    /// The error once [`Self::deadline`] has passed.
    pub(crate) fn timed_out(self) -> EngineError {
        match self.phase {
            Phase::Uci if self.seen_uci => EngineError::HandshakeTimeout {
                waiting_for: "uciok",
            },
            Phase::Uci => EngineError::NotUci {
                output: self.output,
            },
            Phase::Ready => EngineError::HandshakeTimeout {
                waiting_for: "readyok",
            },
        }
    }
}

/// Session state. Invariant: `position`/`go` is only written in
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

struct PendingSearch {
    position: EnginePosition,
    id: SearchId,
    limits: SearchLimits,
    among: Vec<String>,
}

/// Why the session must end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// The engine stopped reading or writing.
    Gone,
    /// The engine did not answer this in time; the transport kills it.
    Hung(&'static str),
}

impl From<Gone> for Exit {
    fn from(_: Gone) -> Self {
        Exit::Gone
    }
}

/// The session after the handshake: searches, their boundaries, setup lines.
pub(crate) struct Session {
    timeouts: Timeouts,
    state: State,
    pending: Option<PendingSearch>,
    pending_setup: Vec<String>,
    warnings: usize,
}

impl Session {
    pub(crate) fn new(timeouts: Timeouts) -> Self {
        Session {
            timeouts,
            state: State::Idle,
            pending: None,
            pending_setup: Vec::new(),
            warnings: 0,
        }
    }

    /// When [`Self::on_time`] must be called next (`None`: nothing to wait for).
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Syncing { deadline } | State::Stopping { deadline, .. } => Some(deadline),
            _ => None,
        }
    }

    /// The time is `now`: an engine past its deadline is hung.
    pub(crate) fn on_time(&mut self, now: Instant) -> Result<(), Exit> {
        match self.state {
            State::Syncing { deadline } if now >= deadline => Err(Exit::Hung("readyok")),
            State::Stopping { deadline, .. } if now >= deadline => Err(Exit::Hung("bestmove")),
            _ => Ok(()),
        }
    }

    pub(crate) fn warn(&mut self, msg: String, link: &mut impl Link) {
        self.warnings += 1;
        if self.warnings < MAX_WARNINGS {
            link.emit(EngineEvent::Warning(msg));
        } else if self.warnings == MAX_WARNINGS {
            link.emit(EngineEvent::Warning(
                "too many warnings; further warnings suppressed".into(),
            ));
        }
    }

    /// The last words before `quit`: a running search is stopped.
    pub(crate) fn quit(&mut self, link: &mut impl Link) {
        if matches!(self.state, State::Searching { .. }) {
            let _ = link.write("stop");
        }
        let _ = link.write("quit");
    }

    pub(crate) fn on_cmd(
        &mut self,
        cmd: Cmd,
        now: Instant,
        link: &mut impl Link,
    ) -> Result<(), Exit> {
        match cmd {
            Cmd::Search {
                position,
                id,
                limits,
                among,
            } => {
                if let Some(old) = self.pending.replace(PendingSearch {
                    position,
                    id,
                    limits,
                    among,
                }) {
                    link.emit(EngineEvent::Superseded { search_id: old.id });
                }
                match self.state {
                    State::Idle => self.begin_sync(now, link)?,
                    State::Searching { id } => self.send_stop(id, now, link)?,
                    State::Syncing { .. } | State::Stopping { .. } => {}
                }
            }
            Cmd::Stop => {
                if let Some(old) = self.pending.take() {
                    link.emit(EngineEvent::Superseded { search_id: old.id });
                }
                if let State::Searching { id } = self.state {
                    self.send_stop(id, now, link)?;
                }
            }
            Cmd::Setup(line) => match self.state {
                // The engine is not searching: safe to send right away; the
                // `isready` before the next `go` fences it.
                State::Idle | State::Syncing { .. } => link.write(&line)?,
                State::Searching { .. } | State::Stopping { .. } => self.pending_setup.push(line),
            },
        }
        Ok(())
    }

    fn send_stop(&mut self, id: SearchId, now: Instant, link: &mut impl Link) -> Result<(), Exit> {
        link.write("stop")?;
        self.state = State::Stopping {
            id,
            deadline: now + self.timeouts.stop,
        };
        Ok(())
    }

    /// Engine is idle: flush queued setup, then `isready` if a search waits.
    fn begin_sync(&mut self, now: Instant, link: &mut impl Link) -> Result<(), Exit> {
        for line in std::mem::take(&mut self.pending_setup) {
            link.write(&line)?;
        }
        if self.pending.is_some() {
            link.write("isready")?;
            self.state = State::Syncing {
                deadline: now + self.timeouts.readyok,
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

    /// One line of the engine's output, read at `now`.
    pub(crate) fn on_line(
        &mut self,
        line: &str,
        now: Instant,
        link: &mut impl Link,
    ) -> Result<(), Exit> {
        let parsed = match parse_line(line) {
            Ok(p) => p,
            Err(e) => {
                self.warn(e.to_string(), link);
                return Ok(());
            }
        };
        match parsed {
            EngineLine::Info(info) => {
                if let Some(id) = self.current_search()
                    && (info.score.is_some() || !info.pv.is_empty())
                {
                    link.emit(EngineEvent::Info {
                        search_id: id,
                        info,
                    });
                }
                // Otherwise: stale output of a finished search, dropped.
            }
            EngineLine::InfoString(text) => {
                let search_id = self.current_search();
                link.emit(EngineEvent::InfoString { search_id, text });
            }
            EngineLine::BestMove { best, ponder } => match self.current_search() {
                Some(id) => {
                    link.emit(EngineEvent::BestMove {
                        search_id: id,
                        best,
                        ponder,
                    });
                    self.state = State::Idle;
                    self.begin_sync(now, link)?;
                }
                None => self.warn(format!("unexpected bestmove ignored: {line:?}"), link),
            },
            EngineLine::ReadyOk => {
                if let State::Syncing { .. } = self.state {
                    match self.pending.take() {
                        Some(p) => {
                            link.write(&p.position.to_command())?;
                            let mut go = p.limits.to_command();
                            if !p.among.is_empty() {
                                go += " searchmoves ";
                                go += &p.among.join(" ");
                            }
                            link.write(&go)?;
                            self.state = State::Searching { id: p.id };
                            link.emit(EngineEvent::SearchStarted { search_id: p.id });
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
                self.warn(format!("unexpected engine output: {line:?}"), link);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A link that records what the session did.
    #[derive(Default)]
    struct Record {
        written: Vec<String>,
        events: Vec<EngineEvent>,
        broken: bool,
    }

    impl Link for Record {
        fn write(&mut self, line: &str) -> Result<(), Gone> {
            if self.broken {
                return Err(Gone);
            }
            self.written.push(line.to_owned());
            Ok(())
        }
        fn emit(&mut self, ev: EngineEvent) {
            self.events.push(ev);
        }
    }

    fn search(id: u64) -> Cmd {
        Cmd::Search {
            position: EnginePosition::startpos(Vec::<String>::new()),
            id: SearchId(id),
            limits: SearchLimits::Infinite,
            among: Vec::new(),
        }
    }

    #[test]
    fn handshake_collects_info_and_asks_ready() {
        let t0 = Instant::now();
        let mut h = Handshake::new(&Timeouts::default(), t0);
        assert!(matches!(
            h.on_line("id name Fake".into(), t0),
            HandshakeStep::Continue
        ));
        assert!(matches!(
            h.on_line(
                "option name Hash type spin default 16 min 1 max 1024".into(),
                t0
            ),
            HandshakeStep::Continue
        ));
        let later = t0 + Duration::from_secs(1);
        assert!(matches!(
            h.on_line("uciok".into(), later),
            HandshakeStep::Write("isready")
        ));
        assert_eq!(h.deadline(), later + Timeouts::default().readyok);
        // Output before `readyok` is not taken for the engine's description.
        assert!(matches!(
            h.on_line("id name Other".into(), later),
            HandshakeStep::Continue
        ));
        match h.on_line("readyok".into(), later) {
            HandshakeStep::Done(info, warnings) => {
                assert_eq!(info.name.as_deref(), Some("Fake"));
                assert_eq!(info.options.len(), 1);
                assert!(warnings.is_empty());
            }
            _ => panic!("handshake not done"),
        }
    }

    #[test]
    fn handshake_timeouts_name_what_was_missing() {
        let t0 = Instant::now();
        let silent = Handshake::new(&Timeouts::default(), t0);
        assert!(matches!(silent.timed_out(), EngineError::NotUci { .. }));
        let mut uci = Handshake::new(&Timeouts::default(), t0);
        let _ = uci.on_line("id name Fake".into(), t0);
        assert!(matches!(
            uci.timed_out(),
            EngineError::HandshakeTimeout {
                waiting_for: "uciok"
            }
        ));
        let mut ready = Handshake::new(&Timeouts::default(), t0);
        let _ = ready.on_line("uciok".into(), t0);
        assert!(matches!(
            ready.timed_out(),
            EngineError::HandshakeTimeout {
                waiting_for: "readyok"
            }
        ));
    }

    #[test]
    fn a_search_waits_for_readyok_and_stale_info_is_dropped() {
        let now = Instant::now();
        let mut s = Session::new(Timeouts::default());
        let mut link = Record::default();
        s.on_cmd(search(1), now, &mut link).unwrap();
        assert_eq!(link.written, ["isready"]);
        // Info before the search starts belongs to nobody.
        s.on_line("info depth 1 score cp 5 pv e2e4", now, &mut link)
            .unwrap();
        assert!(link.events.is_empty());
        s.on_line("readyok", now, &mut link).unwrap();
        assert_eq!(link.written[1], "position startpos");
        assert_eq!(link.written[2], "go infinite");
        assert_eq!(
            link.events,
            [EngineEvent::SearchStarted {
                search_id: SearchId(1)
            }]
        );
        // A newer request stops the running search; the old `bestmove` ends it.
        s.on_cmd(search(2), now, &mut link).unwrap();
        assert_eq!(link.written[3], "stop");
        s.on_line("bestmove e2e4", now, &mut link).unwrap();
        assert_eq!(link.written[4], "isready");
        s.on_line("readyok", now, &mut link).unwrap();
        assert!(link.events.contains(&EngineEvent::SearchStarted {
            search_id: SearchId(2)
        }));
    }

    #[test]
    fn deadlines_and_a_broken_link_end_the_session() {
        let now = Instant::now();
        let mut s = Session::new(Timeouts::default());
        let mut link = Record::default();
        assert_eq!(s.deadline(), None);
        s.on_cmd(search(1), now, &mut link).unwrap();
        let d = s.deadline().expect("waiting for readyok");
        assert_eq!(s.on_time(now), Ok(()));
        assert_eq!(s.on_time(d), Err(Exit::Hung("readyok")));
        link.broken = true;
        assert_eq!(s.on_line("readyok", now, &mut link), Err(Exit::Gone));
    }
}
