//! The engine job in a browser: engines in Web Workers, each taking the next target,
//! driven from the page's frames by their messages — nothing here blocks. The search
//! of every target is the same machine as on a computer ([`super::work`]).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use uci::{
    EngineEvent, EngineHandle, ScoreBound, SearchId, SearchInfo, SearchLimits, Starting,
    WorkerConfig,
};
use web_time::Instant;

use super::work::{Next, Work};
use super::{JobEvent, JobFailure, JobSettings, MAX_PROCESSES, SEARCH_HASH_MB, Target};

/// A node-limited search should never take this long; stop it if it does.
const SEARCH_GUARD: Duration = Duration::from_secs(300);

/// How long an engine may take to start: a worker compiles the whole engine (≈ 100 MB
/// of WebAssembly), several at once.
const START_TIMEOUT: Duration = Duration::from_secs(120);

enum Engine {
    Starting(Starting),
    Ready(EngineHandle),
    Done,
}

struct Running {
    id: SearchId,
    last: Option<SearchInfo>,
    started: Instant,
    stopping: bool,
}

struct Slot {
    engine: Engine,
    work: Option<Work>,
    search: Option<Running>,
    next_id: u64,
}

/// The job: started by [`Dispatcher::start`], driven by [`Dispatcher::poll`].
pub struct Dispatcher {
    slots: Vec<Slot>,
    targets: std::vec::IntoIter<Target>,
    nodes: u64,
    names: HashSet<String>,
    events: Vec<JobEvent>,
    failure: Option<JobFailure>,
    halted: bool,
}

impl Dispatcher {
    /// Starts `settings.processes` engines of the worker script `script`; `wake` is
    /// called whenever one of them has something new.
    pub fn start(
        script: &str,
        settings: &JobSettings,
        targets: Vec<Target>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Dispatcher, JobFailure> {
        let workers = settings
            .processes
            .clamp(1, MAX_PROCESSES)
            .min(targets.len().max(1));
        let mut slots = Vec::with_capacity(workers);
        if !targets.is_empty() {
            for _ in 0..workers {
                let mut config = WorkerConfig::new(script);
                config.timeouts.uciok = START_TIMEOUT;
                config.timeouts.readyok = START_TIMEOUT;
                let wake = wake.clone();
                let starting = uci::Engine::start_worker(config, Box::new(move || wake()))
                    .map_err(|e| JobFailure::Start(e.to_string()))?;
                slots.push(Slot {
                    engine: Engine::Starting(starting),
                    work: None,
                    search: None,
                    next_id: 0,
                });
            }
        }
        Ok(Dispatcher {
            slots,
            targets: targets.into_iter(),
            nodes: settings.nodes,
            names: HashSet::new(),
            events: Vec::new(),
            failure: None,
            halted: false,
        })
    }

    /// Stops the job: the engines are stopped, what came stays.
    pub fn cancel(&mut self) {
        self.halted = true;
        for s in &mut self.slots {
            s.engine = Engine::Done;
        }
    }

    /// What came since the last call, and the end once the job is over (`Ok` also when
    /// cancelled). Call it every frame and when woken.
    pub fn poll(&mut self) -> (Vec<JobEvent>, Option<Result<(), JobFailure>>) {
        for i in 0..self.slots.len() {
            if let Err(e) = self.step(i) {
                self.failure.get_or_insert(e);
                self.cancel();
            }
        }
        let events = std::mem::take(&mut self.events);
        let over = self.slots.iter().all(|s| matches!(s.engine, Engine::Done));
        let end = over.then(|| match self.failure.take() {
            Some(e) => Err(e),
            None => Ok(()),
        });
        (events, end)
    }

    fn step(&mut self, i: usize) -> Result<(), JobFailure> {
        let slot = &mut self.slots[i];
        if let Engine::Starting(s) = &mut slot.engine {
            match s.poll() {
                None => return Ok(()),
                Some(Err(e)) => return Err(JobFailure::Start(e.to_string())),
                Some(Ok(h)) => {
                    // Unknown options are not fatal: some engines lack Threads/Hash/MultiPV.
                    let _ = h.set_threads(1);
                    let _ = h.set_hash_mb(SEARCH_HASH_MB);
                    let _ = h.set_multipv(1);
                    if let Some(name) = &h.info().name
                        && self.names.insert(name.clone())
                    {
                        self.events.push(JobEvent::Engine(name.clone()));
                    }
                    slot.engine = Engine::Ready(h);
                }
            }
        }
        let Engine::Ready(h) = &slot.engine else {
            return Ok(());
        };
        // The answer of the running search, once it is there.
        let mut answer = None;
        for ev in h.drain_events() {
            let Some(run) = slot.search.as_mut() else {
                continue;
            };
            match ev {
                EngineEvent::Info { search_id, info } if search_id == run.id => {
                    if info.multipv_index() == 1 && info.score.is_some() {
                        let exact = info.score.is_some_and(|s| s.bound == ScoreBound::Exact);
                        if exact || run.last.is_none() {
                            run.last = Some(info);
                        }
                    }
                }
                EngineEvent::BestMove {
                    search_id, best, ..
                } if search_id == run.id => {
                    let last = run.last.take();
                    answer = Some(last.map(|info| (info, best)));
                    slot.search = None;
                }
                EngineEvent::Terminated(reason) => return Err(JobFailure::Terminated(reason)),
                _ => {}
            }
        }
        if let Some(run) = slot.search.as_mut() {
            if !run.stopping && run.started.elapsed() > SEARCH_GUARD {
                let _ = h.stop();
                run.stopping = true;
            }
            return Ok(());
        }
        if let (Some(answer), Some(work)) = (answer, slot.work.as_mut()) {
            work.answer(answer);
        }
        // The next search: of this target, or of the next one.
        loop {
            if slot.work.is_none() {
                match self.targets.next().filter(|_| !self.halted) {
                    Some(t) => slot.work = Some(Work::new(t, self.nodes)),
                    None => {
                        slot.engine = Engine::Done;
                        return Ok(());
                    }
                }
            }
            let Some(work) = slot.work.as_ref() else {
                continue;
            };
            match work.next() {
                Next::Search { pos, among, nodes } => {
                    slot.next_id += 1;
                    let id = SearchId(slot.next_id);
                    // Every search starts from a clear hash, so that it depends on its
                    // position alone.
                    let _ = h.new_game();
                    h.search_among(pos, id, SearchLimits::Nodes(nodes), among)
                        .map_err(|e| JobFailure::Engine(e.to_string()))?;
                    slot.search = Some(Running {
                        id,
                        last: None,
                        started: Instant::now(),
                        stopping: false,
                    });
                    return Ok(());
                }
                Next::Done(event) => {
                    self.events.extend(event);
                    slot.work = None;
                }
            }
        }
    }
}
