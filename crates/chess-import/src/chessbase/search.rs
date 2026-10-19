//! Position search in a large ChessBase database: the main line of every candidate
//! game is walked with a fast decoder (moves built from the board, no legal move
//! generation), games that can no longer reach the position are dropped early, and
//! the `.cbb` search booster rules out games beforehand when it is valid.
//! Research and measurements: `docs/research/position-index.md`.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use shakmaty::uci::UciMove;
use shakmaty::{Bitboard, Board, Chess, Color, Position, Rank, Role};

use super::base::{Base, RecordKind};
use super::be;
use super::moves::{self, Step, WalkError};
use crate::position::{
    Collector, GameResult, Match, PositionOutcome, PositionQuery, same_position, uci,
};

/// Candidates handed to a thread at a time; cancellation is checked between blocks.
pub(super) const BLOCK: usize = 2048;
/// Games whose moves lie closer than this are read with one `pread`.
const GAP: u32 = 64 << 10;
/// Longest span of `.cbg` read at once.
const SPAN: u32 = 4 << 20;
/// Read past the last record start of a span (most records are shorter).
const TAIL: u32 = 4096;
/// Size of a `.cbb` record (the file starts with a header of the same size).
pub(super) const CBB_RECORD: usize = 52;
/// Booster records sampled before a search; when most of them admit the target the
/// booster is left out (it would read its whole file to rule out almost nothing).
const SAMPLE: usize = 4096;

