//! Opening chess databases that are not plain PGN files: ChessBase databases
//! (`.cbh` with its companion files), ChessBase archives (`.cbv`), and `.zip` /
//! `.7z` archives holding any of these or PGN files.
//!
//! Everything is converted to PGN text in memory and indexed as a
//! [`PgnDatabase`], so the rest of the application works the same way for every
//! format. Sources are only read.

mod archive;
pub mod cbv;
pub mod chessbase;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chess_core::{OpenError, PgnDatabase, ScanProgress, TextEncoding, detect_encoding};

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

/// An opened database with what is worth telling the user about the conversion.
pub struct Opened {
    pub db: PgnDatabase,
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
        db,
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
    Ok(Opened { db, format, notes })
}
