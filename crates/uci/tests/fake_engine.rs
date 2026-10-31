//! Supervisor tests against the deterministic fake engine (src/bin/fake_uci.rs).
// The process transport.
#![cfg(not(target_family = "wasm"))]

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use uci::{
    Engine, EngineConfig, EngineError, EngineEvent, EngineHandle, EnginePosition, OptionKind,
    Score, ScoreBound, SearchId, SearchLimits, Side, TerminationReason, Timeouts, WhiteEval,
};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_uci");

fn timeouts() -> Timeouts {
    Timeouts {
        uciok: Duration::from_millis(1500),
        readyok: Duration::from_millis(1500),
        stop: Duration::from_millis(1500),
        quit: Duration::from_millis(500),
    }
}

fn config(args: &[&str]) -> EngineConfig {
    let mut c = EngineConfig::new(FAKE);
    c.args = args.iter().map(Into::into).collect();
    c.timeouts = timeouts();
    c
}

fn spawn(args: &[&str]) -> EngineHandle {
    Engine::spawn_with(config(args), Box::new(|| {})).expect("spawn fake engine")
}

fn fen(marker: &str) -> EnginePosition {
    EnginePosition::fen(format!("{marker} w - - 0 1"), Vec::<String>::new())
}

/// Collects events until `pred` matches one (inclusive) or the timeout hits.
fn collect_until(
    h: &EngineHandle,
    timeout: Duration,
    mut pred: impl FnMut(&EngineEvent) -> bool,
) -> (Vec<EngineEvent>, bool) {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    while let Some(ev) = h.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        let hit = pred(&ev);
        out.push(ev);
        if hit {
            return (out, true);
        }
    }
    (out, false)
}

fn proc_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn no_protocol_errors(events: &[EngineEvent]) {
    for e in events {
        if let EngineEvent::InfoString { text, .. } = e {
            assert!(!text.starts_with("ERROR"), "fake engine reported: {text}");
        }
    }
}

#[test]
fn handshake_collects_id_and_typed_options() {
    let woke = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&woke);
    let h = Engine::spawn_with(
        config(&["--banner", "--name", "Fake Engine 1.0"]),
        Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    let info = h.info();
    assert_eq!(info.name.as_deref(), Some("Fake Engine 1.0"));
    assert_eq!(info.author.as_deref(), Some("Test Suite"));
    assert_eq!(info.options.len(), 7);
    assert_eq!(
        info.option("multipv").unwrap().kind,
        OptionKind::Spin {
            default: 1,
            min: Some(1),
            max: Some(10)
        }
    );
    assert_eq!(
        info.option("Play Style").unwrap().kind,
        OptionKind::Combo {
            default: "Normal".into(),
            vars: vec!["Solid".into(), "Normal".into(), "Very Risky".into()]
        }
    );
    assert_eq!(info.option("Clear Hash").unwrap().kind, OptionKind::Button);
    assert_eq!(
        info.option("Ponder").unwrap().kind,
        OptionKind::Check { default: false }
    );
    assert!(matches!(
        info.option("Debug Log File").unwrap().kind,
        OptionKind::String { .. }
    ));
    h.analyze(fen("m1"), SearchId(1)).unwrap();
    let (_, ok) = collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    assert!(ok);
    assert!(woke.load(Ordering::SeqCst) > 0, "waker was not called");
    let pid = h.pid();
    let status = h.quit().expect("reaped");
    assert!(status.success(), "{status:?}");
    assert!(!proc_exists(pid));
}

#[test]
fn option_validation() {
    let h = spawn(&[]);
    assert!(matches!(
        h.set_option("NoSuchOption", Some("1")),
        Err(EngineError::UnknownOption(_))
    ));
    assert!(matches!(
        h.set_multipv(11),
        Err(EngineError::InvalidOptionValue { .. })
    ));
    assert!(h.set_option("Ponder", Some("yes")).is_err());
    assert!(h.set_option("Play Style", Some("Wild")).is_err());
    assert!(h.set_option("Clear Hash", Some("x")).is_err());
    assert!(h.set_option("Debug Log File", Some("a\nquit")).is_err());
    h.set_option("play style", Some("very risky")).unwrap();
    h.set_option("Clear Hash", None).unwrap();
    h.set_threads(2).unwrap();
    h.set_hash_mb(32).unwrap();
    h.set_multipv(3).unwrap();
    h.new_game().unwrap();
}