/// Games among `candidates` (record indices in ascending order; `None`: all records)
/// whose main line reaches the position. `scanned` counts the candidates looked at.
/// `None` when cancelled.
pub fn search(
    base: &Base,
    candidates: Option<&[u32]>,
    query: &PositionQuery,
    scanned: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<PositionOutcome> {
    let total = candidates.map_or(base.records(), <[u32]>::len);
    // A position of the opening: the index knows its games.
    if let Some(index) = base.openings()
        && index.answers(query, candidates)
    {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        scanned.fetch_add(total, Ordering::Relaxed);
        return Some(index.search(base, candidates, query));
    }
    let target = Target::new(query);
    let cbb = base
        .cbb_path()
        .filter(|p| booster_helps(p, &target.signature));
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(total.div_ceil(BLOCK))
        .max(1);
    // Per thread: its collector and, per block, (first candidate, first match).
    type Part = (Collector, Vec<(usize, usize)>);
    let parts: Vec<Option<Part>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut worker = Worker {
                        base,
                        target: &target,
                        cbg: File::open(base.cbg_path()).ok()?,
                        cbb: cbb.and_then(|p| File::open(p).ok()),
                        games: Vec::new(),
                        span: Vec::new(),
                        single: Vec::new(),
                        out: Collector::default(),
                    };
                    let mut blocks = Vec::new();
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                        let from = next.fetch_add(BLOCK, Ordering::Relaxed);
                        if from >= total {
                            return Some((worker.out, blocks));
                        }
                        let to = (from + BLOCK).min(total);
                        blocks.push((from, worker.out.matches.len()));
                        match candidates {
                            Some(c) => worker.block(c[from..to].iter().copied()),
                            None => worker.block(from as u32..to as u32),
                        }
                        scanned.fetch_add(to - from, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // A thread without a result could not open `.cbg`: the other threads could not
    // either, so there is simply nothing found.
    let mut parts: Vec<Part> = parts.into_iter().flatten().collect();
    // Matches in record order by putting the blocks in order (the start position
    // matches every game: 11.7 million in Mega, too many to sort and grow casually).
    let mut order: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (p, (c, blocks)) in parts.iter().enumerate() {
        for (k, &(from, start)) in blocks.iter().enumerate() {
            let end = blocks.get(k + 1).map_or(c.matches.len(), |b| b.1);
            order.push((from, p, start, end));
        }
    }
    order.sort_unstable();
    let mut matches = Vec::with_capacity(parts.iter().map(|p| p.0.matches.len()).sum());
    for (_, p, start, end) in order {
        matches.extend_from_slice(&parts[p].0.matches[start..end]);
    }
    let mut all = Collector::default();
    for (mut c, _) in parts.drain(..) {
        c.matches = Vec::new();
        all.merge(c);
    }
    all.matches = matches;
    Some(all.finish(&query.position))
}

/// The searched position, prepared for cheap checks.
struct Target {
    pos: Chess,
    occupied: Bitboard,
    /// Men of each side, pawns of each side (none of them can grow in number).
    men: [usize; 2],
    pawns: [usize; 2],
    /// Pawns on their start squares (a pawn that has left cannot come back).
    home: [Bitboard; 2],
    line: Option<Vec<UciMove>>,
    start: Chess,
    signature: Signature,
}

impl Target {
    fn new(query: &PositionQuery) -> Target {
        let b = query.position.board();
        let side = |c: Color| b.by_color(c);
        let pawns = |c: Color| side(c) & b.pawns();
        Target {
            pos: query.position.clone(),
            occupied: b.occupied(),
            men: [side(Color::White).count(), side(Color::Black).count()],
            pawns: [pawns(Color::White).count(), pawns(Color::Black).count()],
            home: [
                pawns(Color::White) & Bitboard::from_rank(Rank::Second),
                pawns(Color::Black) & Bitboard::from_rank(Rank::Seventh),
            ],
            line: query.line.as_ref().map(|l| l.iter().map(uci).collect()),
            start: Chess::default(),
            signature: Signature::new(b),
        }
    }

    /// Whether a game in this position may still reach the target.
    fn reachable(&self, b: &Board) -> bool {
        let (w, bl) = (b.white(), b.black());
        let (wp, bp) = (w & b.pawns(), bl & b.pawns());
        w.count() >= self.men[0]
            && bl.count() >= self.men[1]
            && wp.count() >= self.pawns[0]
            && bp.count() >= self.pawns[1]
            && (wp & self.home[0]) == self.home[0]
            && (bp & self.home[1]) == self.home[1]
    }

    /// The first time the main line of the record at `offset` reaches the target.
    fn find(&self, cbg: &[u8], offset: u32, fast: bool) -> Result<Option<Hit>, WalkError> {
        let mut ply = 0u32;
        let mut in_order = self.line.is_some();
        let mut hit = None;
        moves::main_line(cbg, offset, fast, |pos, step| {
            if ply == 0 && in_order && !same_position(pos, &self.start) {
                in_order = false;
            }
            let b = pos.board();
            if b.occupied() == self.occupied && same_position(pos, &self.pos) {
                hit = Some(Hit {
                    ply,
                    same_order: in_order
                        && self.line.as_ref().is_some_and(|l| l.len() == ply as usize),
                    next: match step {
                        Step::Move(m) => Some(uci(m)),
                        Step::Null => Some(UciMove::Null),
                        Step::End => None,
                    },
                });
                return false;
            }
            if !self.reachable(b) {
                return false;
            }
            if in_order {
                let wanted = self.line.as_ref().and_then(|l| l.get(ply as usize));
                in_order = matches!((step, wanted), (Step::Move(m), Some(w)) if uci(m) == *w);
            }
            ply += 1;
            true
        })?;
        Ok(hit)
    }
}

struct Hit {
    ply: u32,
    same_order: bool,
    next: Option<UciMove>,
}

/// What the `.cbb` record of a game that passes through the target must hold.
///
/// A `.cbb` record (52 bytes, per game) accumulates what was ever on the board in
/// the main line, start and final positions included: the squares of white pawns
/// on ranks 3-7 (bytes 6-10, one byte per rank, bit = file) and of black pawns on
/// ranks 2-6 (11-15), the ranks and files of each king (16-19), the squares of
/// rooks and queens (white 20-27, black 28-35) and of knights and bishops (36-43,
/// 44-51), one byte per rank; per side (low nibble white, high black) "was without
/// a queen / rook / bishop / knight" (byte 5, bits 0-3), "had at most one rook /
/// bishop / knight" (byte 4, bits 1-3), "had only pawns and the king" (byte 3, bit 3)
/// and the number of pawns at the end, 7 meaning 7 or 8 (byte 3, bits 0-2). Our own
/// computation matched the Mega file byte for byte (research, section 1.1).
struct Signature {
    need: [u8; CBB_RECORD],
    end_pawns: [u8; 2],
}

impl Signature {
    fn new(b: &Board) -> Signature {
        let mut need = [0u8; CBB_RECORD];
        for (sq, piece) in b.clone() {
            let file = 1u8 << (sq.file() as u32);
            let rank = sq.rank() as usize;
            let white = piece.color == Color::White;
            match piece.role {
                Role::Pawn if white && rank >= 2 => need[0x06 + rank - 2] |= file,
                Role::Pawn if !white && (1..=5).contains(&rank) => need[0x0b + rank - 1] |= file,
                Role::Pawn => {}
                Role::King => {
                    let (r, f) = if white { (0x10, 0x12) } else { (0x11, 0x13) };
                    need[r] |= 1 << rank;
                    need[f] |= file;
                }
                Role::Rook | Role::Queen => need[if white { 0x14 } else { 0x1c } + rank] |= file,
                Role::Knight | Role::Bishop => need[if white { 0x24 } else { 0x2c } + rank] |= file,
            }
        }
        let mut end_pawns = [0; 2];
        for (c, shift) in [(Color::White, 0), (Color::Black, 4)] {
            let count = |role: Role| (b.by_color(c) & b.by_role(role)).count();
            let (q, r, bi, n) = (
                count(Role::Queen),
                count(Role::Rook),
                count(Role::Bishop),
                count(Role::Knight),
            );
            for (flag, none) in [(1u8, q), (2, r), (4, bi), (8, n)] {
                if none == 0 {
                    need[0x05] |= flag << shift;
                }
            }
            for (flag, most) in [(2u8, r), (4, bi), (8, n)] {
                if most <= 1 {
                    need[0x04] |= flag << shift;
                }
            }
            if q + r + bi + n == 0 {
                need[0x03] |= 8 << shift;
            }
            end_pawns[shift / 4] = count(Role::Pawn).min(7) as u8;
        }
        Signature { need, end_pawns }
    }

    /// False when the game cannot have passed through the target.
    fn admits(&self, rec: &[u8]) -> bool {
        rec.len() >= CBB_RECORD
            && self.need.iter().zip(rec).all(|(n, r)| r & n == *n)
            && rec[3] & 7 <= self.end_pawns[0]
            && (rec[3] >> 4) & 7 <= self.end_pawns[1]
    }
}

/// One search thread: its own file handles and buffers.
struct Worker<'a> {
    base: &'a Base,
    target: &'a Target,
    cbg: File,
    cbb: Option<File>,
    /// Games of the current block: record index and moves offset.
    games: Vec<(u32, u32)>,
    span: Vec<u8>,
    single: Vec<u8>,
    out: Collector,
}

impl Worker<'_> {
    fn block(&mut self, indices: impl Iterator<Item = u32>) {
        self.games.clear();
        for i in indices {
            let r = self.base.record(i as usize);
            // Chess960 games (ECO field ≥ 64576) cannot be read.
            if self.base.kind(i as usize) == RecordKind::Game
                && be(r, 0x23, 2).is_some_and(|eco| eco < 64576)
            {
                self.games.push((i, self.base.moves_offset(i as usize)));
            }
        }
        self.booster();
        let Worker {
            base,
            target,
            cbg,
            games,
            span,
            single,
            out,
            ..
        } = self;
        read_games(cbg, games, span, single, |index, data, rel| {
            if let Some(h) = find(target, data, rel) {
                add(base, out, index, h);
            }
        });
    }

    /// Drops the games of the block that the `.cbb` booster rules out.
    fn booster(&mut self) {
        let (Some(cbb), Some(&(first, _)), Some(&(last, _))) =
            (&self.cbb, self.games.first(), self.games.last())
        else {
            return;
        };
        let at = |i: u32| (i as u64 + 1) * CBB_RECORD as u64;
        let records = (last - first) as usize + 1;
        let sig = &self.target.signature;
        if records <= 4 * self.games.len() + 64 {
            // Dense candidates: one read for the whole block.
            read_span(cbb, at(first), &mut self.span, records * CBB_RECORD);
            let span = &self.span;
            self.games.retain(|&(i, _)| {
                let r = (i - first) as usize * CBB_RECORD;
                span.get(r..r + CBB_RECORD)
                    .is_none_or(|rec| sig.admits(rec))
            });
        } else {
            let mut rec = [0u8; CBB_RECORD];
            self.games
                .retain(|&(i, _)| read_exact_at(cbb, &mut rec, at(i)).is_err() || sig.admits(&rec));
        }
    }
}

