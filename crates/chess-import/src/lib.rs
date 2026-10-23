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
pub mod pgn_catalog;
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
    /// The filters' index of the tags, built on first use.
    catalog: std::sync::OnceLock<Box<pgn_catalog::Catalog>>,
}

impl PgnDb {
    pub fn new(db: PgnDatabase) -> PgnDb {
        PgnDb {
            db,
            positions: Mutex::new(None),
            catalog: std::sync::OnceLock::new(),
        }
    }

    /// The filters' index of the games' tags (see [`pgn_catalog`]).
    pub fn catalog(&self) -> &pgn_catalog::Catalog {
        self.catalog
            .get_or_init(|| Box::new(pgn_catalog::Catalog::build(self.db.entries())))
    }

    /// Memory the position table would need while it is built: about 20 bytes per
    /// position, and two to three times that during the build (`docs/dev/import.md`).
    /// Counted from the moves of the games (variations included): the size of the text
    /// says little when every move carries a clock comment.
    pub fn positions_estimate(&self) -> u64 {
        let positions: u64 = self
            .db
            .entries()
            .iter()
            .map(|e| u64::from(e.all_plies) + 1)
            .sum();
        positions * 20 * 3
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

    /// Games (not guiding texts).
    pub fn games(&self) -> usize {
        match self {
            Database::Pgn(db) => db.len() - db.catalog().texts(),
            Database::ChessBase(b) => b.games(),
        }
    }

    /// Guiding texts (of a ChessBase database, or of one converted to PGN).
    pub fn texts(&self) -> usize {
        match self {
            Database::Pgn(db) => db.catalog().texts(),
            Database::ChessBase(b) => b.texts(),
        }
    }

    /// The table row of record `index`.
    pub fn row(&self, index: usize) -> chessbase::base::Row {
        match self {
            Database::Pgn(db) => db
                .entries()
                .get(index)
                .map(|e| pgn_catalog::Catalog::row(e, e.moves()))
                .unwrap_or_default(),
            Database::ChessBase(b) => b.row(index),
        }
    }

    /// Records that pass `filter`, sorted; `None` when cancelled.
    pub fn query(
        &self,
        filter: &chessbase::base::Filter,
        sort: chessbase::base::SortKey,
        ascending: bool,
        scanned: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<Vec<u32>> {
        match self {
            Database::Pgn(db) => {
                db.catalog()
                    .query(db.entries(), filter, sort, ascending, scanned, cancel)
            }
            Database::ChessBase(b) => b.query(filter, sort, ascending, scanned, cancel),
        }
    }

    pub fn sort_records(
        &self,
        records: &mut Vec<u32>,
        sort: chessbase::base::SortKey,
        ascending: bool,
    ) {
        match self {
            Database::Pgn(db) => db.catalog().sort_records(records, sort, ascending),
            Database::ChessBase(b) => b.sort_records(records, sort, ascending),
        }
    }

    pub fn suggest_players(
        &self,
        query: &str,
        limit: usize,
    ) -> (Vec<chessbase::base::Suggestion>, usize) {
        match self {
            Database::Pgn(db) => db.catalog().suggest_players(query, limit),
            Database::ChessBase(b) => b.suggest_players(query, limit),
        }
    }

    pub fn suggest_events(
        &self,
        query: &str,
        limit: usize,
    ) -> (Vec<chessbase::base::Suggestion>, usize) {
        match self {
            Database::Pgn(db) => db.catalog().suggest_events(query, limit),
            Database::ChessBase(b) => b.suggest_events(query, limit),
        }
    }

    /// Most games have no result: a book of chapters (continuations count chapters).
    pub fn books(&self) -> bool {
        match self {
            Database::Pgn(db) => db.catalog().books(),
            Database::ChessBase(_) => false,
        }
    }

    /// The record could not be read cleanly (structure or encoding problems).
    pub fn damaged(&self, index: usize) -> bool {
        match self {
            Database::Pgn(db) => db
                .entries()
                .get(index)
                .is_some_and(|e| e.flags.has_structure_issues || e.flags.has_decode_errors),
            Database::ChessBase(_) => false,
        }
    }

    /// The position search is ready (a PGN database builds its table on first use).
    pub fn positions_ready(&self) -> bool {
        match self {
            Database::Pgn(db) => db.positions.lock().is_ok_and(|t| t.is_some()),
            Database::ChessBase(_) => true,
        }
    }

    /// Which fields at least one game has (ChessBase databases: all of them).
    pub fn filled(&self) -> pgn_catalog::Filled {
        match self {
            Database::Pgn(db) => db.catalog().filled(),
            Database::ChessBase(_) => pgn_catalog::Filled {
                eco: true,
                annotator: true,
                elo: true,
                year: true,
            },
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

/// Opens a database given as its content, without the file system (Android hands
/// over file descriptors, not paths): PGN, `.zip`, `.7z` and `.cbv`; `name` is the file
/// name, for its extension. A ChessBase database is several files: see
/// [`open_chessbase_files`].
pub fn open_bytes(
    name: &str,
    bytes: Vec<u8>,
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> Result<Opened, ImportError> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "cbz" => return Err(ImportError::Unsupported(Unsupported::Encrypted)),
        "2cbh" => return Err(ImportError::Unsupported(Unsupported::ChessBase2)),
        "cbh" => {
            return Err(ImportError::Corrupt(
                "a ChessBase database is several files: open its folder".into(),
            ));
        }
        _ => {}
    }
    if let Some(kind) = archive::sniff(&bytes) {
        let format = match kind {
            archive::Kind::Zip => Format::Zip,
            archive::Kind::SevenZip => Format::SevenZip,
        };
        let (pgn, notes) = open_archive(&bytes, kind, cancel)?;
        return finish(pgn, format, notes, progress, cancel);
    }
    if ext == "cbv" || cbv::looks_like_cbv(&bytes) && ext != "pgn" {
        let part = open_cbv(&bytes)?;
        return finish(
            part.pgn,
            Format::ChessBaseArchive,
            part.notes,
            progress,
            cancel,
        );
    }
    let db = PgnDatabase::from_bytes(bytes, None, progress, cancel)?;
    Ok(Opened {
        db: Database::Pgn(PgnDb::new(db)),
        format: Format::Pgn,
        notes: Vec::new(),
    })
}

/// Opens a ChessBase database from the contents of its files (`<name>.cbh`, `.cbg`,
/// `.cba`, …: `files` are (file name, content)), converted into PGN in memory like the
/// databases inside archives. Meant for databases of a phone's size, not Mega.
pub fn open_chessbase_files(
    cbh_name: &str,
    files: &[(String, Vec<u8>)],
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> Result<Opened, ImportError> {
    let members = chessbase::Files::from_members(
        cbh_name,
        files.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
    );
    let part = convert_chessbase(&members)?;
    finish(part.pgn, Format::ChessBase, part.notes, progress, cancel)
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
