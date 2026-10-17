//! Prints diagnostics and the normalized PGN export of one record.
//! Usage: cargo run -p chess-core --example pgn_dump -- <file.pgn> <1-based index>

use std::sync::atomic::AtomicBool;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(index)) = (args.next(), args.next()) else {
        eprintln!("usage: pgn_dump <file.pgn> <1-based index>");
        std::process::exit(2);
    };
    let Ok(index) = index.parse::<usize>() else {
        eprintln!("index must be a number");
        std::process::exit(2);
    };
    let db = match chess_core::PgnDatabase::open(
        std::path::Path::new(&path),
        &mut |_| {},
        &AtomicBool::new(false),
    ) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("{} records, encoding {}", db.len(), db.encoding().label());
    let Some(parsed) = index.checked_sub(1).and_then(|i| db.load_game(i)) else {
        eprintln!("no record {index}");
        std::process::exit(1);
    };
    for d in &parsed.diagnostics {
        println!("# {d}");
    }
    print!("{}", parsed.game.to_pgn());
}