/// Records the game `index` that reached the target as `hit`.
fn add(base: &Base, out: &mut Collector, index: u32, hit: Hit) {
    let r = base.record(index as usize);
    let u = |at, n| be(r, at, n).unwrap_or(0);
    let result = match r.get(0x1b) {
        Some(2 | 6) => GameResult::WhiteWins,
        Some(1 | 5) => GameResult::Draw,
        Some(0 | 4) => GameResult::BlackWins,
        _ => GameResult::Unknown,
    };
    let year = ((u(0x18, 3) >> 9) & 0xfff) as u16;
    out.add(
        Match {
            index,
            ply: hit.ply,
            same_order: hit.same_order,
            in_variation: false,
            next: None,
        },
        hit.next,
        result,
        u(0x1f, 2) as u16,
        u(0x21, 2) as u16,
        year,
    );
}

/// Calls `f(index, data, offset)` with the moves of every game of `games` (record
/// index and `.cbg` offset, in file order within a run): games whose records lie
/// close together are read with one `pread` into `span`, a record running past the
/// span is read alone into `single`.
pub(super) fn read_games(
    cbg: &File,
    games: &[(u32, u32)],
    span: &mut Vec<u8>,
    single: &mut Vec<u8>,
    mut f: impl FnMut(u32, &[u8], u32),
) {
    let mut start = 0;
    while start < games.len() {
        // A run of games whose moves lie close together, in file order.
        let lo = games[start].1;
        let mut end = start + 1;
        while let Some(&(_, off)) = games.get(end) {
            let prev = games[end - 1].1;
            if off < prev || off - prev > GAP || off - lo > SPAN {
                break;
            }
            end += 1;
        }
        let hi = games[end - 1].1;
        let len = (hi - lo) as usize + TAIL as usize;
        read_span(cbg, u64::from(lo), span, len);
        for &(index, off) in &games[start..end] {
            let rel = off - lo;
            let size = be(span, rel as usize + 1, 3).unwrap_or(u32::MAX) as usize;
            if rel as usize + size <= span.len() {
                f(index, span, rel);
            } else {
                // A record running past the span: read it alone.
                if read_record(cbg, off, single) == 0 {
                    continue;
                }
                f(index, single, 0);
            }
        }
        start = end;
    }
}

/// Whether the booster at `path` rules out a worthwhile share of games for `sig`,
/// judged on its first records: early positions (the start, 1.e4) pass nearly every
/// game, and reading the file for them only costs time.
fn booster_helps(path: &Path, sig: &Signature) -> bool {
    let Ok(f) = File::open(path) else {
        return false;
    };
    let mut buf = Vec::new();
    read_span(&f, CBB_RECORD as u64, &mut buf, SAMPLE * CBB_RECORD);
    let records = buf.as_chunks::<CBB_RECORD>().0;
    if records.is_empty() {
        return false;
    }
    let admitted = records.iter().filter(|r| sig.admits(*r)).count();
    admitted * 2 < records.len()
}

/// The fast walk, and the checked one when the fast one stumbles (damaged moves).
/// A game that neither can read is not found.
fn find(target: &Target, cbg: &[u8], offset: u32) -> Option<Hit> {
    match target.find(cbg, offset, true) {
        Ok(hit) => hit,
        Err(WalkError::Moves(_)) => target.find(cbg, offset, false).ok().flatten(),
        Err(WalkError::Unsupported(_)) => None,
    }
}

/// Reads at a position without moving a shared cursor (`pread` on Unix; on Windows
/// the handle's cursor moves, but every thread has its own handle).
fn read_at(f: &File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(f, buf, at)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(f, buf, at)
    }
}

fn read_exact_at(f: &File, mut buf: &mut [u8], mut at: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        match read_at(f, buf, at)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                at += n as u64;
            }
        }
    }
    Ok(())
}

/// Reads up to `len` bytes at `at` into `buf` (shorter at the end of the file).
fn read_span(f: &File, at: u64, buf: &mut Vec<u8>, len: usize) {
    buf.resize(len, 0);
    let mut done = 0;
    while done < len {
        match read_at(f, &mut buf[done..], at + done as u64) {
            Ok(0) | Err(_) => break,
            Ok(n) => done += n,
        }
    }
    buf.truncate(done);
}