#[test]
fn spawn_errors() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("uci-spawn-errors");
    std::fs::create_dir_all(&tmp).unwrap();

    let e = Engine::spawn(tmp.join("missing"), Vec::<String>::new(), Box::new(|| {})).unwrap_err();
    assert!(matches!(e, EngineError::NotFound(_)), "{e:?}");

    let e = Engine::spawn(&tmp, Vec::<String>::new(), Box::new(|| {})).unwrap_err();
    assert!(matches!(e, EngineError::NotExecutable { .. }), "{e:?}");

    use std::os::unix::fs::PermissionsExt;
    let plain = tmp.join("plain.txt");
    std::fs::write(&plain, "not a program\n").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = Engine::spawn(&plain, Vec::<String>::new(), Box::new(|| {})).unwrap_err();
    assert!(matches!(e, EngineError::NotExecutable { .. }), "{e:?}");

    let noexec = tmp.join("garbage.bin");
    std::fs::write(&noexec, [0u8, 1, 2, 3, 4, 5, 6, 7]).unwrap();
    std::fs::set_permissions(&noexec, std::fs::Permissions::from_mode(0o755)).unwrap();
    let e = Engine::spawn(&noexec, Vec::<String>::new(), Box::new(|| {})).unwrap_err();
    assert!(matches!(e, EngineError::NotExecutable { .. }), "{e:?}");
}

#[test]
fn not_uci_program() {
    let t = Instant::now();
    let e = Engine::spawn_with(config(&["--not-uci"]), Box::new(|| {})).unwrap_err();
    match e {
        EngineError::NotUci { output } => {
            assert!(output.iter().any(|l| l.contains("not a chess engine")))
        }
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(4));
}

#[test]
fn missing_uciok_and_readyok_time_out() {
    let e = Engine::spawn_with(config(&["--no-uciok"]), Box::new(|| {})).unwrap_err();
    assert!(
        matches!(
            e,
            EngineError::HandshakeTimeout {
                waiting_for: "uciok"
            }
        ),
        "{e:?}"
    );
    let t = Instant::now();
    let e = Engine::spawn_with(config(&["--no-readyok"]), Box::new(|| {})).unwrap_err();
    assert!(
        matches!(
            e,
            EngineError::HandshakeTimeout {
                waiting_for: "readyok"
            }
        ),
        "{e:?}"
    );
    assert!(t.elapsed() < Duration::from_secs(4));
}

#[test]
fn exit_during_handshake() {
    let e = Engine::spawn_with(config(&["--exit-on-start"]), Box::new(|| {})).unwrap_err();
    match e {
        EngineError::ProcessExited { status, stderr } => {
            assert_eq!(status.and_then(|s| s.code()), Some(3));
            assert!(
                stderr.iter().any(|l| l.contains("exiting on start")),
                "{stderr:?}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn stderr_flood_does_not_block() {
    let h = spawn(&["--stderr-flood-kb", "2048"]);
    h.analyze(fen("m1"), SearchId(1)).unwrap();
    let (_, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    assert!(ok);
    let tail = h.stderr_tail();
    assert!(!tail.is_empty() && tail.len() <= 256);
}

#[test]
fn multipv_and_mate_scores() {
    let h = spawn(&["--mate"]);
    h.set_multipv(3).unwrap();
    h.analyze(EnginePosition::startpos(["e2e4"]), SearchId(7))
        .unwrap();
    let mut seen = HashMap::new();
    let (events, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        if let EngineEvent::Info { search_id, info } = e {
            assert_eq!(*search_id, SearchId(7));
            seen.insert(info.multipv_index(), info.score.unwrap());
        }
        seen.len() == 3
    });
    assert!(ok, "{events:?}");
    assert_eq!(seen[&1].score, Score::Mate(3));
    assert_eq!(seen[&2].score, Score::Mate(-2));
    assert_eq!(seen[&3].score, Score::Cp(20));
    assert_eq!(seen[&1].bound, ScoreBound::Exact);
    // After 1.e4 Black is to move: mate 3 for Black.
    assert_eq!(
        seen[&1].to_white(Side::Black).eval,
        WhiteEval::Mate {
            winner: Side::Black,
            moves: 3
        }
    );
    let pv_ok = events.iter().any(|e| match e {
        EngineEvent::Info { info, .. } => info.pv.first().map(String::as_str) == Some("e2e4"),
        _ => false,
    });
    assert!(pv_ok);
    no_protocol_errors(&events);
}

#[test]
fn garbage_lines_become_warnings() {
    let h = spawn(&["--garbage"]);
    h.analyze(fen("g1"), SearchId(1)).unwrap();
    let mut infos = 0;
    let mut warnings = 0;
    let (events, _) = collect_until(&h, Duration::from_millis(400), |e| {
        match e {
            EngineEvent::Info { .. } => infos += 1,
            EngineEvent::Warning(_) => warnings += 1,
            _ => {}
        }
        false
    });
    assert!(infos > 3, "{events:?}");
    assert!(warnings > 3);
    // Warnings are capped.
    assert!(warnings <= 100);
    assert!(h.is_running());
    no_protocol_errors(&events);
}

#[test]
fn depth_limited_search_finishes_with_bestmove() {
    let h = spawn(&[]);
    h.search(
        EnginePosition::startpos(["d2d4"]),
        SearchId(3),
        SearchLimits::Depth(5),
    )
    .unwrap();
    let (events, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        matches!(e, EngineEvent::BestMove { .. })
    });
    assert!(ok);
    match events.last().unwrap() {
        EngineEvent::BestMove {
            search_id,
            best,
            ponder,
        } => {
            assert_eq!(*search_id, SearchId(3));
            assert_eq!(best.as_deref(), Some("d2d4"));
            assert_eq!(ponder.as_deref(), Some("e7e5"));
        }
        _ => unreachable!(),
    }
    let max_depth = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Info { info, .. } => info.depth,
            _ => None,
        })
        .max();
    assert_eq!(max_depth, Some(5));
}

