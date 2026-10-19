//! Opening chess databases that are not plain PGN files: ChessBase databases
//! (`.cbh` with its companion files), ChessBase archives (`.cbv`), and `.zip` /
//! `.7z` archives holding any of these or PGN files.
//!
//! Small databases are converted to PGN text in memory and indexed as a
//! [`PgnDatabase`]. A large ChessBase database (Mega Database: millions of games) is
//! read in place as a [`chessbase::base::Base`]: its game list comes from the `.cbh`
//! records and a game is converted to PGN when it is opened. Sources are only read.

mod archive;
pub mod bench;
pub mod cbv;
pub mod chessbase;
pub mod memory;
pub mod pgn_positions;
pub mod position;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use chess_core::{
    OpenError, ParsedGame, PgnDatabase, ScanProgress, TextEncoding, detect_encoding, parse_game,
};

use chessbase::base::Base;

/// File extensions the application can open (lower case).
pub const EXTENSIONS: [&str; 5] = ["pgn", "cbh", "cbv", "zip", "7z"];

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Corrupt(String),
    #[error("{0}")]
    Unsupported(Unsupported),
    #[error("no PGN or ChessBase database in the archive")]
    Empty,
    #[error("import cancelled")]
    Cancelled,
}

impl From<OpenError> for ImportError {
    fn from(e: OpenError) -> Self {
        match e {
            OpenError::Io { path, source } => ImportError::Io { path, source },
            OpenError::Cancelled => ImportError::Cancelled,
        }
    }
}

/// Sources that are recognized but cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unsupported {
    #[error("password-protected archives (.cbz, encrypted zip/7z) are not supported")]
    Encrypted,
    #[error("the ChessBase 2CBH format is not supported; export the database as CBH or PGN")]
    ChessBase2,
}

/// What a database was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pgn,
    ChessBase,
    ChessBaseArchive,
    Zip,
    SevenZip,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Pgn => "PGN",
            Format::ChessBase => "ChessBase",
            Format::ChessBaseArchive => "ChessBase CBV",
            Format::Zip => "ZIP",
            Format::SevenZip => "7z",
        }
    }
}

/// ChessBase databases with at least this many records are read in place
/// ([`Database::ChessBase`]); smaller ones are converted to PGN.
pub const LARGE_CHESSBASE: u64 = 20_000;

/// A PGN database and, once a position search asked for it, the positions of its
/// games.
pub struct PgnDb {
    pub db: PgnDatabase,
    positions: Mutex<Option<Arc<pgn_positions::PositionTable>>>,
}

impl PgnDb {
    pub fn new(db: PgnDatabase) -> PgnDb {
        PgnDb {
            db,
            positions: Mutex::new(None),
        }
    }

    /// Memory the position table would need, built: about 20 bytes per position
    /// and, while it is built, two to three times that (`docs/dev/import.md`),
    /// estimated from the size of the PGN text (a move is some six bytes of it).
    pub fn positions_estimate(&self) -> u64 {
        self.db.byte_len() as u64 * 10
    }

    /// Why the position table cannot be built now: the memory it would need
    /// ([`PgnDb::positions_estimate`]) is not there. `None` once it is built.
    pub fn positions_blocked(&self) -> Option<u64> {
        if self.positions.lock().ok()?.is_some() {
            return None;
        }
        let need = self.positions_estimate();
        let room = memory::available().is_some_and(|free| free >= need + (256 << 20));
        (!room).then_some(need)
    }

    /// The position table, built on first use. `None` when cancelled, or when the
    /// machine has no memory for it ([`PgnDb::positions_blocked`]).
    pub(crate) fn positions(
        &self,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<Arc<pgn_positions::PositionTable>> {
        if let Some(t) = self.positions.lock().ok()?.as_ref() {
            return Some(t.clone());
        }
        if self.positions_blocked().is_some() {
            return None;
        }
        let table = Arc::new(pgn_positions::PositionTable::build(
            &self.db, progress, cancel,
        )?);
        if let Ok(mut slot) = self.positions.lock() {
            *slot = Some(table.clone());
        }
        Some(table)
    }
}

impl std::ops::Deref for PgnDb {
    type Target = PgnDatabase;

