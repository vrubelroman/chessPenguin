//! Opening index of a large ChessBase database: the positions of the first `PLIES`
//! half-moves of every game, so a position of the opening is found without a pass
//! over the games (the booster cannot rule games out there: nearly every game passes
//! through 1.e4). Built in the same pass as the booster, kept in the cache directory
//! next to it, and held in memory only when the machine has room for it.
//!
//! Contents. Per game from the usual start position, its first `PLIES` moves as 16-bit
//! codes (`prefix`; `END` after the last move, `NULL` for a null move) — they give the
//! move a game played from a position, whether it came by the same move order and the
//! games that end there. Then one entry per (position, game) for plies 0..=`PLIES`:
//! the distinct position keys sorted (`keys`), and per key its games in ascending
//! order with the ply at which each reaches it (`games`: record index as a varint
//! delta, then the ply). A game that reaches a position twice within the opening
//! counts at its first ply. Games from a set-up position and the unreadable ones have
//! no entries (a search finds them in the pass, as before; the index answers only for
//! positions reached by at most `PLIES` moves from the usual start).
//!
//! File. `<hash>.cpo` in the cache directory (same hash as the booster): a 48-byte
//! header (magic, plies, records, section lengths), the prefixes, the keys, the
//! games, the games from a set-up position (u32 each, ascending: a search walks
//! them as before); all little-endian. It is read whole into memory ([`Index::load`]), so its
//! size is what it costs to keep.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use shakmaty::uci::UciMove;
use shakmaty::{Chess, Role, Square};

use super::base::Base;
use crate::position::{self, Collector, GameResult, Match, PositionQuery, uci};

/// Half-moves of each game in the index.
pub const PLIES: usize = 10;
/// Moves kept per game: one more than the indexed plies, so that the move played
/// from the deepest indexed position is known too.
const PREFIX: usize = PLIES + 1;
/// Rough size of the index per game, to tell beforehand whether it fits in memory
/// (Mega: about 50 bytes).
pub const BYTES_PER_GAME: u64 = 60;
/// Prefix code: the game's main line ended before this move.
const END: u16 = u16::MAX;
/// Prefix code: a null move.
const NULL: u16 = u16::MAX - 1;
const MAGIC: &[u8; 8] = b"CPOPEN02";
/// Header: magic, plies, records, then the lengths of the four sections.
const HEADER: usize = 48;
/// Temporary files the entries are spread over by the top bits of the key, so that
/// no more than a fraction of them is sorted in memory at a time.
const BUCKETS: usize = 64;
/// Bytes of a raw entry in a bucket file: key (8), game (4), ply (1).
const RAW: usize = 13;

/// A move in 16 bits: from (6), to (6), promotion role (3).
fn encode(m: UciMove) -> u16 {
    match m {
        UciMove::Normal {
            from,
            to,
            promotion,
        } => {
            u16::from(u8::from(from))
                | u16::from(u8::from(to)) << 6
                | u16::from(promotion.map_or(0, |r| r as u8)) << 12
        }
        _ => NULL,
    }
}

fn decode(v: u16) -> Option<UciMove> {
    match v {
        END => None,
        NULL => Some(UciMove::Null),
        v => Some(UciMove::Normal {
            from: Square::new(u32::from(v & 63)),
            to: Square::new(u32::from((v >> 6) & 63)),
            promotion: Role::try_from(v >> 12).ok(),
        }),
    }
}

/// What the build pass collects for one game (see [`Sink`]).
pub(super) struct GameEntry {
    pub index: u32,
    /// The first `PREFIX` moves, `END`-padded.
    pub prefix: [u16; PREFIX],
    /// (position key, ply) for plies 0..=`PLIES`, up to the end of the line.
    pub keys: Vec<(u64, u8)>,
    /// The game starts from the usual position (else it gets no entries).
    pub standard: bool,
}

impl GameEntry {
    pub(super) fn new(index: u32) -> GameEntry {
        GameEntry {
            index,
            prefix: [END; PREFIX],
            keys: Vec::with_capacity(PLIES + 1),
            standard: true,
        }
    }