/// Core race: many rapid position changes against an engine that keeps
/// talking after `stop` and even after `bestmove`. Every Info must carry the
/// id of the position it was computed for.
fn rapid_changes(extra: &[&str], n: u64, pause_every: u64) {
    let mut args = vec![
        "--stale-after-stop",
        "3",
        "--info-after-bestmove",
        "3",
        "--info-interval",
        "1",
    ];
    args.extend_from_slice(extra);
    let h = spawn(&args);
    h.set_multipv(2).unwrap();
    for i in 0..n {
        let t = Instant::now();
        h.analyze(fen(&format!("m{i}")), SearchId(i)).unwrap();
        assert!(t.elapsed() < Duration::from_millis(50), "analyze blocked");
        if pause_every > 0 && i % pause_every == 0 {
            std::thread::sleep(Duration::from_millis(i % 4));
        }
    }
    let last = SearchId(n - 1);
    let (mut events, ok) = collect_until(
        &h,
        Duration::from_secs(5),
        |e| matches!(e, EngineEvent::Info { search_id, .. } if *search_id == last),
    );
    assert!(ok, "no info for the last request");
    h.stop().unwrap();
    let (more, ok) = collect_until(
        &h,
        Duration::from_secs(3),
        |e| matches!(e, EngineEvent::BestMove { search_id, .. } if *search_id == last),
    );
    assert!(ok);
    events.extend(more);
    // Let trailing stale output arrive; it must be dropped.
    events.extend(collect_until(&h, Duration::from_millis(100), |_| false).0);

    let mut started = Vec::new();
    let mut bestmoves = HashSet::new();
    let mut superseded = HashSet::new();
    let mut infos = 0;
    let mut current: Option<SearchId> = None;
    for e in &events {
        match e {
            EngineEvent::SearchStarted { search_id } => {
                assert!(current.is_none(), "start before bestmove of {current:?}");
                current = Some(*search_id);
                started.push(*search_id);
            }
            EngineEvent::Info { search_id, info } => {
                infos += 1;
                assert_eq!(Some(*search_id), current, "info outside its search");
                assert_eq!(
                    info.pv[0],
                    format!("m{}", search_id.0),
                    "info of another position under {search_id:?}"
                );
            }
            EngineEvent::BestMove { search_id, .. } => {
                assert_eq!(Some(*search_id), current);
                assert!(bestmoves.insert(*search_id));
                current = None;
            }
            EngineEvent::Superseded { search_id } => {
                assert!(superseded.insert(*search_id));
            }
            EngineEvent::Terminated(r) => panic!("terminated: {r:?}"),
            EngineEvent::Warning(w) => panic!("warning: {w}"),
            EngineEvent::InfoString { .. } => {}
        }
    }
    no_protocol_errors(&events);
    assert!(infos > 0);
    // Ids are started in increasing order, every started id got its bestmove,
    // every id is either started or superseded, never both.
    assert!(started.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(started.iter().copied().collect::<HashSet<_>>(), bestmoves);
    for i in 0..n {
        let id = SearchId(i);
        assert!(
            bestmoves.contains(&id) ^ superseded.contains(&id),
            "{id:?} started={} superseded={}",
            bestmoves.contains(&id),
            superseded.contains(&id)
        );
    }
    assert!(
        started.len() < n as usize,
        "requests were not coalesced ({} started)",
        started.len()
    );
}

#[test]
fn rapid_position_changes_never_mix_search_ids() {
    rapid_changes(&[], 200, 0);
}

#[test]
fn rapid_position_changes_with_pauses_and_slow_bestmove() {
    rapid_changes(&["--bestmove-delay", "3"], 60, 5);
}

#[test]
fn slow_bestmove_within_timeout() {
    let h = spawn(&["--bestmove-delay", "300", "--stale-after-stop", "2"]);
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    let t = Instant::now();
    h.analyze(fen("b"), SearchId(2)).unwrap();
    assert!(t.elapsed() < Duration::from_millis(50));
    let (events, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        matches!(
            e,
            EngineEvent::Info {
                search_id: SearchId(2),
                ..
            }
        )
    });
    assert!(ok);
    let bm_pos = events
        .iter()
        .position(|e| {
            matches!(
                e,
                EngineEvent::BestMove {
                    search_id: SearchId(1),
                    ..
                }
            )
        })
        .expect("bestmove of search 1");
    for (i, e) in events.iter().enumerate() {
        if let EngineEvent::Info { search_id, info } = e {
            let expect = if i < bm_pos { "a" } else { "b" };
            assert_eq!(info.pv[0], expect);
            assert_eq!(
                *search_id,
                if i < bm_pos { SearchId(1) } else { SearchId(2) }
            );
        }
    }
}

