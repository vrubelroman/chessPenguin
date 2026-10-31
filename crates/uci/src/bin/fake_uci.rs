//! Deterministic fake UCI engine used by the `uci` crate tests.
//! Not a real engine: it echoes a marker of the analysed position as the
//! first PV move, so tests can check that output is attributed to the right
//! search.
//!
//! Marker: first FEN field for `position fen`, otherwise the last move, or
//! `startpos`.
//!
//! Flags (all optional):
//!   --name NAME            id name (default "Fake UCI")
//!   --banner               print a banner line before anything
//!   --exit-on-start        print to stderr and exit(3) immediately
//!   --stderr-flood-kb N    write N KiB to stderr at start
//!   --not-uci              answer `uci` with non-UCI chatter only
//!   --uciok-delay MS       sleep before answering `uci`
//!   --no-uciok             never send uciok
//!   --no-readyok           never send readyok
//!   --garbage              emit malformed lines during handshake and search
//!   --info-interval MS     period of info output while searching (default 5)
//!   --stale-after-stop N   after `stop`, N more info lines of the old search
//!   --info-after-bestmove N  N info lines of the old search after bestmove
//!   --bestmove-delay MS    delay between `stop` and `bestmove`
//!   --never-bestmove       ignore `stop` and go silent (hung engine)
//!   --exit-after-go N      on the N-th `go`, print one info and exit(1)
//!   --mate                 multipv 1 = mate 3, multipv 2 = mate -2
//!   --ignore-quit          ignore `quit` (must be killed)
//!   --hash-memory          scores depend on the position and on the searches made
//!                          since the last `ucinewgame` (as a hash kept between them)
//!   --searchmoves          honour `go ... searchmoves M...`: M's first move is the
//!                          best move, scored 30 cp lower (a worse move)
//!   --shallow N MOVE       `go nodes n` with n < N (no `searchmoves`): MOVE is the
//!                          best move (a shallow search choosing another move)
//!   --answer MARKER TAIL   `go` in the position of MARKER answers at once with
//!                          `info depth 10 multipv 1 score TAIL` and the bestmove of
//!                          the first move after `pv` in TAIL (repeatable)
//!
//! Protocol violations by the GUI (`position`/`go`/`setoption`/`ucinewgame`
//! during a search) are reported as `info string ERROR ...`.

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

#[derive(Default)]
struct Cfg {
    name: Option<String>,
    banner: bool,
    exit_on_start: bool,
    stderr_flood_kb: usize,
    not_uci: bool,
    uciok_delay: u64,
    no_uciok: bool,
    no_readyok: bool,
    garbage: bool,
    info_interval: u64,
    stale_after_stop: u32,
    info_after_bestmove: u32,
    bestmove_delay: u64,
    never_bestmove: bool,
    exit_after_go: Option<u32>,
    mate: bool,
    ignore_quit: bool,
    hash_memory: bool,
    searchmoves: bool,
    shallow: Option<(u64, String)>,
    answers: Vec<(String, String)>,
}

fn parse_args() -> Cfg {
    let mut c = Cfg {
        info_interval: 5,
        ..Cfg::default()
    };
    let mut args = std::env::args().skip(1);
    let num = |v: Option<String>| -> u64 { v.and_then(|s| s.parse().ok()).unwrap_or(0) };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--name" => c.name = args.next(),
            "--banner" => c.banner = true,
            "--exit-on-start" => c.exit_on_start = true,
            "--stderr-flood-kb" => c.stderr_flood_kb = num(args.next()) as usize,
            "--not-uci" => c.not_uci = true,
            "--uciok-delay" => c.uciok_delay = num(args.next()),
            "--no-uciok" => c.no_uciok = true,
            "--no-readyok" => c.no_readyok = true,
            "--garbage" => c.garbage = true,
            "--info-interval" => c.info_interval = num(args.next()),
            "--stale-after-stop" => c.stale_after_stop = num(args.next()) as u32,
            "--info-after-bestmove" => c.info_after_bestmove = num(args.next()) as u32,
            "--bestmove-delay" => c.bestmove_delay = num(args.next()),
            "--never-bestmove" => c.never_bestmove = true,
            "--exit-after-go" => c.exit_after_go = Some(num(args.next()) as u32),
            "--mate" => c.mate = true,
            "--ignore-quit" => c.ignore_quit = true,
            "--hash-memory" => c.hash_memory = true,
            "--searchmoves" => c.searchmoves = true,
            "--shallow" => {
                let n = num(args.next());
                c.shallow = args.next().map(|m| (n, m));
            }
            "--answer" => {
                if let (Some(m), Some(t)) = (args.next(), args.next()) {
                    c.answers.push((m, t));
                }
            }
            other => {
                eprintln!("fake_uci: unknown flag {other}");
                std::process::exit(2);
            }
        }
    }
    c
}

