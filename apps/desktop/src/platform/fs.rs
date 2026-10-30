//! Files of the program and the files the user opened: the disk on a computer; in a
//! browser the page's files ([`super::web::vfs`]): the program's data kept in the
//! browser's storage, the user's files in memory for the session.

use std::io;
use std::path::Path;

/// What is known of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub len: u64,
    /// Last change, ms since the Unix epoch (0: not known).
    pub modified_ms: u64,
    pub readonly: bool,
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(not(web))]
    {
        std::fs::read(path)
    }
    #[cfg(web)]
    {
        super::web::vfs::read(path)
    }
}

pub fn read_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The first `len` bytes of the file (fewer when it is shorter).
pub fn read_head(path: &Path, len: usize) -> io::Result<Vec<u8>> {
    #[cfg(not(web))]
    {
        use std::io::Read;
        let mut head = Vec::with_capacity(len);
        std::fs::File::open(path)?
            .take(len as u64)
            .read_to_end(&mut head)?;
        Ok(head)
    }
    #[cfg(web)]
    {
        let mut all = super::web::vfs::read(path)?;
        all.truncate(len);
        Ok(all)
    }
}

/// Writes the file whole: a reader sees the old text or the new one, never a part
/// (on a computer through a temporary file and a rename; its folder is made).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(not(web))]
    {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
    #[cfg(web)]
    {
        super::web::vfs::write(path, bytes.to_vec())
    }
}

pub fn exists(path: &Path) -> bool {
    meta(path).is_ok()
}

pub fn meta(path: &Path) -> io::Result<Meta> {
    #[cfg(not(web))]
    {
        let m = std::fs::metadata(path)?;
        let modified_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Ok(Meta {
            len: m.len(),
            modified_ms,
            readonly: m.permissions().readonly(),
        })
    }
    #[cfg(web)]
    {
        super::web::vfs::meta(path)
    }
}

pub fn remove_file(path: &Path) -> io::Result<()> {
    #[cfg(not(web))]
    {
        std::fs::remove_file(path)
    }
    #[cfg(web)]
    {
        super::web::vfs::remove(path)
    }
}
