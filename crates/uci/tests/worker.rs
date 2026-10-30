//! The worker transport against a fake engine written in JavaScript, in a browser
//! (`docs/dev/web.md`, «Проверки»: wasm-bindgen-test-runner with headless Firefox).
//!
//! The fake engine answers like `src/bin/fake_uci.rs` in the points that matter
//! here: the first PV move of a search is the marker of its position (the first
//! FEN field), info lines come every few milliseconds until `stop`, and flags make
//! it hang, crash or stay silent.
#![cfg(target_family = "wasm")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use uci::{
    Engine, EngineError, EngineEvent, EngineHandle, EnginePosition, SearchId, SearchLimits,
    Starting, TerminationReason, Timeouts, WorkerConfig,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const FAKE: &str = r#"
let searching = null, timer = null, marker = 'startpos', n = 0;
const send = (l) => postMessage(l);
function stopSearch() {
  if (searching === null) return;
  clearInterval(timer); timer = null;
  const m = searching; searching = null;
  if (FLAGS.hangOnStop) return;
  send('info depth 9 score cp 1 pv ' + m);
  send('bestmove ' + m);
}
onmessage = (e) => {
  const line = String(e.data), w = line.split(' ');
  switch (w[0]) {
    case 'uci':
      if (FLAGS.silent) return;
      send('id name Fake Worker'); send('id author Test Suite');
      send('option name MultiPV type spin default 1 min 1 max 10');
      send('option name Hash type spin default 16 min 1 max 1024');
      if (!FLAGS.noUciok) send('uciok');
      break;
    case 'isready': if (!FLAGS.noReadyok) send('readyok'); break;
    case 'position':
      if (w[1] === 'fen') marker = w[2];
      else { const i = w.indexOf('moves'); marker = i > 0 && w.length > i + 1 ? w[w.length - 1] : 'startpos'; }
      break;
    case 'go':
      if (FLAGS.crashOnGo) throw new Error('fake engine crashed');
      searching = marker; n = 0;
      if (w[1] === 'nodes') { send('info depth 1 score cp 3 nodes ' + w[2] + ' pv ' + marker); stopSearch(); break; }
      timer = setInterval(() => { n++; send('info depth ' + n + ' score cp ' + n + ' pv ' + searching); }, 5);
      break;
    case 'stop': stopSearch(); break;
    case 'quit': close(); break;
  }
};
"#;

/// A worker script of the fake engine with these flags (a `blob:` URL).
fn fake(flags: &str) -> String {
    let source = format!("const FLAGS = {{{flags}}};\n{FAKE}");
    let parts = js_sys::Array::of1(&JsValue::from_str(&source));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("text/javascript");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts).unwrap();
    web_sys::Url::create_object_url_with_blob(&blob).unwrap()
}

fn timeouts() -> Timeouts {
    Timeouts {
        uciok: Duration::from_millis(1500),
        readyok: Duration::from_millis(1500),
        stop: Duration::from_millis(500),
        quit: Duration::from_millis(500),
    }
}

async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .unwrap();
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
}