    /// Called by the pass for the position at `ply` and the step from it.
    pub(super) fn visit(&mut self, pos: &Chess, ply: usize, step: &super::moves::Step<'_>) {
        use super::moves::Step;
        if !self.standard || ply > PLIES {
            return;
        }
        if ply == 0 && !position::same_position(pos, &Chess::default()) {
            self.standard = false;
            return;
        }
        self.keys.push((position::key(pos), ply as u8));
        self.prefix[ply] = match step {
            Step::Move(m) => encode(uci(m)),
            Step::Null => NULL,
            Step::End => END,
        };
    }
}

/// Collects the entries of the build pass into bucket files.
pub(super) struct Sink {
    buckets: Vec<Mutex<BufWriter<File>>>,
    prefix: Mutex<Vec<u16>>,
    /// Games from a set-up position (the index has no entries for them).
    set_up: Mutex<Vec<u32>>,
    dir: std::path::PathBuf,
}

impl Sink {
    pub(super) fn new(dir: &Path, records: usize) -> io::Result<Sink> {
        std::fs::create_dir_all(dir)?;
        let mut buckets = Vec::with_capacity(BUCKETS);
        for b in 0..BUCKETS {
            let f = File::create(dir.join(format!("bucket-{b:02}")))?;
            buckets.push(Mutex::new(BufWriter::with_capacity(1 << 16, f)));
        }
        Ok(Sink {
            buckets,
            prefix: Mutex::new(vec![END; records * PREFIX]),
            set_up: Mutex::new(Vec::new()),
            dir: dir.to_owned(),
        })
    }

    /// Adds the entries of a block of games (`entries` hold distinct games) and the
    /// games of the block that start from a set-up position.
    pub(super) fn add(&self, entries: &[GameEntry], set_up: &[u32]) -> io::Result<()> {
        if !set_up.is_empty()
            && let Ok(mut s) = self.set_up.lock()
        {
            s.extend_from_slice(set_up);
        }
        if let Ok(mut p) = self.prefix.lock() {
            for e in entries {
                let at = e.index as usize * PREFIX;
                p[at..at + PREFIX].copy_from_slice(&e.prefix);
            }
        }
        // Group by bucket so that each lock is taken once per block.
        let mut raw: Vec<Vec<u8>> = vec![Vec::new(); BUCKETS];
        for e in entries {
            for &(key, ply) in &e.keys {
                let b = &mut raw[(key >> 58) as usize];
                b.extend_from_slice(&key.to_le_bytes());
                b.extend_from_slice(&e.index.to_le_bytes());
                b.push(ply);
            }
        }
        for (b, bytes) in raw.iter().enumerate() {
            if !bytes.is_empty() {
                let mut w = self
                    .buckets
                    .get(b)
                    .and_then(|m| m.lock().ok())
                    .ok_or_else(|| io::Error::other("bucket lock"))?;
                w.write_all(bytes)?;
            }
        }
        Ok(())
    }

    /// Removes the bucket files (a cancelled or failed pass).
    pub(super) fn discard(self) -> io::Result<()> {
        self.remove_buckets();
        Ok(())
    }

    fn remove_buckets(&self) {
        for b in 0..BUCKETS {
            let _ = std::fs::remove_file(self.dir.join(format!("bucket-{b:02}")));
        }
        let _ = std::fs::remove_dir(&self.dir);
    }