    fn deref(&self) -> &PgnDatabase {
        &self.db
    }
}

/// An opened database.
pub enum Database {
    Pgn(PgnDb),
    ChessBase(Box<Base>),
}

impl Database {
    /// Number of records; an index below it identifies a game (or a deleted record
    /// of a ChessBase database).
    pub fn len(&self) -> usize {
        match self {
            Database::Pgn(db) => db.len(),
            Database::ChessBase(b) => b.records(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fully parses record `index`. A ChessBase game that could only be read in part
    /// carries the problem as a diagnostic.
    pub fn load_game(&self, index: usize) -> Option<ParsedGame> {
        match self {
            Database::Pgn(db) => db.load_game(index),
            Database::ChessBase(b) => {
                let (pgn, problem) = b.pgn(index)?;
                let mut parsed = parse_game(pgn.as_bytes(), TextEncoding::Utf8);
                if let Some(p) = problem {
                    parsed.diagnostics.push(chess_core::Diagnostic {
                        severity: chess_core::Severity::Warning,
                        code: chess_core::DiagnosticCode::PartlyConverted,
                        message: format!("ChessBase: {p}"),
                        location: None,
                        node: None,
                    });
                }
                Some(parsed)
            }
        }
    }

    /// Bytes that identify record `index` (its content for PGN, its `.cbh` record for
    /// ChessBase), for remembering viewed games.
    pub fn identity(&self, index: usize) -> &[u8] {
        match self {
            Database::Pgn(db) => db.raw_bytes(index).unwrap_or_default(),
            Database::ChessBase(b) => b.record(index),
        }
    }

    /// Record `index` as PGN text (the original text of a PGN file).
    pub fn raw_text(&self, index: usize) -> Option<String> {
        match self {
            Database::Pgn(db) => db.raw_text(index),
            Database::ChessBase(b) => b.pgn(index).map(|(pgn, _)| pgn),
        }
    }

    pub fn as_pgn(&self) -> Option<&PgnDatabase> {
        match self {
            Database::Pgn(db) => Some(&db.db),
            Database::ChessBase(_) => None,
        }
    }

    /// Games among `candidates` (record indices in ascending order; `None`: all)
    /// that reach the position, with what was played next. `progress` counts the
    /// records looked at (or, the first time for a PGN database, the games prepared).
    /// `None` when cancelled.
    pub fn search_position(
        &self,
        candidates: Option<&[u32]>,
        query: &position::PositionQuery,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<position::PositionOutcome> {
        match self {
            Database::Pgn(db) => {
                let table = db.positions(progress, cancel)?;
                table.search(&db.db, candidates, query, cancel)
            }
            Database::ChessBase(b) => {
                chessbase::search::search(b, candidates, query, progress, cancel)
            }
        }
    }

    /// Memory a position search would need first and does not have (a PGN
    /// database's table, see [`PgnDb::positions_blocked`]).
    pub fn positions_blocked(&self) -> Option<u64> {
        match self {
            Database::Pgn(db) => db.positions_blocked(),
            Database::ChessBase(_) => None,
        }
    }

    pub fn as_chessbase(&self) -> Option<&Base> {
        match self {
            Database::ChessBase(b) => Some(b),
            Database::Pgn(_) => None,
        }
    }
}

/// Number of records of the ChessBase database whose `.cbh` file is `path`, from the
/// file size (each record and the header take 46 bytes).
pub fn chessbase_records(path: &Path) -> Option<u64> {
    let len = std::fs::metadata(path).ok()?.len();
    Some((len / 46).saturating_sub(1))
}

/// An opened database with what is worth telling the user about the conversion.
pub struct Opened {
    pub db: Database,
    pub format: Format,
    /// Problems and remarks, one line each (empty for a clean import).
    pub notes: Vec<String>,
}

/// True if the file's extension is one [`open`] handles.
pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// Opens `path`. Plain PGN files are read as before; other formats are recognized
/// by extension and by content.
pub fn open(
    path: &Path,
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> Result<Opened, ImportError> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let io_err = |source| ImportError::Io {
        path: path.to_owned(),
        source,
    };
    match ext.as_str() {
        "cbz" => return Err(ImportError::Unsupported(Unsupported::Encrypted)),
        "2cbh" => return Err(ImportError::Unsupported(Unsupported::ChessBase2)),
        "cbh" if chessbase_records(path).is_some_and(|n| n >= LARGE_CHESSBASE) => {
            let base = Base::open(
                path,
                &mut |done, total| {
                    progress(ScanProgress {
                        bytes_done: done as usize,
                        bytes_total: total as usize,
                        games: 0,
                    })
                },
                cancel,
            )?;
            let notes = base.notes().to_vec();
            return Ok(Opened {
                db: Database::ChessBase(Box::new(base)),
                format: Format::ChessBase,
                notes,
            });
        }
        "cbh" => {
            let files = chessbase::Files::from_dir(path)?;
            let part = convert_chessbase(&files)?;
            return finish(part.pgn, Format::ChessBase, part.notes, progress, cancel);
        }
        _ => {}
    }
    // Content decides for archives (whatever their extension); anything else is PGN.
    let mut head = [0u8; 8];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path).map_err(io_err)?;
        let n = f.read(&mut head).map_err(io_err)?;
        if n < head.len() {
            head[n..].fill(0);
        }
    }
    if let Some(kind) = archive::sniff(&head) {
        let bytes = std::fs::read(path).map_err(io_err)?;
        let format = match kind {
            archive::Kind::Zip => Format::Zip,
            archive::Kind::SevenZip => Format::SevenZip,
        };
        let (pgn, notes) = open_archive(&bytes, kind, cancel)?;
        return finish(pgn, format, notes, progress, cancel);
    }
    if ext == "cbv" || cbv::looks_like_cbv(&head) && ext != "pgn" {
        let bytes = std::fs::read(path).map_err(io_err)?;
        let part = open_cbv(&bytes)?;
        return finish(
            part.pgn,
            Format::ChessBaseArchive,
            part.notes,
            progress,
            cancel,
        );
    }
    let db = PgnDatabase::open(path, progress, cancel)?;
    Ok(Opened {
        db: Database::Pgn(PgnDb::new(db)),
        format: Format::Pgn,
        notes: Vec::new(),
    })
}

/// A converted part of a database: UTF-8 PGN and notes.
struct Part {
    pgn: String,
    notes: Vec<String>,
}

fn convert_chessbase(files: &chessbase::Files) -> Result<Part, ImportError> {
    let c = chessbase::convert(files)?;
    Ok(Part {
        pgn: c.pgn,
        notes: c.notes,
    })
}

fn open_cbv(bytes: &[u8]) -> Result<Part, ImportError> {
    let members = cbv::unpack(bytes)?;
    let cbh: Vec<&str> = members
        .iter()
        .map(|m| m.name.as_str())
        .filter(|n| n.to_lowercase().ends_with(".cbh"))
        .collect();
    let Some(first) = cbh.first() else {
        return Err(ImportError::Corrupt(
            "the ChessBase archive holds no database (.cbh)".into(),
        ));
    };
    let files = chessbase::Files::from_members(
        first,
        members.iter().map(|m| (m.name.as_str(), m.data.as_slice())),
    );
    convert_chessbase(&files)
}

/// Every database in an archive: PGN files, ChessBase databases and ChessBase
/// archives, in the order they are stored. A database whose games are exactly those
/// of one already taken (an archive often holds a ChessBase database both unpacked
/// and as `.cbv`) is left out.
fn open_archive(
    bytes: &[u8],
    kind: archive::Kind,
    cancel: &AtomicBool,
) -> Result<(String, Vec<String>), ImportError> {
    let wanted = |name: &str| {
        let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        ext == "pgn" || ext == "cbv" || chessbase_member(ext)
    };
    let entries = archive::read(bytes, kind, &wanted)?;
    let mut parts: Vec<(String, Part)> = Vec::new();
    let mut skipped = Vec::new();
    for e in &entries {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
        let lower = e.name.to_lowercase();
        let part = if lower.ends_with(".pgn") {
            Ok(Part {
                pgn: decode_pgn(&e.data),
                notes: Vec::new(),
            })
        } else if lower.ends_with(".cbv") {
            open_cbv(&e.data)
        } else if lower.ends_with(".cbh") {
            let files = chessbase::Files::from_members(
                &e.name,
                entries.iter().map(|m| (m.name.as_str(), m.data.as_slice())),
            );
            convert_chessbase(&files)
        } else {
            continue;
        };
        match part {
            Ok(p) if parts.iter().any(|(_, q)| q.pgn == p.pgn) => skipped.push(e.name.clone()),
            Ok(p) => parts.push((e.name.clone(), p)),
            Err(err) => parts.push((
                e.name.clone(),
                Part {
                    pgn: String::new(),
                    notes: vec![err.to_string()],
                },
            )),
        }
    }
    if parts.iter().all(|(_, p)| p.pgn.is_empty()) {
        if let Some((_, p)) = parts.into_iter().find(|(_, p)| !p.notes.is_empty()) {
            return Err(ImportError::Corrupt(p.notes.join("; ")));
        }
        return Err(ImportError::Empty);
    }
    let several = parts.len() > 1;
    let mut pgn = String::new();
    let mut notes = Vec::new();
    for (name, p) in parts {
        for n in p.notes {
            notes.push(if several { format!("{name}: {n}") } else { n });
        }
        if !p.pgn.is_empty() {
            pgn.push_str(&p.pgn);
            if !pgn.ends_with("\n\n") {
                pgn.push_str("\n\n");
            }
        }
    }
    for s in skipped {
        notes.push(format!(
            "{s}: the same games as another database in the archive, skipped"
        ));
    }
    Ok((pgn, notes))
}

fn chessbase_member(ext: &str) -> bool {
    matches!(ext, "cbh" | "cbg" | "cba" | "cbp" | "cbt" | "cbc")
}

/// PGN from an archive member, as UTF-8 (its own encoding detected).
fn decode_pgn(bytes: &[u8]) -> String {
    let enc = detect_encoding(bytes).encoding;
    enc.decode(bytes).0.into_owned()
}

fn finish(
    pgn: String,
    format: Format,
    notes: Vec<String>,
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> Result<Opened, ImportError> {
    let db = PgnDatabase::from_bytes(pgn.into_bytes(), Some(TextEncoding::Utf8), progress, cancel)?;
    Ok(Opened {
        db: Database::Pgn(PgnDb::new(db)),
        format,
        notes,
    })
}
