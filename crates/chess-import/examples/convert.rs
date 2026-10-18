//! Converts a database to PGN on stdout; notes go to stderr.
//!
//!     cargo run -p chess-import --example convert -- "Some Book.cbv" > book.pgn

use std::io::Write;
use std::sync::atomic::AtomicBool;

fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: convert <file.pgn|.cbh|.cbv|.zip|.7z>");
        std::process::exit(2);
    };
    let never = AtomicBool::new(false);
    match chess_import::open(std::path::Path::new(&path), &mut |_| {}, &never) {
        Ok(o) => {
            eprintln!("{}: {} games", o.format.label(), o.db.len());
            for n in &o.notes {
                eprintln!("note: {n}");
            }
            let mut out = std::io::stdout().lock();
            for i in 0..o.db.len() {
                if let Some(text) = o.db.raw_text(i) {
                    let _ = writeln!(out, "{}\n", text.trim_end());
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