struct Search {
    marker: String,
    depth: u32,
    max_depth: Option<u32>,
    /// Added to every score (`--hash-memory`).
    bias: i32,
}

fn out(line: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}

fn info_lines(cfg: &Cfg, marker: &str, depth: u32, multipv: u32, bias: i32) {
    for k in 1..=multipv {
        let score = if cfg.mate && k == 1 {
            "mate 3".to_string()
        } else if cfg.mate && k == 2 {
            "mate -2".to_string()
        } else {
            format!("cp {}", 50 - 10 * k as i32 + bias)
        };
        out(&format!(
            "info depth {depth} seldepth {} multipv {k} score {score} nodes {} nps 100000 \
             hashfull 3 time {depth} pv {marker} e7e5 g1f3",
            depth + 2,
            depth as u64 * 1000
        ));
    }
    if cfg.garbage {
        out("info depth notanumber pv x");
        out("#### garbage ####");
        out("info score banana 7");
    }
}

fn main() {
    let cfg = parse_args();
    if cfg.exit_on_start {
        eprintln!("fake_uci: exiting on start");
        std::process::exit(3);
    }
    if cfg.stderr_flood_kb > 0 {
        let line = "x".repeat(1023);
        let mut e = std::io::stderr().lock();
        for i in 0..cfg.stderr_flood_kb {
            let _ = writeln!(e, "{i} {line}");
        }
    }
    if cfg.banner {
        out("Fake UCI engine by nobody");
    }

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut multipv = 1u32;
    let mut marker = "startpos".to_string();
    let mut search: Option<Search> = None;
    let mut gos = 0u32;
    // Searches since the last `ucinewgame` (`--hash-memory`).
    let mut since_new_game = 0i32;
    let mut hung = false;
    let interval = Duration::from_millis(cfg.info_interval.max(1));

    loop {
        let line = if search.is_some() {
            match rx.recv_timeout(interval) {
                Ok(l) => Some(l),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(l) => Some(l),
                Err(_) => return,
            }
        };
        let Some(line) = line else {
            // Search tick.
            if let Some(s) = search.as_mut() {
                s.depth += 1;
                info_lines(&cfg, &s.marker, s.depth, multipv, s.bias);
                if s.max_depth.is_some_and(|m| s.depth >= m) {
                    out(&format!("bestmove {} ponder e7e5", bm(&s.marker)));
                    search = None;
                }
            }
            continue;
        };
        if hung {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks.first().copied() {
            Some("uci") => {
                if cfg.not_uci {
                    out("Hello! I am definitely not a chess engine.");
                    continue;
                }
                std::thread::sleep(Duration::from_millis(cfg.uciok_delay));
                out(&format!(
                    "id name {}",
                    cfg.name.as_deref().unwrap_or("Fake UCI")
                ));
                out("id author Test Suite");
                out("option name Threads type spin default 1 min 1 max 64");
                out("option name Hash type spin default 16 min 1 max 1024");
                out("option name MultiPV type spin default 1 min 1 max 10");
                out("option name Ponder type check default false");
                out(
                    "option name Play Style type combo default Normal var Solid var Normal var Very Risky",
                );
                out("option name Clear Hash type button");
                out("option name Debug Log File type string default <empty>");
                if cfg.garbage {
                    out("option name Broken type spin default lots");
                    out("~~~ random noise ~~~");
                }
                if !cfg.no_uciok {
                    out("uciok");
                }
            }
            Some("isready") => {
                if !cfg.no_readyok {
                    out("readyok");
                }
            }
            Some("setoption") => {
                if search.is_some() {
                    out("info string ERROR setoption during search");
                }
                // setoption name <name...> value <v>
                let vpos = toks.iter().position(|&t| t == "value");
                let name = toks[2..vpos.unwrap_or(toks.len())].join(" ");
                if name.eq_ignore_ascii_case("MultiPV")
                    && let Some(v) = vpos.and_then(|p| toks.get(p + 1))
                {
                    multipv = v.parse().unwrap_or(1);
                }
            }
            Some("ucinewgame") => {
                if search.is_some() {
                    out("info string ERROR ucinewgame during search");
                }
                since_new_game = 0;
            }
            Some("position") => {
                if search.is_some() {
                    out("info string ERROR position during search");
                }
                marker = match toks.get(1) {
                    Some(&"fen") => toks.get(2).unwrap_or(&"?").to_string(),
                    _ => match toks.iter().position(|&t| t == "moves") {
                        Some(p) if p + 1 < toks.len() => toks[toks.len() - 1].to_string(),
                        _ => "startpos".to_string(),
                    },
                };
            }
            Some("go") => {
                if search.is_some() {
                    out("info string ERROR go during search");
                }
                gos += 1;
                if let Some((_, tail)) = cfg.answers.iter().find(|(m, _)| *m == marker) {
                    out(&format!("info depth 10 multipv 1 score {tail}"));
                    let best = tail
                        .split_whitespace()
                        .skip_while(|&t| t != "pv")
                        .nth(1)
                        .unwrap_or("0000");
                    out(&format!("bestmove {best}"));
                    continue;
                }
                if cfg.exit_after_go == Some(gos) {
                    info_lines(&cfg, &marker, 1, multipv, 0);
                    std::process::exit(1);
                }
                let max_depth = toks
                    .iter()
                    .position(|&t| t == "depth")
                    .and_then(|p| toks.get(p + 1))
                    .and_then(|v| v.parse().ok())
                    // `go nodes N`: a short search that ends by itself, like a real one.
                    .or_else(|| toks.contains(&"nodes").then_some(3));
                since_new_game += 1;
                let mut bias = if cfg.hash_memory {
                    let of_position = marker.bytes().map(i32::from).sum::<i32>() % 97;
                    of_position + 1000 * since_new_game
                } else {
                    0
                };
                let among = toks
                    .iter()
                    .position(|&t| t == "searchmoves")
                    .and_then(|p| toks.get(p + 1));
                let nodes: Option<u64> = toks
                    .iter()
                    .position(|&t| t == "nodes")
                    .and_then(|p| toks.get(p + 1))
                    .and_then(|v| v.parse().ok());
                let mut shown = marker.clone();
                match (among, &cfg.shallow) {
                    (Some(m), _) if cfg.searchmoves => {
                        shown = (*m).to_owned();
                        bias -= 30;
                    }
                    (None, Some((below, best))) if nodes.is_some_and(|n| n < *below) => {
                        shown = best.clone();
                    }
                    _ => {}
                }
                search = Some(Search {
                    marker: shown,
                    depth: 0,
                    max_depth,
                    bias,
                });
            }
            Some("stop") => {
                let Some(s) = search.take() else { continue };
                if cfg.never_bestmove {
                    hung = true;
                    continue;
                }
                for i in 0..cfg.stale_after_stop {
                    info_lines(&cfg, &s.marker, s.depth + 1 + i, multipv, s.bias);
                }
                std::thread::sleep(Duration::from_millis(cfg.bestmove_delay));
                out(&format!("bestmove {} ponder e7e5", bm(&s.marker)));
                for i in 0..cfg.info_after_bestmove {
                    info_lines(&cfg, &s.marker, s.depth + 100 + i, multipv, s.bias);
                }
            }
            Some("quit") if !cfg.ignore_quit => return,
            _ => {}
        }
    }
}

/// Best move token: the marker itself when it looks like a move, else e2e4.
fn bm(marker: &str) -> &str {
    if marker.len() == 4 && marker.is_ascii() {
        marker
    } else {
        "e2e4"
    }
}