    /// Sorts the entries and writes the index to `out` (`.part` first). `progress`
    /// grows by `records` over the sort. The bucket files are removed either way.
    /// `Ok(false)` when cancelled.
    pub(super) fn finish(
        self,
        out: &Path,
        records: usize,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> io::Result<bool> {
        let result = self.write(out, records, progress, cancel);
        self.remove_buckets();
        if !matches!(result, Ok(true)) {
            let _ = std::fs::remove_file(out.with_extension("part"));
            let _ = std::fs::remove_file(out.with_extension("games"));
        }
        result
    }

    fn write(
        &self,
        out: &Path,
        records: usize,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> io::Result<bool> {
        for w in &self.buckets {
            w.lock()
                .map_err(|_| io::Error::other("bucket lock"))?
                .flush()?;
        }
        let part = out.with_extension("part");
        let mut file = BufWriter::with_capacity(1 << 20, File::create(&part)?);
        // Header: magic, plies, records, then the section lengths filled in at the end.
        file.write_all(MAGIC)?;
        file.write_all(&(PLIES as u32).to_le_bytes())?;
        file.write_all(&(records as u32).to_le_bytes())?;
        file.write_all(&[0u8; HEADER - 16])?;
        // The prefix section: records × PREFIX × u16.
        let prefix = self.prefix.lock().map_err(|_| io::Error::other("lock"))?;
        let mut bytes = Vec::with_capacity(prefix.len() * 2);
        for m in prefix.iter() {
            bytes.extend_from_slice(&m.to_le_bytes());
        }
        drop(prefix);
        let prefix_len = bytes.len() as u64;
        file.write_all(&bytes)?;
        drop(bytes);
        // The keys (kept in memory: a few dozen bytes per position) and the games
        // (streamed to a file, appended after the keys), bucket by bucket: the
        // buckets are in key order.
        let games_path = out.with_extension("games");
        let mut games = BufWriter::with_capacity(
            1 << 20,
            File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&games_path)?,
        );
        let mut games_len = 0u64;
        let mut keys: Vec<u8> = Vec::new();
        let mut raw = Vec::new();
        let mut list = Vec::new();
        for b in 0..BUCKETS {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            raw.clear();
            File::open(self.dir.join(format!("bucket-{b:02}")))?.read_to_end(&mut raw)?;
            let mut entries: Vec<(u64, u32, u8)> = raw
                .as_chunks::<RAW>()
                .0
                .iter()
                .map(|e| {
                    (
                        u64::from_le_bytes(e[..8].try_into().expect("8 bytes")),
                        u32::from_le_bytes(e[8..12].try_into().expect("4 bytes")),
                        e[12],
                    )
                })
                .collect();
            entries.sort_unstable();
            let mut i = 0;
            while i < entries.len() {
                let key = entries[i].0;
                list.clear();
                let mut last = 0u32;
                let mut count = 0u32;
                while i < entries.len() && entries[i].0 == key {
                    let (_, game, ply) = entries[i];
                    i += 1;
                    // The same game twice (a repeated position): its first ply.
                    if count > 0 && game == last {
                        continue;
                    }
                    varint(&mut list, game - if count == 0 { 0 } else { last + 1 });
                    list.push(ply);
                    last = game;
                    count += 1;
                }
                keys.extend_from_slice(&key.to_le_bytes());
                keys.extend_from_slice(&games_len.to_le_bytes()[..5]);
                keys.extend_from_slice(&count.to_le_bytes());
                games.write_all(&list)?;
                games_len += list.len() as u64;
            }
            // In all exactly `records` over the buckets.
            progress.fetch_add(
                records * (b + 1) / BUCKETS - records * b / BUCKETS,
                Ordering::Relaxed,
            );
        }
        let keys_len = keys.len() as u64;
        file.write_all(&keys)?;
        drop(keys);
        let mut games = games.into_inner().map_err(|e| e.into_error())?;
        {
            use std::io::Seek;
            games.seek(io::SeekFrom::Start(0))?;
            io::copy(&mut games, &mut file)?;
        }
        drop(games);
        let _ = std::fs::remove_file(&games_path);
        // The games from a set-up position, ascending.
        let mut set_up =
            std::mem::take(&mut *self.set_up.lock().map_err(|_| io::Error::other("lock"))?);
        set_up.sort_unstable();
        for g in &set_up {
            file.write_all(&g.to_le_bytes())?;
        }
        let set_up_len = set_up.len() as u64 * 4;
        let mut file = file.into_inner().map_err(|e| e.into_error())?;
        {
            use std::io::Seek;
            file.seek(io::SeekFrom::Start(16))?;
            file.write_all(&prefix_len.to_le_bytes())?;
            file.write_all(&keys_len.to_le_bytes())?;
            file.write_all(&games_len.to_le_bytes())?;
            file.write_all(&set_up_len.to_le_bytes())?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part, out)?;
        Ok(true)
    }
}

fn varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    let mut shift = 0;
    loop {
        let b = *bytes.get(*at)?;
        *at += 1;
        v |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
}

/// Size of a key record: key (8), offset into the games (5), count (4).
const KEY: usize = 17;

/// The index in memory: the file's bytes and where its sections are.
pub struct Index {
    bytes: Vec<u8>,
    records: usize,
    prefix: std::ops::Range<usize>,
    keys: std::ops::Range<usize>,
    games: std::ops::Range<usize>,
    set_up: std::ops::Range<usize>,
}

impl Index {
    /// Size of the index file (what loading it takes in memory), if it is there.
    pub fn file_size(path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|m| m.len())
    }

