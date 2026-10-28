//! A check of the `prep` crate on a real database, printed as text: the candidates
//! for a name, the repertoire tree of each colour, the opening table, where the games
//! left the book (the database itself, or another one), and, with an engine, the
//! opening errors by opening and the repeated ones.
//!
//! ```text
//! cargo run --release -p prep --example prep_report -- <database> "<player>" \
//!     [--engine <path>] [--book <path>] [--plies 20] [--processes N] \
//!     [--min-games 3] [--no-cache]
//! ```
//!
//! The report and the evaluations are cached under `~/.cache/chesspenguin/prep/`
//! like the application would (`--no-cache` ignores and does not write it).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use prep::{
    BookOptions, ColorReport, Options, Player, PrepReport, PositionCache, Side, Tree,
};

struct Args {
    db: PathBuf,
    player: String,
    engine: Option<PathBuf>,
    book: Option<PathBuf>,
    plies: usize,
    processes: usize,
    min_games: u32,
    cache: bool,
}

fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        db: PathBuf::new(),
        player: String::new(),
        engine: None,
        book: None,
        plies: prep::DEFAULT_PLIES,
        processes: game_review::processes(game_review::MAX_PROCESSES),
        min_games: 3,
        cache: true,
    };
    let mut positional = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--engine" => a.engine = it.next().map(PathBuf::from),
            "--book" => a.book = it.next().map(PathBuf::from),
            "--plies" => a.plies = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.plies),
            "--processes" => {
                a.processes = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.processes)
            }
            "--min-games" => {
                a.min_games = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.min_games)
            }
            "--no-cache" => a.cache = false,
            _ => positional.push(arg),
        }
    }
    if positional.len() < 2 {
        eprintln!("usage: prep_report <database> <player> [--engine path] [--book path] …");
        std::process::exit(2);
    }
    a.db = PathBuf::from(&positional[0]);
    a.player = positional[1].clone();
    a
}

fn open(path: &Path) -> chess_import::Database {
    let t = Instant::now();
    let opened = chess_import::open(path, &mut |_| {}, &AtomicBool::new(false))
        .unwrap_or_else(|e| {
            eprintln!("cannot open {}: {e}", path.display());
            std::process::exit(1);
        });
    println!(
        "[open] {} ({}): {} games, {:.2} s",
        path.display(),
        opened.format.label(),
        opened.db.games(),
        t.elapsed().as_secs_f64()
    );
    opened.db
}

fn tally(t: &prep::Tally) -> String {
    format!("{} (+{} ={} -{})", t.games, t.wins, t.draws, t.losses)
}

fn print_tree(tree: &Tree, node: usize, depth: usize, min_games: u32, report: &PrepReport) {
    for &c in &tree.nodes[node].children {
        let n = &tree.nodes[c as usize];
        if n.tally.games < min_games {
            continue;
        }
        let name = match (n.named, n.opening) {
            (true, Some(o)) => format!("  — {}", report.openings[o].name),
            _ => String::new(),
        };
        println!(
            "{:indent$}{} {} {}{}{}{}",
            "",
            prep::move_label(usize::from(n.ply) - 1, &n.san),
            tally(&n.tally),
            n.last_date,
            n.average_elo().map_or(String::new(), |e| format!(" Elo {e}")),
            if n.transposition { " ↔" } else { "" },
            name,
            indent = depth * 2
        );
        if depth < 8 {
            print_tree(tree, c as usize, depth + 1, min_games, report);
        }
    }
}

