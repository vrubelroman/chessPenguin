//! `.zip` and `.7z` archives, read into memory.

use std::io::{Cursor, Read};

use crate::ImportError;

/// A file of an archive (directories are left out).
pub struct Entry {
    /// Path inside the archive with `/` separators.
    pub name: String,
    pub data: Vec<u8>,
}

/// Total size an archive may unpack to: protection against archive bombs.
const MAX_TOTAL: u64 = 2 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZip,
}

pub fn sniff(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        Some(Kind::Zip)
    } else if bytes.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        Some(Kind::SevenZip)
    } else {
        None
    }
}

/// Reads the members whose name `wanted` accepts (by lower-case name).
pub fn read(
    bytes: &[u8],
    kind: Kind,
    wanted: &dyn Fn(&str) -> bool,
) -> Result<Vec<Entry>, ImportError> {
    match kind {
        Kind::Zip => read_zip(bytes, wanted),
        Kind::SevenZip => read_7z(bytes, wanted),
    }
}

fn too_large() -> ImportError {
    ImportError::Corrupt("the archive unpacks to more than 2 GB".into())
}

fn read_zip(bytes: &[u8], wanted: &dyn Fn(&str) -> bool) -> Result<Vec<Entry>, ImportError> {
    let bad = |e: zip::result::ZipError| ImportError::Corrupt(format!("zip archive: {e}"));
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(bad)?;
    let mut out = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(bad)?;
        let name = f.name().map_err(bad)?.replace('\\', "/");
        if f.is_dir() || !wanted(&name.to_lowercase()) {
            continue;
        }
        if f.encrypted() {
            return Err(ImportError::Unsupported(crate::Unsupported::Encrypted));
        }
        total += f.size();
        if total > MAX_TOTAL {
            return Err(too_large());
        }
        let mut data = Vec::with_capacity(f.size().min(MAX_TOTAL) as usize);
        f.by_ref()
            .take(MAX_TOTAL)
            .read_to_end(&mut data)
            .map_err(|e| ImportError::Corrupt(format!("zip archive, {name}: {e}")))?;
        out.push(Entry { name, data });
    }
    Ok(out)
}

fn read_7z(bytes: &[u8], wanted: &dyn Fn(&str) -> bool) -> Result<Vec<Entry>, ImportError> {
    let bad = |e: sevenz_rust2::Error| match e {
        sevenz_rust2::Error::PasswordRequired => {
            ImportError::Unsupported(crate::Unsupported::Encrypted)
        }
        e => ImportError::Corrupt(format!("7z archive: {e}")),
    };
    let mut reader =
        sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty())
            .map_err(bad)?;
    let declared: u64 = reader.archive().files.iter().map(|f| f.size()).sum();
    if declared > MAX_TOTAL {
        return Err(too_large());
    }
    let mut out = Vec::new();
    reader
        .for_each_entries(|entry, r| {
            let name = entry.name().replace('\\', "/");
            // Solid archives decode in order: entries that are not wanted are still read.
            let mut data = Vec::new();
            r.take(MAX_TOTAL).read_to_end(&mut data)?;
            if !entry.is_directory() && wanted(&name.to_lowercase()) {
                out.push(Entry { name, data });
            }
            Ok(true)
        })
        .map_err(bad)?;
    Ok(out)
}