    /// Reads the whole file; `None` when it is not an index for `records` records.
    pub fn load(path: &Path, records: usize) -> Option<Index> {
        let bytes = std::fs::read(path).ok()?;
        let head = bytes.get(..HEADER)?;
        if &head[..8] != MAGIC {
            return None;
        }
        let u32_at = |at: usize| -> Option<u64> {
            Some(u64::from(u32::from_le_bytes(
                head[at..at + 4].try_into().ok()?,
            )))
        };
        let u64_at = |at: usize| -> Option<usize> {
            Some(u64::from_le_bytes(head[at..at + 8].try_into().ok()?) as usize)
        };
        if u32_at(8) != Some(PLIES as u64) || u32_at(12) != Some(records as u64) {
            return None;
        }
        let (p, k, g, u) = (u64_at(16)?, u64_at(24)?, u64_at(32)?, u64_at(40)?);
        let prefix = HEADER..HEADER + p;
        let keys = prefix.end..prefix.end + k;
        let games = keys.end..keys.end + g;
        let set_up = games.end..games.end + u;
        if set_up.end != bytes.len() || p != records * PREFIX * 2 || k % KEY != 0 || u % 4 != 0 {
            return None;
        }
        Some(Index {
            bytes,
            records,
            prefix,
            keys,
            games,
            set_up,
        })
    }

    pub fn memory(&self) -> usize {
        self.bytes.len()
    }

    pub fn positions(&self) -> usize {
        self.keys.len() / KEY
    }

    fn key_at(&self, i: usize) -> u64 {
        let at = self.keys.start + i * KEY;
        u64::from_le_bytes(self.bytes[at..at + 8].try_into().expect("8 bytes"))
    }

    /// Where `key` is among the keys, if it is.
    fn find(&self, key: u64) -> Option<usize> {
        let n = self.positions();
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.key_at(mid) < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (lo < n && self.key_at(lo) == key).then_some(lo)
    }

