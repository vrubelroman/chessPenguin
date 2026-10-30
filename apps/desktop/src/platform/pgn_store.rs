//! Writing games into a PGN database the library has open: its file on a computer
//! (`chess_core::pgn::store`, as before); in a browser the page's file of that path
//! (only the program's own: «My games»; the user's files are only read). The text
//! written is the same byte for byte.

use std::path::Path;

use chess_core::PgnDatabase;
use chess_core::pgn::store::{self, Appended, Removed, WriteError};

#[cfg(web)]
fn write(path: &Path, bytes: Vec<u8>) -> Result<(), WriteError> {
    super::web::vfs::write(path, bytes).map_err(WriteError::Io)
}

#[cfg(web)]
fn utf8(db: &PgnDatabase) -> Result<(), WriteError> {
    if db.encoding() != chess_core::TextEncoding::Utf8 && !db.detected_encoding().ascii_only {
        return Err(WriteError::Encoding);
    }
    Ok(())
}

/// Adds `record` at the end of `db` (the file `path`).
pub fn append(db: &PgnDatabase, path: &Path, record: &str) -> Result<Appended, WriteError> {
    #[cfg(not(web))]
    {
        let _ = path;
        store::append(db, record)
    }
    #[cfg(web)]
    {
        utf8(db)?;
        let (bytes, offset) = store::appended(db, record);
        write(path, bytes)?;
        Ok(Appended { offset })
    }
}

/// Replaces record `index` of `db` with `record`.
pub fn replace(
    db: &PgnDatabase,
    path: &Path,
    index: usize,
    record: &str,
) -> Result<(), WriteError> {
    #[cfg(not(web))]
    {
        let _ = path;
        store::replace(db, index, record)
    }
    #[cfg(web)]
    {
        utf8(db)?;
        write(path, store::replaced(db, index, record)?)
    }
}

/// Removes record `index` of `db`; [`restore`] puts it back.
pub fn remove(db: &PgnDatabase, path: &Path, index: usize) -> Result<Removed, WriteError> {
    #[cfg(not(web))]
    {
        let _ = path;
        store::remove(db, index)
    }
    #[cfg(web)]
    {
        utf8(db)?;
        let (bytes, gone) = store::removed(db, index)?;
        write(path, bytes)?;
        Ok(gone)
    }
}

/// Puts a removed record back at its place.
pub fn restore(db: &PgnDatabase, path: &Path, removed: &Removed) -> Result<(), WriteError> {
    #[cfg(not(web))]
    {
        let _ = path;
        store::restore(db, removed)
    }
    #[cfg(web)]
    {
        write(path, store::restored(db, removed)?)
    }
}