fn print_color(report: &PrepReport, cr: &ColorReport, min_games: u32) {
    let colour = if cr.white { "White" } else { "Black" };
    println!("\n==== {colour}: {} games, {} tree nodes ====", cr.games.len(), cr.tree.nodes.len());
    println!("-- repertoire (nodes with at least {min_games} games; ↔ = transposition)");
    print_tree(&cr.tree, 0, 0, min_games, report);
    println!("-- openings");
    for f in &cr.families {
        println!(
            "{} {}{}",
            tally(&f.tally),
            if f.name.is_empty() { "(unnamed)" } else { &f.name },
            errors_text(&f.errors)
        );
        for v in &f.variations {
            let (name, line, eco) = match v.opening {
                Some(o) => {
                    let o = &report.openings[o];
                    (o.name.as_str(), o.line.as_str(), o.eco.as_str())
                }
                None => ("(unnamed)", "", ""),
            };
            let games: Vec<String> = v
                .games
                .iter()
                .map(|&g| format!("#{}", report.games[g as usize].index + 1))
                .collect();
            println!(
                "    {} {eco} {name} [{line}]{} — {}",
                tally(&v.tally),
                errors_text(&v.errors),
                games.join(" ")
            );
        }
    }
    if report.book.is_some() {
        let (own, other) = cr.book_exits(&report.games);
        let total = cr.games.len() as u32;
        println!(
            "-- book: left by his own move in {own} games, by the opponent's in {other}, \
             within the book for the whole line in {}",
            total - own - other
        );
        let mut by_ply: Vec<(u32, bool, u32)> = cr
            .games
            .iter()
            .filter_map(|&g| report.games[g as usize].book)
            .map(|b| (b.ply, b.by_player, 1))
            .collect();
        by_ply.sort_unstable();
        by_ply.dedup_by(|a, b| {
            if a.0 == b.0 && a.1 == b.1 {
                b.2 += a.2;
                true
            } else {
                false
            }
        });
        let text: Vec<String> = by_ply
            .iter()
            .map(|(ply, own, n)| format!("ply {ply}{}: {n}", if *own { " (his)" } else { "" }))
            .collect();
        println!("   {}", text.join(", "));
    }
    if !cr.errors.is_empty() || report.engine.is_some() {
        println!(
            "-- errors: {} inaccuracies, {} mistakes, {} blunders in the player's moves",
            cr.counts.inaccuracies, cr.counts.mistakes, cr.counts.blunders
        );
        for e in &cr.errors {
            let g = &report.games[e.game as usize];
            println!(
                "    #{} {} {:?}: {} (best {}) {} → {}  [{}]",
                g.index + 1,
                g.move_label(e.ply as usize),
                e.judgement,
                e.san,
                e.best_san,
                e.before.format(),
                e.after.format(),
                e.opening.map_or("", |o| report.openings[o].name.as_str())
            );
        }
        println!("-- repeated errors (same position, same move, two or more games)");
        for r in &cr.repeated {
            let games: Vec<String> = r
                .games
                .iter()
                .map(|&g| format!("#{}", report.games[g as usize].index + 1))
                .collect();
            println!(
                "    {}× {} {:?} (best {}) {} — {}  [{}]",
                r.times(),
                prep::move_label(r.ply as usize, &r.san),
                r.judgement,
                r.best_san,
                tally(&r.tally),
                games.join(" "),
                r.opening.map_or("", |o| report.openings[o].name.as_str())
            );
        }
    }
}

fn errors_text(e: &prep::ErrorCounts) -> String {
    if e.total() == 0 {
        String::new()
    } else {
        format!(" ?!{} ?{} ??{}", e.inaccuracies, e.mistakes, e.blunders)
    }
}