    /// Number of games that reach the position with `key` within the opening.
    pub fn count(&self, key: u64) -> usize {
        self.find(key).map_or(0, |i| {
            let at = self.keys.start + i * KEY + 13;
            u32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("4 bytes")) as usize
        })
    }

    /// The games of the position with `key`: (record index, ply), ascending.
    fn games_of(&self, key: u64) -> Vec<(u32, u8)> {
        let Some(lo) = self.find(key) else {
            return Vec::new();
        };
        let at = self.keys.start + lo * KEY + 8;
        let mut off = [0u8; 8];
        off[..5].copy_from_slice(&self.bytes[at..at + 5]);
        let offset = u64::from_le_bytes(off) as usize;
        let count = u32::from_le_bytes(self.bytes[at + 5..at + 9].try_into().expect("4 bytes"));
        let games = &self.bytes[self.games.clone()];
        let mut out = Vec::with_capacity(count as usize);
        let mut pos = offset;
        let mut last = 0u32;
        for i in 0..count {
            let Some(delta) = read_varint(games, &mut pos) else {
                break;
            };
            let game = if i == 0 { delta } else { last + 1 + delta };
            let Some(&ply) = games.get(pos) else {
                break;
            };
            pos += 1;
            out.push((game, ply));
            last = game;
        }
        out
    }

    /// The first `PREFIX` moves of a game (`END`-padded).
    fn prefix_of(&self, game: u32) -> [u16; PREFIX] {
        let at = self.prefix.start + game as usize * PREFIX * 2;
        let mut out = [END; PREFIX];
        for (i, m) in out.iter_mut().enumerate() {
            let b = at + i * 2;
            *m = u16::from_le_bytes([self.bytes[b], self.bytes[b + 1]]);
        }
        out
    }

    /// Whether the index should answer `query`: a position reached from the usual
    /// start by at most `PLIES` moves (then every game that reaches it within the
    /// opening is in the index), unless `candidates` are so few against the games of
    /// the position that walking them is cheaper than decoding its list.
    pub fn answers(&self, query: &PositionQuery, candidates: Option<&[u32]>) -> bool {
        let opening = query
            .line
            .as_ref()
            .is_some_and(|l| l.len() <= PLIES && self.records > 0);
        opening && candidates.is_none_or(|c| c.len() * 32 >= self.count(query.key()))
    }

    /// Records `games` (record index, ply) that reach the position: what each played
    /// next and its result, ratings and year.
    fn tally(&self, base: &Base, games: &[(u32, u8)], line: &[u16]) -> Collector {
        let mut out = Collector::default();
        for &(game, ply) in games {
            let prefix = self.prefix_of(game);
            let ply = usize::from(ply);
            let same_order = ply == line.len() && prefix[..ply] == line[..];
            let next = decode(prefix[ply]);
            let r = base.record(game as usize);
            let u = |at, n| super::be(r, at, n).unwrap_or(0);
            let result = match r.get(0x1b) {
                Some(2 | 6) => GameResult::WhiteWins,
                Some(1 | 5) => GameResult::Draw,
                Some(0 | 4) => GameResult::BlackWins,
                _ => GameResult::Unknown,
            };
            let year = ((u(0x18, 3) >> 9) & 0xfff) as u16;
            out.add(
                Match {
                    index: game,
                    ply: ply as u32,
                    same_order,
                    in_variation: false,
                    next: None,
                },
                next,
                result,
                u(0x1f, 2) as u16,
                u(0x21, 2) as u16,
                year,
            );
        }
        out
    }

    /// The games (among `candidates`, ascending; `None`: all) that reach the position,
    /// with what they played next, as the pass would find them.
    pub fn collect(
        &self,
        base: &Base,
        candidates: Option<&[u32]>,
        query: &PositionQuery,
    ) -> Collector {
        let line: Vec<u16> = query
            .line
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|m| encode(uci(m)))
            .collect();
        // The games asked for, in record order.
        let mut games = self.games_of(query.key());
        if let Some(c) = candidates {
            let mut i = 0;
            games.retain(|&(g, _)| {
                while i < c.len() && c[i] < g {
                    i += 1;
                }
                i < c.len() && c[i] == g
            });
        }
        // Millions of games for an early position: tallied in parallel, each part
        // keeping its order.
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(games.len().div_ceil(1 << 16))
            .max(1);
        let chunk = games.len().div_ceil(threads).max(1);
        let line = &line;
        let parts: Vec<Collector> = std::thread::scope(|s| {
            let handles: Vec<_> = games
                .chunks(chunk)
                .map(|part| s.spawn(move || self.tally(base, part, line)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        let mut out = Collector::default();
        for part in parts {
            out.merge(part);
        }
        out
    }

    /// The games from a set-up position (not in the index) among `candidates`.
    pub fn set_up_games(&self, candidates: Option<&[u32]>) -> Vec<u32> {
        let all = self
            .set_up
            .clone()
            .step_by(4)
            .map(|at| u32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("4 bytes")));
        match candidates {
            None => all.collect(),
            Some(c) => {
                let mut i = 0;
                all.filter(|&g| {
                    while i < c.len() && c[i] < g {
                        i += 1;
                    }
                    i < c.len() && c[i] == g
                })
                .collect()
            }
        }
    }
}
