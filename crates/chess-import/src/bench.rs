//! `chesspenguin --bench <database> [report.txt]`: timings of opening, filtering and
//! position search in one database, to compare machines (laptops in particular) with
//! the numbers in `docs/dev/import.md`. Every search runs twice, so the first run
//! shows the cost of a cold file cache and the second the warm one. Nothing of the
//! database is written; a large ChessBase base without a usable `.cbb` gets our own
//! booster built in `booster_dir` (timed), as the application would do.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use shakmaty::san::SanPlus;
use shakmaty::{Chess, Move, Position};

use crate::chessbase::base::{Filter, NameFilter, OpeningsStatus, SortKey};
use crate::position::PositionQuery;
use crate::{Database, ScanProgress};

/// (name, moves from the start position).
const LINES: [(&str, &str); 5] = [
    ("start", ""),
    ("1.e4", "e4"),
    ("Najdorf, ply 10", "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6"),
    (
        "Nimzo-Indian, ply 20",
        "d4 Nf6 c4 e6 Nc3 Bb4 e3 O-O Bd3 d5 Nf3 c5 O-O Nc6 a3 Bxc3 bxc3 dxc4 Bxc4 Qc7",
    ),
    (
        "Ruy Lopez, ply 40",
        "e4 e5 Nf3 Nc6 Bb5 a6 Ba4 Nf6 O-O Be7 Re1 b5 Bb3 d6 c3 O-O h3 Nb8 d4 Nbd7 \
         Nbd2 Bb7 Bc2 Re8 Nf1 Bf8 Ng3 g6 a4 c5 d5 c4 Bg5 h6 Be3 Nc5 Qd2 h5",
    ),
];

fn san_line(line: &str) -> Option<(Chess, Vec<Move>)> {
    let mut pos = Chess::default();
    let mut moves = Vec::new();
    for s in line.split_whitespace() {
        let m = s.parse::<SanPlus>().ok()?.san.to_move(&pos).ok()?;
        moves.push(m);
        pos.play_unchecked(m);
    }
    Some((pos, moves))
}

/// Peak resident memory of this process, where the system tells (Linux).
fn peak_memory() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM"))
                .map(|l| l.trim_start_matches("VmHWM:").trim().to_owned())
        })
        .unwrap_or_else(|| "n/a".into())
}

fn mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1e6)
}

