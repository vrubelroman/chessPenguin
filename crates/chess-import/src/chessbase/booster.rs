//! Our own search booster for a large ChessBase database whose `.cbb` is missing or
//! stale: the same 52-byte record per game (what was ever on the board in the main
//! line, see `Signature` in `search.rs`), computed from `.cbg` once and kept in the
//! user's cache directory under a name derived from the database (path, size and
//! date of `.cbg`), so a changed database gets a new file. The format is that of
//! `.cbb`, header included, so the search reads both the same way; our record matched
//! ChessBase's byte for byte on Mega (`docs/research/position-index.md`, section 1.1).

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use shakmaty::{Board, Chess, Color, Position, Role};

use super::base::{Base, RecordKind};
use super::be;
use super::moves::{self, Step, WalkError};
use super::search::{BLOCK, CBB_RECORD, read_games};

/// Marks in `r` what stands on `b`: the layout of a `.cbb` record.
fn mark(b: &Board, r: &mut [u8; CBB_RECORD]) {
    for (sq, piece) in b.clone() {
        let file = 1u8 << (sq.file() as u32);
        let rank = sq.rank() as usize;
        let white = piece.color == Color::White;
        match piece.role {
            Role::Pawn if white && (2..=6).contains(&rank) => r[0x06 + rank - 2] |= file,
            Role::Pawn if !white && (1..=5).contains(&rank) => r[0x0b + rank - 1] |= file,
            Role::Pawn => {}
            Role::King => {
                let (rk, f) = if white { (0x10, 0x12) } else { (0x11, 0x13) };
                r[rk] |= 1 << rank;
                r[f] |= file;
            }
            Role::Rook | Role::Queen => r[if white { 0x14 } else { 0x1c } + rank] |= file,
            Role::Knight | Role::Bishop => r[if white { 0x24 } else { 0x2c } + rank] |= file,
        }
    }
    for (c, shift) in [(Color::White, 0), (Color::Black, 4)] {
        let count = |role: Role| (b.by_color(c) & b.by_role(role)).count();
        let (q, rook, bishop, knight) = (
            count(Role::Queen),
            count(Role::Rook),
            count(Role::Bishop),
            count(Role::Knight),
        );
        // "Was without" (byte 5) and "had at most one" (byte 4); for the queen
        // ChessBase sets the bit of byte 4 together with that of byte 5.
        if q == 0 {
            r[0x05] |= 1 << shift;
            r[0x04] |= 1 << shift;
        }
        for (flag, n) in [(2u8, rook), (4, bishop), (8, knight)] {
            if n == 0 {
                r[0x05] |= flag << shift;
            }
            if n <= 1 {
                r[0x04] |= flag << shift;
            }
        }
        if q + rook + bishop + knight == 0 {
            r[0x03] |= 8 << shift;
        }
    }
}

fn walk(cbg: &[u8], offset: u32, fast: bool) -> Result<[u8; CBB_RECORD], WalkError> {
    let mut r = [0u8; CBB_RECORD];
    let mut end_pawns = 0u8;
    moves::main_line(cbg, offset, fast, |pos: &Chess, step| {
        let b = pos.board();
        mark(b, &mut r);
        if matches!(step, Step::End) {
            let pawns = |c: Color| (b.by_color(c) & b.pawns()).count().min(7) as u8;
            end_pawns = pawns(Color::White) | (pawns(Color::Black) << 4);
        }
        true
    })?;
    r[0x03] |= end_pawns;
    Ok(r)
}

/// The booster record of the game at `offset` in `cbg`: the fast walk, and the
/// checked one when the fast one stumbles. `None` when the game cannot be read (a
/// position search cannot find it either).
pub(super) fn record_of(cbg: &[u8], offset: u32) -> Option<[u8; CBB_RECORD]> {
    match walk(cbg, offset, true) {
        Ok(r) => Some(r),
        Err(WalkError::Moves(_)) => walk(cbg, offset, false).ok(),
        Err(WalkError::Unsupported(_)) => None,
    }
}

fn write_all_at(f: &File, buf: &[u8], at: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::write_all_at(f, buf, at)
    }
    #[cfg(windows)]
    {
        let (mut buf, mut at) = (buf, at);
        while !buf.is_empty() {
            match std::os::windows::fs::FileExt::seek_write(f, buf, at)? {
                0 => return Err(io::ErrorKind::WriteZero.into()),
                n => {
                    buf = &buf[n..];
                    at += n as u64;
                }
            }
        }
        Ok(())
    }
}

/// Writes the booster of `base` to `out` (a `.part` file next to it first, renamed
/// when complete): one pass over the main lines, in parallel. `progress` counts the
/// records done. `Ok(false)` when cancelled; then nothing is left behind.
pub(super) fn build(
    base: &Base,
    out: &Path,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> io::Result<bool> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = out.with_extension("part");
    let count = base.file_records() + 1;
    File::create(&part)?.set_len((count * CBB_RECORD) as u64)?;
    let total = base.records();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(total.div_ceil(BLOCK))
        .max(1);
    let failure: Mutex<Option<io::Error>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let run = || -> io::Result<()> {
                    let cbg = File::open(base.cbg_path())?;
                    let file = File::options().write(true).open(&part)?;
                    let mut games: Vec<(u32, u32)> = Vec::new();
                    let (mut span, mut single) = (Vec::new(), Vec::new());
                    let mut block = vec![0u8; BLOCK * CBB_RECORD];
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        let from = next.fetch_add(BLOCK, Ordering::Relaxed);
                        if from >= total {
                            return Ok(());
                        }
                        let to = (from + BLOCK).min(total);
                        let bytes = &mut block[..(to - from) * CBB_RECORD];
                        bytes.fill(0);
                        games.clear();
                        for i in from..to {
                            // Texts and Chess960 games (ECO field ≥ 64576) have no
                            // record, like in ChessBase's own file.
                            if base.kind(i) == RecordKind::Game
                                && be(base.record(i), 0x23, 2).is_some_and(|eco| eco < 64576)
                            {
                                games.push((i as u32, base.moves_offset(i)));
                            }
                        }
                        read_games(&cbg, &games, &mut span, &mut single, |index, data, rel| {
                            if let Some(r) = record_of(data, rel) {
                                let at = (index as usize - from) * CBB_RECORD;
                                bytes[at..at + CBB_RECORD].copy_from_slice(&r);
                            }
                        });
                        write_all_at(&file, bytes, ((from + 1) * CBB_RECORD) as u64)?;
                        progress.fetch_add(to - from, Ordering::Relaxed);
                    }
                };
                if let Err(e) = run() {
                    cancel.store(true, Ordering::Relaxed);
                    if let Ok(mut f) = failure.lock() {
                        f.get_or_insert(e);
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap_or(None) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&part);
        return Ok(false);
    }
    // The header last: a file without the count is never taken for a booster.
    let mut header = [0u8; CBB_RECORD];
    header[..4].copy_from_slice(&(count as u32).to_be_bytes());
    let file = File::options().write(true).open(&part)?;
    write_all_at(&file, &header, 0)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, out)?;
    Ok(true)
}
