//! The page's file tree: files by path, in memory. The program's data (`/config`,
//! `/data`, `/cache`) is written through to the browser's storage ([`super::idb`]) and
//! read from it at the start ([`load`]); the rest lives for the session.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::platform::fs::Meta;

pub const CONFIG: &str = "/config";
pub const DATA: &str = "/data";
pub const CACHE: &str = "/cache";
pub const EXAMPLES: &str = "/examples";
pub const OPENED: &str = "/opened";

/// The folders kept in the browser's storage.
const KEPT: [&str; 3] = [CONFIG, DATA, CACHE];

struct Entry {
    bytes: Vec<u8>,
    modified_ms: u64,
    readonly: bool,
}

thread_local! {
    static FILES: RefCell<BTreeMap<PathBuf, Entry>> = const { RefCell::new(BTreeMap::new()) };
    /// The number of the last file opened (each gets a folder of its own).
    static OPENED_COUNT: RefCell<u32> = const { RefCell::new(0) };
}

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

fn kept(path: &Path) -> bool {
    KEPT.iter().any(|k| path.starts_with(k))
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{}: no such file", path.display()),
    )
}

/// Takes in what the browser's storage keeps; called once before the interface starts.
pub async fn load() {
    let stored = super::idb::load_all().await;
    FILES.with_borrow_mut(|files| {
        for (path, bytes) in stored {
            files.insert(
                PathBuf::from(path),
                Entry {
                    bytes,
                    modified_ms: 0,
                    readonly: false,
                },
            );
        }
    });
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    FILES.with_borrow(|files| {
        files
            .get(path)
            .map(|e| e.bytes.clone())
            .ok_or_else(|| not_found(path))
    })
}

pub fn meta(path: &Path) -> io::Result<Meta> {
    FILES.with_borrow(|files| {
        files
            .get(path)
            .map(|e| Meta {
                len: e.bytes.len() as u64,
                modified_ms: e.modified_ms,
                readonly: e.readonly,
            })
            .ok_or_else(|| not_found(path))
    })
}

pub fn write(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
    let keep = kept(path);
    FILES.with_borrow_mut(|files| {
        if files.get(path).is_some_and(|e| e.readonly) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{}: only read", path.display()),
            ));
        }
        if keep {
            super::idb::put(path.to_string_lossy().into_owned(), bytes.clone());
        }
        files.insert(
            path.to_owned(),
            Entry {
                bytes,
                modified_ms: now_ms(),
                readonly: false,
            },
        );
        Ok(())
    })
}

pub fn remove(path: &Path) -> io::Result<()> {
    FILES.with_borrow_mut(|files| {
        files.remove(path).ok_or_else(|| not_found(path))?;
        if kept(path) {
            super::idb::delete(path.to_string_lossy().into_owned());
        }
        Ok(())
    })
}

/// A file the user gave the page (chosen or dropped): kept for the session, only read.
/// Its path ends with its own name, in a folder of its own.
pub fn add_opened(name: &str, bytes: Vec<u8>) -> PathBuf {
    let n = OPENED_COUNT.with_borrow_mut(|n| {
        *n += 1;
        *n
    });
    // A name is a file's, not a path (a browser gives no folders; this makes sure).
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let path = Path::new(OPENED).join(n.to_string()).join(name);
    FILES.with_borrow_mut(|files| {
        files.insert(
            path.clone(),
            Entry {
                bytes,
                modified_ms: now_ms(),
                readonly: true,
            },
        );
    });
    path
}