fn main() {
    let a = args();
    let db = open(&a.db);
    let never = AtomicBool::new(false);
    let cache_dir = a.cache.then(|| {
        std::env::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cache/chesspenguin/prep")
    });

    let t = Instant::now();
    let (list, total) = prep::candidates(&db, &a.player, 10);
    println!("[candidates] {total} for «{}», {:.1} ms:", a.player, t.elapsed().as_secs_f64() * 1000.0);
    for c in &list {
        println!("    {} · {} games (id {})", c.name, c.games, c.id);
    }
    let Some(chosen) = list
        .iter()
        .find(|c| c.name.to_lowercase() == a.player.to_lowercase())
        .or(list.first())
    else {
        eprintln!("no such player");
        std::process::exit(1);
    };
    let player = Player::new([chosen.id]);
    println!("[player] {} ({} games)", chosen.name, chosen.games);

    let options = Options {
        plies: a.plies,
        side: Side::Any,
    };
    let t = Instant::now();
    let cached = cache_dir
        .as_deref()
        .and_then(|d| PrepReport::load(d, &db, &player, &options));
    let mut report = match cached {
        Some(r) => {
            println!("[report] from the cache, {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            r
        }
        None => {
            let progress = AtomicUsize::new(0);
            let r = prep::build(&db, &player, &options, &progress, &never).expect("not cancelled");
            println!(
                "[report] built: {} games ({} skipped), {} plies, {} openings named, {:.1} ms",
                r.games.len(),
                r.skipped,
                r.plies,
                r.openings.len(),
                t.elapsed().as_secs_f64() * 1000.0
            );
            r
        }
    };

    // The book: another database, or this one without the player's own games.
    let t = Instant::now();
    let book_options = BookOptions {
        threshold: prep::DEFAULT_BOOK_THRESHOLD,
        same_database: a.book.is_none(),
    };
    let book = a.book.as_deref().map(open);
    let book_db = book.as_ref().unwrap_or(&db);
    if report.book.as_ref().is_some_and(|b| b.fingerprint == prep::fingerprint(book_db)) {
        println!("[book] from the cached report");
    } else {
        let progress = AtomicUsize::new(0);
        match report.find_book_exits(book_db, &book_options, &progress, &never) {
            Some(()) => println!(
                "[book] threshold {}: {} of {} games left the book, {:.2} s",
                book_options.threshold,
                report.book.as_ref().map_or(0, |b| b.left),
                report.games.len(),
                t.elapsed().as_secs_f64()
            ),
            None => println!("[book] not searchable (no memory for the position table?)"),
        }
    }

    for cr in [&report.white, &report.black] {
        print_color(&report, cr, a.min_games);
    }

    if let Some(engine) = &a.engine {
        let mut cache = cache_dir
            .as_deref()
            .map(PositionCache::load)
            .unwrap_or_default();
        println!(
            "\n[engine] {} positions, {} distinct; {} cached evaluations on disk",
            report.games.iter().map(|g| g.keys.len()).sum::<usize>(),
            report.distinct_positions(),
            cache.len()
        );
        let settings = game_review::JobSettings::new(a.processes, None);
        let t = Instant::now();
        let mut last_shown = Instant::now();
        let mut first = true;
        let result = prep::analyse(
            &mut report,
            &mut cache,
            cache_dir.as_deref(),
            uci::EngineConfig::new(engine.clone()),
            &settings,
            &never,
            &mut |p| {
                if first {
                    println!(
                        "[engine] {} positions to evaluate, {} engine processes",
                        p.total, a.processes
                    );
                    first = false;
                }
                if last_shown.elapsed().as_secs_f64() >= 5.0 {
                    println!("    {} / {} ({:.0} s, nps {})", p.done, p.total, t.elapsed().as_secs_f64(), p.nps);
                    last_shown = Instant::now();
                }
            },
        );
        match result {
            Ok(()) => println!(
                "[engine] done in {:.1} s; {} evaluations in the cache; engine {}",
                t.elapsed().as_secs_f64(),
                cache.len(),
                report.engine.as_ref().map_or("?", |e| e.name.as_str())
            ),
            Err(e) => println!("[engine] failed: {e:?}"),
        }
        for cr in [&report.white, &report.black] {
            print_color(&report, cr, u32::MAX);
        }
    }
    if let Some(dir) = &cache_dir {
        match report.save(dir) {
            Ok(()) => println!("[cache] report saved in {}", dir.display()),
            Err(e) => println!("[cache] not saved: {e}"),
        }
    }
}