/// Reads the `.cbg` record at `offset` into `buf`; its size, 0 when unreadable.
fn read_record(f: &File, offset: u32, buf: &mut Vec<u8>) -> usize {
    let mut head = [0u8; 4];
    if read_exact_at(f, &mut head, u64::from(offset)).is_err() {
        return 0;
    }
    let size = be(&head, 1, 3).unwrap_or(0) as usize;
    // Records hold at most a few megabytes; a larger size means damaged data.
    if !(4..=64 << 20).contains(&size) {
        return 0;
    }
    buf.resize(size, 0);
    if read_exact_at(f, buf, u64::from(offset)).is_err() {
        return 0;
    }
    size
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::position::Tally;
    use shakmaty::san::SanPlus;
    use shakmaty::{CastlingMode, EnPassantMode, FromSetup, Move, fen::Fen};

    /// A sample `.cbv` unpacked into a fresh temporary directory (removed on drop).
    struct Sample {
        dir: PathBuf,
        cbh: PathBuf,
    }

    impl Sample {
        fn unpack(name: &str) -> Option<Sample> {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../pgnFilesforExample")
                .join(name);
            let Ok(bytes) = std::fs::read(&path) else {
                eprintln!("skipped: {} not found", path.display());
                return None;
            };
            let dir = std::env::temp_dir().join(format!(
                "chess-import-search-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let mut cbh = None;
            for m in crate::cbv::unpack(&bytes).expect("unpacks") {
                let file = m.name.rsplit('/').next().unwrap_or(&m.name).to_owned();
                let p = dir.join(&file);
                std::fs::write(&p, &m.data).expect("write");
                if file.to_ascii_lowercase().ends_with(".cbh") {
                    cbh = Some(p);
                }
            }
            Some(Sample {
                dir,
                cbh: cbh.expect("a .cbh member"),
            })
        }

        fn open(&self) -> Base {
            Base::open(&self.cbh, &mut |_, _| {}, &AtomicBool::new(false)).expect("opens")
        }
    }

    impl Drop for Sample {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Main-line positions with the move played from each.
    type Line = Vec<(Chess, Option<UciMove>)>;

    /// Main-line positions of a record by the tree decoder, with the move played
    /// from each (`None` after the last); `None` for an unreadable game.
    fn oracle_line(base: &Base, index: usize) -> Option<Line> {
        if base.kind(index) != RecordKind::Game {
            return None;
        }
        let cbg = std::fs::read(base.cbg_path()).expect("cbg");
        let tree = moves::decode(&cbg, base.moves_offset(index)).ok()?;
        let mut pos: Chess = match &tree.start_fen {
            Some(f) => {
                let setup = f.parse::<Fen>().ok()?.into_setup();
                match Chess::from_setup(setup, CastlingMode::Standard) {
                    Ok(p) => p,
                    Err(e) => match e.ignore_invalid_castling_rights() {
                        Ok(p) => p,
                        Err(e) => e.ignore_invalid_ep_square().ok()?,
                    },
                }
            }
            None => Chess::default(),
        };
        let mut out = Vec::new();
        let mut node = 0;
        while let Some(&c) = tree.nodes[node].children.first() {
            let san = &tree.nodes[c].san;
            if san == "--" {
                out.push((pos.clone(), Some(UciMove::Null)));
                let mut setup = pos.to_setup(EnPassantMode::Legal);
                setup.swap_turn();
                setup.ep_square = None;
                pos = Chess::from_setup(setup, CastlingMode::Standard).ok()?;
            } else {
                let m: Move = san.parse::<SanPlus>().ok()?.san.to_move(&pos).ok()?;
                out.push((pos.clone(), Some(uci(&m))));
                pos.play_unchecked(m);
            }
            node = c;
        }
        out.push((pos, None));
        Some(out)
    }

    /// Expected (index, ply, next move) of the games that reach `target`.
    fn oracle(
        lines: &[Option<Line>],
        candidates: Option<&[u32]>,
        target: &Chess,
    ) -> Vec<(u32, u32, Option<UciMove>)> {
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if candidates.is_some_and(|c| !c.contains(&(i as u32))) {
                continue;
            }
            let Some(line) = line else { continue };
            if let Some(ply) = line.iter().position(|(p, _)| same_position(p, target)) {
                out.push((i as u32, ply as u32, line[ply].1));
            }
        }
        out
    }

    fn run(base: &Base, candidates: Option<&[u32]>, query: &PositionQuery) -> PositionOutcome {
        let scanned = AtomicUsize::new(0);
        let out = search(base, candidates, query, &scanned, &AtomicBool::new(false))
            .expect("not cancelled");
        assert_eq!(
            scanned.load(Ordering::Relaxed),
            candidates.map_or(base.records(), <[u32]>::len)
        );
        out
    }

    /// Search results as (index, ply), with the total of continuations.
    fn summary(out: &PositionOutcome) -> (Vec<(u32, u32)>, u32) {
        let pairs = out.matches.iter().map(|m| (m.index, m.ply)).collect();
        let next: u32 = out.moves.iter().map(|m| m.tally.games).sum();
        (pairs, next + out.ended.games)
    }

    /// Targets: the start position and positions along a few games, the last one
    /// included.
    fn targets(lines: &[Option<Line>]) -> Vec<Chess> {
        let mut out = vec![Chess::default()];
        for line in lines.iter().flatten().step_by(37).take(6) {
            for ply in [4, 11, 24, line.len() - 1] {
                if let Some((p, _)) = line.get(ply) {
                    out.push(p.clone());
                }
            }
        }
        out
    }

    fn check_sample(name: &str) {
        let Some(sample) = Sample::unpack(name) else {
            return;
        };
        let base = sample.open();
        let lines: Vec<_> = (0..base.records()).map(|i| oracle_line(&base, i)).collect();
        let every_third: Vec<u32> = (0..base.records() as u32).step_by(3).collect();
        for target in targets(&lines) {
            let query = PositionQuery::new(target.clone(), None);
            for candidates in [None, Some(every_third.as_slice())] {
                let want = oracle(&lines, candidates, &target);
                let out = run(&base, candidates, &query);
                let (got, continuations) = summary(&out);
                let want_pairs: Vec<(u32, u32)> = want.iter().map(|w| (w.0, w.1)).collect();
                assert_eq!(
                    got,
                    want_pairs,
                    "{name}: {}",
                    Fen::from_position(&target, EnPassantMode::Legal)
                );
                // Null moves have no SAN and are left out of the continuations.
                let nulls = want.iter().filter(|w| w.2 == Some(UciMove::Null)).count();
                assert_eq!(continuations as usize + nulls, want.len());
                let ended = want.iter().filter(|w| w.2.is_none()).count();
                assert_eq!(out.ended.games as usize, ended);
            }
        }
    }

    #[test]
    fn finds_what_the_tree_decoder_finds() {
        // Games from the usual start (Anand) and from set positions (Endgame Manual).
        check_sample("Anand 01-14.cbv");
        check_sample("Endgame Manual.cbv");
    }

    #[test]
    fn move_order_and_cancel() {
        let Some(sample) = Sample::unpack("Anand 01-14.cbv") else {
            return;
        };
        let base = sample.open();
        let lines: Vec<_> = (0..base.records()).map(|i| oracle_line(&base, i)).collect();
        // The moves of a game, as the user's line: that game is in the same order.
        let (index, line) = lines
            .iter()
            .enumerate()
            .find_map(|(i, l)| l.as_ref().filter(|l| l.len() > 16).map(|l| (i, l)))
            .expect("a long game");
        let mut pos = Chess::default();
        let mut moves = Vec::new();
        for (_, m) in &line[..14] {
            let m = m.as_ref().expect("a move").to_move(&pos).expect("legal");
            moves.push(m);
            pos.play_unchecked(m);
        }
        let out = run(
            &base,
            None,
            &PositionQuery::new(pos.clone(), Some(moves.clone())),
        );
        let m = out
            .matches
            .iter()
            .find(|m| m.index == index as u32)
            .expect("the game itself");
        assert!(m.same_order && m.ply == 14 && !m.in_variation);
        for m in &out.matches {
            // Every game in the same order played exactly these moves.
            let l = lines[m.index as usize].as_ref().expect("readable");
            let prefix: Vec<UciMove> = l[..m.ply as usize].iter().filter_map(|(_, u)| *u).collect();
            let same = m.ply == 14 && prefix == moves.iter().map(uci).collect::<Vec<_>>();
            assert_eq!(m.same_order, same, "game {}", m.index);
        }
        // Without a line nothing is "in the same order".
        let out = run(&base, None, &PositionQuery::new(pos.clone(), None));
        assert!(out.matches.iter().all(|m| !m.same_order));
        let cancel = AtomicBool::new(true);
        let query = PositionQuery::new(pos, None);
        assert!(search(&base, None, &query, &AtomicUsize::new(0), &cancel).is_none());
    }

    /// Our own `.cbb` record of a game (what the booster holds), from the main line.
    fn own_cbb(line: &[(Chess, Option<UciMove>)]) -> [u8; CBB_RECORD] {
        let mut r = [0u8; CBB_RECORD];
        for (pos, _) in line {
            let sig = Signature::new(pos.board());
            // What a position requires is what it leaves in the record, except the
            // final pawn count.
            for (r, n) in r.iter_mut().zip(sig.need) {
                *r |= n;
            }
        }
        let (fin, _) = line.last().expect("a position");
        let b = fin.board();
        let pawns = |c: Color| (b.by_color(c) & b.pawns()).count().min(7) as u8;
        r[3] |= pawns(Color::White) | (pawns(Color::Black) << 4);
        r
    }

    #[test]
    fn booster_rules_out_games_but_loses_none() {
        let Some(sample) = Sample::unpack("Anand 01-14.cbv") else {
            return;
        };
        let base = sample.open();
        let lines: Vec<_> = (0..base.records()).map(|i| oracle_line(&base, i)).collect();
        // A booster as ChessBase writes it: a header record with the count, then a
        // record per game (zeros for texts and unreadable games).
        let count = base.records() as u32 + 1;
        let mut cbb = vec![0u8; CBB_RECORD];
        cbb[..4].copy_from_slice(&count.to_be_bytes());
        for line in &lines {
            cbb.extend(line.as_deref().map_or([0; CBB_RECORD], own_cbb));
        }
        let path = sample.cbh.with_extension("cbb");
        let targets = targets(&lines);
        let without: Vec<_> = targets
            .iter()
            .map(|t| summary(&run(&base, None, &PositionQuery::new(t.clone(), None))))
            .collect();

        // A booster with the wrong count, or older than the games, is not used.
        std::fs::write(&path, &cbb[..cbb.len() - CBB_RECORD]).expect("write");
        assert!(sample.open().cbb_path().is_none());
        std::fs::write(&path, &cbb).expect("write");
        let older = std::fs::metadata(base.cbg_path())
            .expect("cbg")
            .modified()
            .expect("time")
            - std::time::Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(older)
            .expect("touch");
        assert!(sample.open().cbb_path().is_none());
        std::fs::write(&path, &cbb).expect("write");
        let base = sample.open();
        assert!(base.cbb_path().is_some());

        for (t, want) in targets.iter().zip(&without) {
            let got = summary(&run(&base, None, &PositionQuery::new(t.clone(), None)));
            assert_eq!(&got, want);
            // And it does rule games out.
            let sig = Signature::new(t.board());
            let admitted = (0..base.records())
                .filter(|&i| sig.admits(&cbb[(i + 1) * CBB_RECORD..]))
                .count();
            assert!(admitted >= want.0.len());
        }
        let deep = targets.last().expect("a target");
        let sig = Signature::new(deep.board());
        let admitted = (0..base.records())
            .filter(|&i| sig.admits(&cbb[(i + 1) * CBB_RECORD..]))
            .count();
        assert!(
            admitted < base.records() / 2,
            "{admitted} of {}",
            base.records()
        );
    }

    fn san_line(line: &str) -> (Chess, Vec<Move>) {
        let mut pos = Chess::default();
        let mut moves = Vec::new();
        for s in line.split_whitespace() {
            let m = s
                .parse::<SanPlus>()
                .expect("SAN")
                .san
                .to_move(&pos)
                .expect("legal");
            moves.push(m);
            pos.play_unchecked(m);
        }
        (pos, moves)
    }

    fn peak_memory() -> String {
        std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("VmHWM"))
            .map(|l| l.trim_start_matches("VmHWM:").trim().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn own_booster_when_cbb_is_missing() {
        let Some(sample) = Sample::unpack("Anand 01-14.cbv") else {
            return;
        };
        let base = sample.open();
        assert!(base.cbb_path().is_none() && !base.booster_wanted());
        let lines: Vec<_> = (0..base.records()).map(|i| oracle_line(&base, i)).collect();
        let targets = targets(&lines);
        let without: Vec<_> = targets
            .iter()
            .map(|t| summary(&run(&base, None, &PositionQuery::new(t.clone(), None))))
            .collect();

        let dir = sample.dir.join("cache");
        base.set_booster_dir(&dir);
        assert!(base.booster_wanted() && !base.booster_ready());
        // Cancelled: nothing is left behind.
        let cancelled = AtomicBool::new(true);
        assert!(
            !base
                .build_booster(&AtomicUsize::new(0), &cancelled)
                .expect("builds")
        );
        assert!(std::fs::read_dir(&dir).map_or(true, |d| d.count() == 0));
        let progress = AtomicUsize::new(0);
        assert!(
            base.build_booster(&progress, &AtomicBool::new(false))
                .expect("builds")
        );
        assert_eq!(progress.load(Ordering::Relaxed), base.records());
        let path = base.cbb_path().expect("our booster").to_owned();
        assert!(path.starts_with(&dir) && !base.booster_wanted());
        let cbb = std::fs::read(&path).expect("file");
        assert_eq!(cbb.len(), (base.records() + 1) * CBB_RECORD);
        assert_eq!(be(&cbb, 0, 4), Some(base.records() as u32 + 1));
        // Our record holds everything the search may require of a game's record.
        for (i, line) in lines.iter().enumerate() {
            let rec = &cbb[(i + 1) * CBB_RECORD..(i + 2) * CBB_RECORD];
            match line {
                Some(l) => {
                    let want = own_cbb(l);
                    assert!(
                        rec.iter().zip(want).all(|(r, w)| r & w == w),
                        "game {}",
                        i + 1
                    );
                    assert_eq!(rec[3] & 0x77, want[3] & 0x77, "game {}", i + 1);
                }
                None => assert!(rec.iter().all(|b| *b == 0), "record {}", i + 1),
            }
        }
        for (t, want) in targets.iter().zip(&without) {
            let got = summary(&run(&base, None, &PositionQuery::new(t.clone(), None)));
            assert_eq!(&got, want);
        }
        // Opened again, the booster is found and not built anew.
        let again = sample.open();
        again.set_booster_dir(&dir);
        assert_eq!(again.cbb_path(), Some(path.as_path()));
        // A `.cbb` that can be trusted comes first.
        let cbb_path = sample.cbh.with_extension("cbb");
        std::fs::write(&cbb_path, &cbb).expect("write");
        let with_cbb = sample.open();
        with_cbb.set_booster_dir(&dir);
        assert_eq!(with_cbb.cbb_path(), Some(cbb_path.as_path()));
    }

    /// Mega Database 2026 (not in the repository): our booster record equals
    /// ChessBase's byte for byte. Run with
    /// `cargo test --release -p chess-import mega_own_booster -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mega_own_booster() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pgnFilesforExample/Mega2026/Mega Database 2026.cbh");
        if !path.exists() {
            eprintln!("skipped: no Mega");
            return;
        }
        let base = Base::open(&path, &mut |_, _| {}, &AtomicBool::new(false)).expect("opens");
        let cbb = File::open(base.cbb_path().expect(".cbb")).expect("open");
        let cbg = File::open(base.cbg_path()).expect("open");
        let (mut buf, mut rec) = (Vec::new(), [0u8; CBB_RECORD]);
        let (mut checked, mut differ, mut unreadable, mut chess960) = (0, 0, 0, 0);
        for i in (0..base.records()).step_by(97) {
            read_exact_at(&cbb, &mut rec, ((i + 1) * CBB_RECORD) as u64).expect("read");
            if base.kind(i) == RecordKind::Game
                && be(base.record(i), 0x23, 2).is_some_and(|eco| eco >= 64576)
            {
                // Chess960: no search reads these games, their record does not matter.
                chess960 += 1;
                continue;
            }
            let ours = if base.kind(i) == RecordKind::Game
                && read_record(&cbg, base.moves_offset(i), &mut buf) > 0
            {
                match super::super::booster::record_of(&buf, 0) {
                    Some(r) => r,
                    None => {
                        // A game our decoder does not read: no search finds it, and
                        // its record does not matter.
                        let why = moves::main_line(&buf, 0, false, |_, _| true)
                            .err()
                            .map(|e| e.reason().to_owned());
                        unreadable += 1;
                        if unreadable <= 5 {
                            eprintln!("game {}: unreadable ({why:?})", i + 1);
                        }
                        continue;
                    }
                }
            } else {
                [0; CBB_RECORD]
            };
            checked += 1;
            if ours != rec {
                differ += 1;
                if differ <= 5 {
                    eprintln!("game {}: ours {ours:02x?}\n  cbb {rec:02x?}", i + 1);
                }
            }
        }
        eprintln!(
            "{checked} records checked, {differ} differ; skipped: {unreadable} unreadable, {chess960} Chess960"
        );
        assert_eq!(differ, 0);
    }

    /// Everything of an outcome that the index must reproduce.
    fn full(out: &PositionOutcome) -> (Vec<Match>, Vec<(UciMove, Tally)>, Tally) {
        (
            out.matches.clone(),
            out.moves.iter().map(|m| (m.uci, m.tally.clone())).collect(),
            out.ended.clone(),
        )
    }

    #[test]
    fn opening_index_matches_the_pass() {
        let Some(sample) = Sample::unpack("Anand 01-14.cbv") else {
            return;
        };
        let base = sample.open();
        let dir = sample.dir.join("cache");
        base.set_booster_dir(&dir);
        assert!(base.prepare_wanted());
        let progress = AtomicUsize::new(0);
        assert!(
            base.prepare(&progress, &AtomicBool::new(false))
                .expect("prepares")
        );
        assert!(progress.load(Ordering::Relaxed) >= base.records());
        let Some(index) = base.openings() else {
            eprintln!("skipped: no memory for the index");
            return;
        };
        assert!(index.positions() > 0 && !base.prepare_wanted());
        let plain = sample.open();
        assert!(plain.openings().is_none());
        let lines: Vec<_> = (0..base.records()).map(|i| oracle_line(&base, i)).collect();
        let every_third: Vec<u32> = (0..base.records() as u32).step_by(3).collect();
        let mut checked = 0;
        for line in lines.iter().flatten().step_by(23) {
            for ply in [0, 1, 3, 6, super::super::openings::PLIES] {
                let Some((pos, _)) = line.get(ply) else {
                    continue;
                };
                let moves: Vec<Move> = line[..ply]
                    .iter()
                    .scan(Chess::default(), |p, (_, m)| {
                        let m = m.as_ref()?.to_move(p).ok()?;
                        p.play_unchecked(m);
                        Some(m)
                    })
                    .collect();
                if moves.len() != ply {
                    continue;
                }
                let query = PositionQuery::new(pos.clone(), Some(moves));
                assert!(index.answers(&query, None));
                for cands in [None, Some(every_third.as_slice())] {
                    let fast = run(&base, cands, &query);
                    let slow = run(&plain, cands, &query);
                    assert_eq!(full(&fast), full(&slow), "ply {ply}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 10, "{checked} queries");
        // Deeper than the index: the pass answers.
        let line = lines
            .iter()
            .flatten()
            .find(|l| l.len() > 20)
            .expect("a game");
        let deep = PositionQuery::new(line[15].0.clone(), None);
        assert!(!index.answers(&deep, None));
        // Opened again: the index on disk is loaded without a build.
        let again = sample.open();
        again.set_booster_dir(&dir);
        let progress = AtomicUsize::new(0);
        assert!(
            again
                .prepare(&progress, &AtomicBool::new(false))
                .expect("loads")
        );
        assert_eq!(progress.load(Ordering::Relaxed), 0);
        assert!(again.openings().is_some());
    }

    /// Mega Database 2026 (not in the repository): the opening index against the
    /// pass, with timings. Run with
    /// `cargo test --release -p chess-import mega_openings -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mega_openings() {
        use super::super::base::{Filter, NameFilter, OpeningsStatus, SortKey};
        use std::time::Instant;
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pgnFilesforExample/Mega2026/Mega Database 2026.cbh");
        if !path.exists() {
            eprintln!("skipped: no Mega");
            return;
        }
        let base = Base::open(&path, &mut |_, _| {}, &AtomicBool::new(false)).expect("opens");
        let dir =
            std::env::temp_dir().join(format!("chess-import-openings-{}", std::process::id()));
        base.set_booster_dir(&dir);
        let t = Instant::now();
        let progress = AtomicUsize::new(0);
        assert!(
            base.prepare(&progress, &AtomicBool::new(false))
                .expect("prepares")
        );
        eprintln!(
            "prepared in {:.2?}: {:?}, peak memory {}",
            t.elapsed(),
            base.openings_status(),
            peak_memory()
        );
        let index = base.openings().expect("loaded");
        eprintln!(
            "{} positions, {} MB",
            index.positions(),
            index.memory() / 1_000_000
        );
        let plain = Base::open(&path, &mut |_, _| {}, &AtomicBool::new(false)).expect("opens");
        let never = AtomicBool::new(false);
        let carlsen = plain
            .query(
                &Filter {
                    player: NameFilter {
                        text: "carlsen, magnus".into(),
                        ..NameFilter::default()
                    },
                    ..Filter::default()
                },
                SortKey::Number,
                true,
                &AtomicUsize::new(0),
                &never,
            )
            .expect("query");
        for (name, line) in [
            ("start", ""),
            ("1.e4", "e4"),
            ("1.d4 Nf6 2.c4 e6", "d4 Nf6 c4 e6"),
            ("Najdorf", "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6"),
        ] {
            let (pos, moves) = san_line(line);
            let query = PositionQuery::new(pos, Some(moves));
            for cands in [None, Some(carlsen.as_slice())] {
                let t = Instant::now();
                let fast = run(&base, cands, &query);
                let fast_ms = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                let slow = run(&plain, cands, &query);
                let slow_ms = t.elapsed().as_secs_f64() * 1e3;
                let same = |o: &PositionOutcome| o.matches.iter().filter(|m| m.same_order).count();
                eprintln!(
                    "{name:18} {:8}: index {:9} games ({:9} same order) {fast_ms:8.1} ms · pass {:9} games ({:9} same order) {slow_ms:8.1} ms · ended {} / {}",
                    if cands.is_some() { "Carlsen" } else { "all" },
                    fast.matches.len(),
                    same(&fast),
                    slow.matches.len(),
                    same(&slow),
                    fast.ended.games,
                    slow.ended.games
                );
                // Both lists are in record order: one merge tells the differences.
                let (mut missing, mut extra) = (0, 0);
                let (mut i, mut j) = (0, 0);
                while i < slow.matches.len() || j < fast.matches.len() {
                    match (slow.matches.get(i), fast.matches.get(j)) {
                        (Some(a), Some(b)) if a == b => {
                            i += 1;
                            j += 1;
                        }
                        (Some(a), Some(b)) if a.index < b.index => {
                            missing += 1;
                            i += 1;
                        }
                        (Some(a), Some(b)) if a.index > b.index => {
                            extra += 1;
                            j += 1;
                        }
                        (Some(_), Some(_)) => {
                            // Same game, different details.
                            missing += 1;
                            extra += 1;
                            i += 1;
                            j += 1;
                        }
                        (Some(_), None) => {
                            missing += 1;
                            i += 1;
                        }
                        (None, _) => {
                            extra += 1;
                            j += 1;
                        }
                    }
                }
                if missing + extra > 0 {
                    eprintln!("  differ: {missing} only in the pass, {extra} only in the index");
                }
                assert!(matches!(base.openings_status(), OpeningsStatus::Loaded(_)));
            }
        }
        eprintln!("peak memory {}", peak_memory());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mega Database 2026 (not in the repository): builds our booster for the whole
    /// base into a temporary directory and times it, then searches with it. Run with
    /// `cargo test --release -p chess-import mega_build_booster -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mega_build_booster() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pgnFilesforExample/Mega2026/Mega Database 2026.cbh");
        if !path.exists() {
            eprintln!("skipped: no Mega");
            return;
        }
        let base = Base::open(&path, &mut |_, _| {}, &AtomicBool::new(false)).expect("opens");
        base.without_booster();
        let dir = std::env::temp_dir().join(format!("chess-import-booster-{}", std::process::id()));
        base.set_booster_dir(&dir);
        assert!(base.booster_wanted());
        let t = std::time::Instant::now();
        let progress = AtomicUsize::new(0);
        assert!(
            base.build_booster(&progress, &AtomicBool::new(false))
                .expect("builds")
        );
        let built = t.elapsed();
        let own = base.cbb_path().expect("ours").to_owned();
        let size = std::fs::metadata(&own).expect("file").len();
        eprintln!(
            "built in {built:.2?}: {} ({size} bytes), peak memory {}",
            own.display(),
            peak_memory()
        );
        let (pos, line) = san_line("e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6");
        let t = std::time::Instant::now();
        let out = run(&base, None, &PositionQuery::new(pos, Some(line)));
        eprintln!(
            "Najdorf with our booster: {} games in {:.1?}",
            out.matches.len(),
            t.elapsed()
        );
        assert_eq!(out.matches.len(), 291_273);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mega Database 2026 (not in the repository): the counts of the research
    /// (`docs/research/position-index.md`, section 3) and timings. Run with
    /// `cargo test --release -p chess-import mega_position_search -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mega_position_search() {
        use super::super::base::{Filter, NameFilter, SortKey};
        use std::time::Instant;
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pgnFilesforExample/Mega2026/Mega Database 2026.cbh");
        if !path.exists() {
            eprintln!("skipped: {} not found", path.display());
            return;
        }
        let t = Instant::now();
        let open = || Base::open(&path, &mut |_, _| {}, &AtomicBool::new(false)).expect("opens");
        let base = open();
        let plain = open();
        plain.without_booster();
        println!(
            "open (twice): {:.2} s, booster: {}, peak memory {}",
            t.elapsed().as_secs_f64(),
            base.cbb_path().is_some(),
            peak_memory()
        );
        assert!(base.cbb_path().is_some());
        let never = AtomicBool::new(false);
        let filter = Filter {
            player: NameFilter {
                text: "carlsen, magnus".into(),
                ..NameFilter::default()
            },
            ..Filter::default()
        };
        let carlsen = base
            .query(&filter, SortKey::Number, true, &AtomicUsize::new(0), &never)
            .expect("query");
        println!("Carlsen: {} games", carlsen.len());
        let nimzo = "d4 Nf6 c4 e6 Nc3 Bb4 e3 O-O Bd3 d5 Nf3 c5 O-O Nc6 a3 Bxc3 bxc3 dxc4 Bxc4 Qc7";
        let ruy = "e4 e5 Nf3 Nc6 Bb5 a6 Ba4 Nf6 O-O Be7 Re1 b5 Bb3 d6 c3 O-O h3 Nb8 d4 Nbd7 \
                   Nbd2 Bb7 Bc2 Re8 Nf1 Bf8 Ng3 g6 a4 c5 d5 c4 Bg5 h6 Be3 Nc5 Qd2 h5";
        // (name, line, (games reaching the position, of them in the same order)):
        // the research's linear pass plus the games that end in the position (127,
        // 13 and 3), the same order as its move-prefix index.
        let queries = [
            ("start", "", None),
            ("1.e4", "e4", None),
            (
                "Najdorf",
                "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6",
                Some((291_273, 273_875)),
            ),
            ("Nimzo-Indian, ply 20", nimzo, Some((2_208, 665))),
            ("Ruy Lopez, ply 40", ruy, Some((406, 325))),
        ];
        for (name, line, expected) in queries {
            let (pos, moves) = san_line(line);
            let query = PositionQuery::new(pos, Some(moves));
            let mut found = Vec::new();
            for (label, b, cands) in [
                ("all, .cbb", &base, None),
                ("all, no .cbb", &plain, None),
                ("Carlsen, .cbb", &base, Some(carlsen.as_slice())),
                ("Carlsen, no .cbb", &plain, Some(carlsen.as_slice())),
            ] {
                let t = Instant::now();
                let out = search(b, cands, &query, &AtomicUsize::new(0), &never).expect("runs");
                let ms = t.elapsed().as_secs_f64() * 1e3;
                let same = out.matches.iter().filter(|m| m.same_order).count();
                println!(
                    "{name:22} {label:17} {:9} games, {same:9} same order, {:3} moves, {ms:7.1} ms",
                    out.matches.len(),
                    out.moves.len()
                );
                found.push(out.matches);
            }
            println!("  peak memory {}", peak_memory());
            assert_eq!(found[0], found[1], "{name}: the booster loses nothing");
            assert_eq!(found[2], found[3], "{name}: the booster loses nothing");
            if let Some((games, same)) = expected {
                assert_eq!(found[0].len(), games, "{name}");
                assert_eq!(found[0].iter().filter(|m| m.same_order).count(), same);
            }
        }
        println!("peak memory: {}", peak_memory());
    }
}