/// Runs the measurements on the database at `path` and writes the report to `out`.
pub fn run(path: &Path, booster_dir: Option<&Path>, out: &mut dyn Write) -> io::Result<()> {
    let never = AtomicBool::new(false);
    let counter = AtomicUsize::new(0);
    writeln!(
        out,
        "ChessPenguin {} bench · {} {} · {} threads · memory total {}, available {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        crate::memory::total().map_or("n/a".into(), mb),
        crate::memory::available().map_or("n/a".into(), mb),
    )?;
    writeln!(out, "{}", path.display())?;

    let t = Instant::now();
    let opened = crate::open(path, &mut |_: ScanProgress| {}, &never)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let db = opened.db;
    writeln!(
        out,
        "open: {:.2} s · {:?} · {} records · peak memory {}",
        t.elapsed().as_secs_f64(),
        opened.format,
        db.len(),
        peak_memory()
    )?;

    match &db {
        Database::ChessBase(base) => {
            if let Some(dir) = booster_dir {
                base.set_booster_dir(dir);
            }
            let mut prepared = String::new();
            if base.prepare_wanted() {
                let t = Instant::now();
                prepared = match base.prepare(&counter, &never) {
                    Ok(true) => format!(" (prepared in {:.1} s)", t.elapsed().as_secs_f64()),
                    Ok(false) => " (cancelled)".into(),
                    Err(e) => format!(" (preparing failed: {e})"),
                };
            }
            let openings = match base.openings_status() {
                OpeningsStatus::Loaded(bytes) => format!("loaded, {}", mb(bytes as u64)),
                OpeningsStatus::OnDisk { bytes, room } => format!(
                    "on disk, {}, not loaded ({})",
                    mb(bytes),
                    if room {
                        "room for it"
                    } else {
                        "not enough memory"
                    }
                ),
                OpeningsStatus::Missing { room: true } => "not built".into(),
                OpeningsStatus::Missing { room: false } => "not built: not enough memory".into(),
                OpeningsStatus::NoCache => "no cache directory".into(),
            };
            writeln!(
                out,
                "search booster (.cbb or our own): {} · opening index: {openings}{prepared}",
                if base.booster_ready() { "yes" } else { "no" }
            )?;

            let carlsen = Filter {
                player: NameFilter {
                    text: "carlsen, magnus".into(),
                    ..NameFilter::default()
                },
                ..Filter::default()
            };
            let t = Instant::now();
            let selected = base
                .query(&carlsen, SortKey::Date, false, &counter, &never)
                .unwrap_or_default();
            writeln!(
                out,
                "filter player \"carlsen, magnus\", newest first: {} games in {:.1} ms",
                selected.len(),
                t.elapsed().as_secs_f64() * 1e3
            )?;
            let quick = Filter {
                text: "carlsen 2020".into(),
                ..Filter::default()
            };
            let t = Instant::now();
            let found = base
                .query(&quick, SortKey::Number, true, &counter, &never)
                .map_or(0, |v| v.len());
            writeln!(
                out,
                "quick search \"carlsen 2020\": {found} games in {:.1} ms",
                t.elapsed().as_secs_f64() * 1e3
            )?;
            let t = Instant::now();
            let all = base
                .query(&Filter::default(), SortKey::White, true, &counter, &never)
                .unwrap_or_default();
            writeln!(
                out,
                "all {} records sorted by White: {:.1} ms",
                all.len(),
                t.elapsed().as_secs_f64() * 1e3
            )?;
            drop(all);
            let mut candidates: Vec<u32> = selected;
            candidates.sort_unstable();

            writeln!(
                out,
                "position search (all games: first run, second run; then among the filtered games):"
            )?;
            for (name, line) in LINES {
                let Some((pos, moves)) = san_line(line) else {
                    continue;
                };
                let query = PositionQuery::new(pos, Some(moves));
                let mut cells = Vec::new();
                let mut games = 0;
                for cands in [None, None, Some(candidates.as_slice())] {
                    let t = Instant::now();
                    let found = db.search_position(cands, &query, &counter, &never);
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    let n = found.as_ref().map_or(0, |o| o.matches.len());
                    if cands.is_none() {
                        games = n;
                    }
                    cells.push(format!("{ms:8.1} ms"));
                }
                writeln!(
                    out,
                    "  {name:22} {games:9} games   {}   {}   filtered {}",
                    cells[0], cells[1], cells[2]
                )?;
            }
        }
        Database::Pgn(pgn) => {
            writeln!(
                out,
                "games: {} · position table estimated at {}{}",
                pgn.db.len(),
                mb(pgn.positions_estimate()),
                match pgn.positions_blocked() {
                    Some(_) => " · not built: not enough memory",
                    None => "",
                }
            )?;
            let t = Instant::now();
            let Some(table) = pgn.positions(&counter, &never) else {
                return Ok(());
            };
            writeln!(
                out,
                "position table: {:.2} s · {} positions · {} · peak memory {}",
                t.elapsed().as_secs_f64(),
                table.positions(),
                mb(table.memory() as u64),
                peak_memory()
            )?;
            writeln!(out, "position search (first run, second run):")?;
            for (name, line) in LINES {
                let Some((pos, moves)) = san_line(line) else {
                    continue;
                };
                let query = PositionQuery::new(pos, Some(moves));
                let mut cells = Vec::new();
                let mut games = 0;
                for _ in 0..2 {
                    let t = Instant::now();
                    let found = db.search_position(None, &query, &counter, &never);
                    cells.push(format!("{:8.1} ms", t.elapsed().as_secs_f64() * 1e3));
                    games = found.map_or(0, |o| o.matches.len());
                }
                writeln!(
                    out,
                    "  {name:22} {games:9} games   {}   {}",
                    cells[0], cells[1]
                )?;
            }
        }
    }
    writeln!(out, "peak memory: {}", peak_memory())?;
    Ok(())
}