#[test]
fn options_are_deferred_until_search_ends() {
    let h = spawn(&[]);
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    h.set_multipv(3).unwrap();
    h.new_game().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    h.analyze(fen("b"), SearchId(2)).unwrap();
    let mut max_pv = 0;
    let (events, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        if let EngineEvent::Info {
            search_id: SearchId(2),
            info,
        } = e
        {
            max_pv = max_pv.max(info.multipv_index());
        }
        max_pv == 3
    });
    assert!(ok, "{events:?}");
    no_protocol_errors(&events);
}

#[test]
fn hung_engine_is_killed_and_does_not_block_caller() {
    let mut cfg = config(&["--never-bestmove"]);
    cfg.timeouts.stop = Duration::from_millis(300);
    let h = Engine::spawn_with(cfg, Box::new(|| {})).unwrap();
    let pid = h.pid();
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    let t = Instant::now();
    for i in 2..50 {
        h.analyze(fen("b"), SearchId(i)).unwrap();
    }
    assert!(t.elapsed() < Duration::from_millis(50), "caller blocked");
    let (events, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        matches!(e, EngineEvent::Terminated(_))
    });
    assert!(ok, "{events:?}");
    assert_eq!(
        events.last().unwrap(),
        &EngineEvent::Terminated(TerminationReason::Hung {
            waiting_for: "bestmove"
        })
    );
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, EngineEvent::SearchStarted { .. }))
    );
    assert!(!h.is_running());
    assert!(matches!(
        h.analyze(fen("c"), SearchId(99)),
        Err(EngineError::NotRunning)
    ));
    assert!(!proc_exists(pid), "hung process not reaped");
    let t = Instant::now();
    drop(h);
    assert!(t.elapsed() < Duration::from_millis(500));
}

#[test]
fn crash_during_search_reports_terminated() {
    let h = spawn(&["--exit-after-go", "2"]);
    let pid = h.pid();
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    h.analyze(fen("b"), SearchId(2)).unwrap();
    let (events, ok) = collect_until(&h, Duration::from_secs(3), |e| {
        matches!(e, EngineEvent::Terminated(_))
    });
    assert!(ok, "{events:?}");
    match events.last().unwrap() {
        EngineEvent::Terminated(TerminationReason::Exited { status }) => {
            assert_eq!(status.and_then(|s| s.code()), Some(1));
        }
        other => panic!("{other:?}"),
    }
    assert!(!proc_exists(pid));
    assert!(matches!(h.stop(), Err(EngineError::NotRunning)));
}

#[test]
fn quit_kills_engine_that_ignores_quit() {
    let h = spawn(&["--ignore-quit"]);
    let pid = h.pid();
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    let t = Instant::now();
    let status = h.quit().expect("reaped");
    assert!(t.elapsed() < Duration::from_secs(2));
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9), "{status:?}");
    assert!(!proc_exists(pid));
}

#[test]
fn drop_during_search_terminates_process() {
    let h = spawn(&[]);
    let pid = h.pid();
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    assert!(proc_exists(pid));
    drop(h);
    assert!(!proc_exists(pid), "process left behind after drop");
}

