//! The page's file tree: files by path, in memory. The program's data (`/config`,
//! `/data`, `/cache`) is written through to the browser's storage ([`super::idb`]) and
//! read from it at the start ([`load`]); the rest lives for the session. The user's
//! files are the browser's `File`s, not their contents: the page's reader reads them
//! (`super::import`), so a database's bytes never fill the page's memory.

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
    content: Content,
    modified_ms: u64,
    readonly: bool,
}

enum Content {
    Bytes(Vec<u8>),
    /// A file the user gave the page: the browser reads it when asked.
    File(web_sys::File),
}

impl Content {
    fn len(&self) -> u64 {
        match self {
            Content::Bytes(b) => b.len() as u64,
            Content::File(f) => f.size() as u64,
        }
    }
}

thread_local! {
    static FILES: RefCell<BTreeMap<PathBuf, Entry>> = const { RefCell::new(BTreeMap::new()) };
    /// The number of the last file opened (each gets a folder of its own; numbered from
    /// the visit's start time, so that a file of an earlier visit keeps its own).
    static OPENED_COUNT: RefCell<u64> = const { RefCell::new(0) };
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
                    content: Content::Bytes(bytes),
                    modified_ms: 0,
                    readonly: false,
                },
            );
        }
    });
}

/// The file's bytes; a user's file is read by the page's reader instead
/// ([`source`]).
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    FILES.with_borrow(
        |files| match &files.get(path).ok_or_else(|| not_found(path))?.content {
            Content::Bytes(b) => Ok(b.clone()),
            Content::File(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the user's file is read by the page's reader",
            )),
        },
    )
}

/// The file for the page's reader: its bytes (a `Uint8Array`) or the user's `File`.
pub fn source(path: &Path) -> io::Result<wasm_bindgen::JsValue> {
    FILES.with_borrow(|files| {
        Ok(
            match &files.get(path).ok_or_else(|| not_found(path))?.content {
                Content::Bytes(b) => js_sys::Uint8Array::from(b.as_slice()).into(),
                Content::File(f) => f.clone().into(),
            },
        )
    })
}

pub fn meta(path: &Path) -> io::Result<Meta> {
    FILES.with_borrow(|files| {
        files
            .get(path)
            .map(|e| Meta {
                len: e.content.len(),
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
                content: Content::Bytes(bytes),
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

/// The files right in the folder `dir`.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    FILES.with_borrow(|files| {
        files
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect()
    })
}

/// A database the user gave the page (chosen, dropped, found in a chosen folder):
/// its files (its own first: a ChessBase database's `.cbh`, then its other files),
/// kept for the session, only read, in a folder of their own; the path of the first.
pub fn add_opened(files: Vec<web_sys::File>) -> PathBuf {
    let n = OPENED_COUNT.with_borrow_mut(|n| {
        *n = (*n + 1).max(now_ms());
        *n
    });
    let path = Path::new(OPENED)
        .join(n.to_string())
        .join(files.first().map(name_of).unwrap_or_default());
    put_opened(&path, files);
    path
}

/// A database of an earlier visit the browser let the page read again, at its old
/// path (its other files beside it).
pub fn put_opened(path: &Path, files: Vec<web_sys::File>) {
    let dir = path.parent().unwrap_or(Path::new(OPENED)).to_owned();
    FILES.with_borrow_mut(|entries| {
        for (i, f) in files.into_iter().enumerate() {
            let at = if i == 0 {
                path.to_owned()
            } else {
                dir.join(name_of(&f))
            };
            entries.insert(
                at,
                Entry {
                    content: Content::File(f),
                    modified_ms: now_ms(),
                    readonly: true,
                },
            );
        }
    });
}

/// A file's name, not a path (a browser gives no folders; this makes sure).
fn name_of(f: &web_sys::File) -> String {
    let name = f.name();
    name.rsplit(['/', '\\']).next().unwrap_or(&name).to_owned()
}

/// The files of the database `path` for the page's reader: (name, [`source`]); a
/// ChessBase database's `.cbh` with the files of its name beside it.
pub fn database_files(path: &Path) -> io::Result<Vec<(String, wasm_bindgen::JsValue)>> {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut out = vec![(name(path), source(path)?)];
    let cbh = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cbh"));
    if cbh && let Some(dir) = path.parent() {
        let stem = |p: &Path| {
            p.file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        for p in list(dir) {
            if p != path && stem(&p) == stem(path) {
                out.push((name(&p), source(&p)?));
            }
        }
    }
    Ok(out)
}