/// Starts the fake engine and waits for its handshake; also counts the waker's calls.
async fn start(flags: &str) -> (Result<EngineHandle, EngineError>, u32) {
    let woke = Arc::new(AtomicU32::new(0));
    let w = Arc::clone(&woke);
    let mut config = WorkerConfig::new(fake(flags));
    config.timeouts = timeouts();
    let mut starting: Starting = Engine::start_worker(
        config,
        Box::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .expect("worker created");
    for _ in 0..400 {
        if let Some(r) = starting.poll() {
            return (r, woke.load(Ordering::Relaxed));
        }
        sleep(10).await;
    }
    panic!("the handshake neither ended nor timed out");
}

/// Events until `pred` holds for one (inclusive), or `ms` pass.
async fn until(h: &EngineHandle, ms: u32, pred: impl Fn(&EngineEvent) -> bool) -> Vec<EngineEvent> {
    let mut out = Vec::new();
    for _ in 0..ms / 5 {
        while let Some(ev) = h.try_recv() {
            let hit = pred(&ev);
            out.push(ev);
            if hit {
                return out;
            }
        }
        sleep(5).await;
    }
    panic!("no expected event in {ms} ms: {out:?}");
}

fn fen(marker: &str) -> EnginePosition {
    EnginePosition::fen(format!("{marker} w - - 0 1"), Vec::<String>::new())
}

#[wasm_bindgen_test]
async fn handshake_then_analysis_and_stop() {
    let (h, woke) = start("").await;
    let h = h.expect("started");
    assert_eq!(h.info().name.as_deref(), Some("Fake Worker"));
    assert_eq!(h.info().options.len(), 2);
    assert!(woke > 0, "the waker was not called");
    h.set_multipv(2).unwrap();
    assert!(matches!(
        h.set_multipv(11),
        Err(EngineError::InvalidOptionValue { .. })
    ));
    h.analyze(fen("m1"), SearchId(1)).unwrap();
    let evs = until(&h, 2000, |e| matches!(e, EngineEvent::Info { .. })).await;
    assert!(evs.contains(&EngineEvent::SearchStarted {
        search_id: SearchId(1)
    }));
    h.stop().unwrap();
    let evs = until(&h, 2000, |e| matches!(e, EngineEvent::BestMove { .. })).await;
    match evs.last() {
        Some(EngineEvent::BestMove {
            search_id, best, ..
        }) => {
            assert_eq!(*search_id, SearchId(1));
            assert_eq!(best.as_deref(), Some("m1"));
        }
        other => panic!("{other:?}"),
    }
    assert!(h.is_running());
}

#[wasm_bindgen_test]
async fn rapid_requests_start_only_the_latest_and_never_mix_output() {
    let (h, _) = start("").await;
    let h = h.expect("started");
    h.analyze(fen("a"), SearchId(1)).unwrap();
    until(&h, 2000, |e| matches!(e, EngineEvent::Info { .. })).await;
    for (id, m) in [(2, "b"), (3, "c"), (4, "d")] {
        h.analyze(fen(m), SearchId(id)).unwrap();
    }
    let evs = until(&h, 3000, |e| {
        matches!(
            e,
            EngineEvent::Info {
                search_id: SearchId(4),
                ..
            }
        )
    })
    .await;
    for id in [2, 3] {
        assert!(evs.contains(&EngineEvent::Superseded {
            search_id: SearchId(id)
        }));
        assert!(!evs.contains(&EngineEvent::SearchStarted {
            search_id: SearchId(id)
        }));
    }
    for e in &evs {
        if let EngineEvent::Info { search_id, info } = e {
            let want = if *search_id == SearchId(1) { "a" } else { "d" };
            assert_eq!(info.pv.first().map(String::as_str), Some(want), "{e:?}");
        }
    }
    // A node-limited search ends by itself.
    h.search(fen("n"), SearchId(5), SearchLimits::Nodes(1000))
        .unwrap();
    let evs = until(&h, 2000, |e| {
        matches!(
            e,
            EngineEvent::BestMove {
                search_id: SearchId(5),
                ..
            }
        )
    })
    .await;
    assert!(matches!(evs.last(), Some(EngineEvent::BestMove { best: Some(b), .. }) if b == "n"));
}

#[wasm_bindgen_test]
async fn an_engine_hung_on_stop_is_terminated() {
    let (h, _) = start("hangOnStop: true").await;
    let h = h.expect("started");
    h.analyze(fen("h"), SearchId(1)).unwrap();
    until(&h, 2000, |e| matches!(e, EngineEvent::Info { .. })).await;
    h.stop().unwrap();
    let evs = until(&h, 3000, |e| matches!(e, EngineEvent::Terminated(_))).await;
    assert_eq!(
        evs.last(),
        Some(&EngineEvent::Terminated(TerminationReason::Hung {
            waiting_for: "bestmove"
        }))
    );
    assert!(!h.is_running());
    assert!(matches!(h.stop(), Err(EngineError::NotRunning)));
}

#[wasm_bindgen_test]
async fn a_crash_ends_the_session() {
    let (h, _) = start("crashOnGo: true").await;
    let h = h.expect("started");
    h.analyze(fen("x"), SearchId(1)).unwrap();
    let evs = until(&h, 3000, |e| matches!(e, EngineEvent::Terminated(_))).await;
    assert_eq!(
        evs.last(),
        Some(&EngineEvent::Terminated(TerminationReason::Exited {
            status: None
        }))
    );
}

#[wasm_bindgen_test]
async fn handshake_failures_are_named() {
    let (silent, _) = start("silent: true").await;
    assert!(
        matches!(silent, Err(EngineError::NotUci { .. })),
        "{silent:?}"
    );
    let (no_uciok, _) = start("noUciok: true").await;
    assert!(
        matches!(
            no_uciok,
            Err(EngineError::HandshakeTimeout {
                waiting_for: "uciok"
            })
        ),
        "{no_uciok:?}"
    );
    let (no_readyok, _) = start("noReadyok: true").await;
    assert!(
        matches!(
            no_readyok,
            Err(EngineError::HandshakeTimeout {
                waiting_for: "readyok"
            })
        ),
        "{no_readyok:?}"
    );
    // A script that does not load.
    let mut config = WorkerConfig::new("/no-such-engine.js");
    config.timeouts = timeouts();
    let mut starting = Engine::start_worker(config, Box::new(|| {})).expect("worker created");
    let mut result = None;
    for _ in 0..400 {
        result = starting.poll();
        if result.is_some() {
            break;
        }
        sleep(10).await;
    }
    assert!(
        matches!(result, Some(Err(EngineError::ProcessExited { .. }))),
        "{result:?}"
    );
}