#[test]
fn stop_cancels_pending_and_idle_stop_is_harmless() {
    let h = spawn(&["--bestmove-delay", "100"]);
    h.stop().unwrap(); // idle: nothing happens
    h.analyze(fen("a"), SearchId(1)).unwrap();
    collect_until(&h, Duration::from_secs(2), |e| {
        matches!(e, EngineEvent::Info { .. })
    });
    h.analyze(fen("b"), SearchId(2)).unwrap(); // pending behind stop of #1
    h.stop().unwrap(); // cancels #2
    let (events, _) = collect_until(&h, Duration::from_millis(500), |_| false);
    assert!(events.contains(&EngineEvent::Superseded {
        search_id: SearchId(2)
    }));
    assert!(events.iter().any(|e| matches!(
        e,
        EngineEvent::BestMove {
            search_id: SearchId(1),
            ..
        }
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        EngineEvent::SearchStarted {
            search_id: SearchId(2)
        }
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        EngineEvent::Info {
            search_id: SearchId(2),
            ..
        }
    )));
}

#[test]
fn invalid_positions_are_rejected_before_sending() {
    let h = spawn(&[]);
    assert!(matches!(
        h.analyze(EnginePosition::startpos(["e2e4", "--"]), SearchId(1)),
        Err(EngineError::InvalidInput(_))
    ));
    assert!(matches!(
        h.analyze(EnginePosition::startpos(["0000"]), SearchId(1)),
        Err(EngineError::InvalidInput(_))
    ));
    assert!(h.recv_timeout(Duration::from_millis(100)).is_none());
}

/// The nice value of a process (field 19 of /proc/<pid>/stat, after the name).
#[cfg(target_os = "linux")]
fn nice_of(pid: u32) -> i32 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("stat");
    let after_name = &stat[stat.rfind(')').expect("name") + 2..];
    after_name
        .split(' ')
        .nth(16)
        .expect("nice")
        .parse()
        .expect("number")
}

#[cfg(target_os = "linux")]
#[test]
fn an_engine_can_run_below_the_app() {
    let ours = nice_of(std::process::id());
    let calm = Engine::spawn_with(
        EngineConfig {
            nice: 10,
            ..config(&[])
        },
        Box::new(|| {}),
    )
    .expect("spawn");
    let plain = spawn(&[]);
    assert_eq!(nice_of(calm.pid()), (ours + 10).min(19));
    assert_eq!(nice_of(plain.pid()), ours);
    calm.quit();
    plain.quit();
}

#[cfg(target_os = "linux")]
#[test]
fn an_engine_gets_the_variables_given() {
    let engine = Engine::spawn_with(
        EngineConfig {
            env: vec![("CP_TEST_HOME".into(), "/some/where".into())],
            ..config(&[])
        },
        Box::new(|| {}),
    )
    .expect("spawn");
    let environ = std::fs::read(format!("/proc/{}/environ", engine.pid())).expect("environ");
    assert!(
        environ
            .split(|&b| b == 0)
            .any(|v| v == b"CP_TEST_HOME=/some/where")
    );
    // The app's own environment goes on as well.
    assert!(environ.split(|&b| b == 0).any(|v| v.starts_with(b"PATH=")));
    engine.quit();
}

/// The state letter of process `pid` (`/proc/<pid>/stat`), `None` when it is gone.
#[cfg(target_os = "linux")]
fn proc_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?.1.chars().next()
}

/// `kill_with_parent`: the engine dies with the thread that started it (the program
/// quitting while a handshake hangs); without it, it lives on.
#[cfg(target_os = "linux")]
#[test]
fn an_engine_can_die_with_its_starting_thread() {
    let start = |kill: bool| {
        std::thread::spawn(move || {
            let mut c = config(&[]);
            c.kill_with_parent = kill;
            let handle = Engine::spawn_with(c, Box::new(|| {})).expect("spawn fake engine");
            let pid = handle.pid();
            // Never quit: as if the program ended under it.
            std::mem::forget(handle);
            pid
        })
        .join()
        .expect("the thread")
    };
    let gone = |pid: u32| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match proc_state(pid) {
                None | Some('Z') | Some('X') => return true,
                _ if Instant::now() > deadline => return false,
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    };
    let killed = start(true);
    assert!(gone(killed), "the engine outlived its thread");
    let kept = start(false);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        matches!(proc_state(kept), Some('S' | 'R')),
        "{:?}",
        proc_state(kept)
    );
    unsafe {
        libc::kill(kept as i32, libc::SIGKILL);
    }
}
