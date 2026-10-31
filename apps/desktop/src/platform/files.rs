//! The files the user opens and saves: the system's dialogs on a computer; in a
//! browser its file chooser, files dropped on the page, and downloads. A file the user
//! gives the page is only read (decisions «Веб-версия»): it gets a path of the page's
//! file tree for the session and is opened by that path like a file on a disk. Chrome
//! and Edge remember a chosen file for the next visit ([`super::web::remembered`]).

use std::path::PathBuf;

/// The extensions of the databases the program opens (both cases for the dialogs of
/// systems that tell them apart).
const DATABASES: [&str; 10] = [
    "pgn", "PGN", "cbh", "CBH", "cbv", "CBV", "zip", "ZIP", "7z", "7Z",
];

/// Databases a browser does not open: their files together larger than this (the
/// program for the computer opens them).
pub const TOO_BIG: u64 = 2_000_000_000;

/// What the user chose or dropped: the databases to open, the ones left out because
/// they are too big for a browser (name, bytes of their files), the ChessBase
/// databases not all files of which were given (the name of their `.cbh`), whether a
/// chosen (or, on a computer, dropped) folder held no database, and the files dropped
/// on a computer's window that are no databases (their names).
#[derive(Debug, Default, PartialEq)]
pub struct Picked {
    pub paths: Vec<PathBuf>,
    pub too_big: Vec<(String, u64)>,
    pub incomplete: Vec<String>,
    pub empty_folder: bool,
    pub unsupported: Vec<String>,
    /// Files dropped on a computer's window that are no longer there (their names).
    pub missing: Vec<String>,
}

/// The files the user gave the page, made databases: the ChessBase ones of their
/// several files (`chess_import::group_chosen`), each kept for the session in a folder
/// of its own. Also the files of each database kept (its path, their places in
/// `files`), for the browser to remember.
#[cfg(web)]
pub(super) fn gather(
    files: &[web_sys::File],
    folder: bool,
) -> (Picked, Vec<(PathBuf, Vec<usize>)>) {
    let names: Vec<String> = files.iter().map(|f| f.name()).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let chosen = chess_import::group_chosen(&refs, folder);
    let mut picked = Picked {
        incomplete: chosen.incomplete,
        ..Picked::default()
    };
    let mut kept = Vec::new();
    for db in chosen.databases {
        let size: u64 = db.iter().map(|&i| files[i].size() as u64).sum();
        if size > TOO_BIG {
            picked.too_big.push((names[db[0]].clone(), size));
            continue;
        }
        let path = super::web::vfs::add_opened(db.iter().map(|&i| files[i].clone()).collect());
        picked.paths.push(path.clone());
        kept.push((path, db));
    }
    picked.empty_folder = folder
        && picked.paths.is_empty()
        && picked.too_big.is_empty()
        && picked.incomplete.is_empty();
    (picked, kept)
}

/// A ChessBase database's `.cbh` given without the files it cannot be read without
/// (a browser has only the files the user gave it).
pub fn chessbase_incomplete(path: &std::path::Path) -> bool {
    #[cfg(web)]
    {
        let cbh = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cbh"));
        cbh && super::web::vfs::database_files(path).is_ok_and(|files| {
            !files
                .iter()
                .any(|(name, _)| name.to_lowercase().ends_with(".cbg"))
        })
    }
    #[cfg(not(web))]
    {
        let _ = path;
        false
    }
}

/// How the user's file `path` is named to the user: on a computer its path, in a
/// browser its name (the page's own places, `/opened/…`, mean nothing to the user).
pub fn shown(path: &std::path::Path) -> String {
    #[cfg(web)]
    return path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    #[cfg(not(web))]
    path.display().to_string()
}

/// A file the browser holds for this visit only (one the user chose or dropped): after
/// a reload it is gone, the user chooses it again or keeps a copy. Never on a computer.
pub fn session_only(path: &std::path::Path) -> bool {
    #[cfg(web)]
    return path.starts_with(super::web::vfs::OPENED);
    #[cfg(not(web))]
    {
        let _ = path;
        false
    }
}

/// Keeps a copy of the session database `path` (all its files) in the browser's
/// storage, where it stays between visits; `then` gets the copy's path. Refused when
/// the browser gives the site too little room (the size and what is free, for the
/// message).
#[cfg(web)]
pub fn keep_copy(path: &std::path::Path, then: impl FnOnce(Result<PathBuf, KeepCopy>) + 'static) {
    use wasm_bindgen::JsCast;
    let parts = match super::web::vfs::database_files(path) {
        Ok(p) => p,
        Err(e) => return then(Err(KeepCopy::Failed(e.to_string()))),
    };
    let need: u64 = parts
        .iter()
        .map(|(_, s)| match s.dyn_ref::<web_sys::Blob>() {
            Some(b) => b.size() as u64,
            None => js_sys::Reflect::get(s, &"length".into())
                .ok()
                .and_then(|l| l.as_f64())
                .unwrap_or(0.0) as u64,
        })
        .sum();
    if let Some(free) = super::storage_free()
        && need + need / 10 > free
    {
        return then(Err(KeepCopy::NoRoom { need, free }));
    }
    // A folder of its own, as the database had: the names stay as they were.
    let dir = super::data_dir()
        .unwrap_or_default()
        .join("copies")
        .join((js_sys::Date::now() as u64).to_string());
    let first = dir.join(&parts[0].0);
    wasm_bindgen_futures::spawn_local(async move {
        for (name, source) in parts {
            let bytes = match source.dyn_into::<web_sys::Blob>() {
                Ok(blob) => match wasm_bindgen_futures::JsFuture::from(blob.array_buffer()).await {
                    Ok(buf) => js_sys::Uint8Array::new(&buf).to_vec(),
                    Err(e) => return then(Err(KeepCopy::Failed(format!("{e:?}")))),
                },
                Err(array) => js_sys::Uint8Array::new(&array).to_vec(),
            };
            if let Err(e) = super::fs::write_atomic(&dir.join(name), &bytes) {
                return then(Err(KeepCopy::Failed(e.to_string())));
            }
        }
        then(Ok(first));
    });
}

/// Why no copy was kept.
#[cfg(web)]
pub enum KeepCopy {
    NoRoom { need: u64, free: u64 },
    Failed(String),
}

/// Asks the user for databases to open; `then` gets what was chosen (nothing when the
/// user cancelled). The dialog does not hold the screen: `then` comes later.
pub fn pick_databases(then: impl FnOnce(Picked) + Send + 'static) {
    #[cfg(not(web))]
    {
        use crate::i18n::tr;
        let _ = std::thread::Builder::new()
            .name("dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .set_title(tr("Open database"))
                    .add_filter(tr("Chess databases"), &DATABASES)
                    .add_filter("PGN", &["pgn", "PGN"])
                    .add_filter(tr("All files"), &["*"])
                    .pick_files();
                then(Picked {
                    paths: picked.unwrap_or_default(),
                    ..Picked::default()
                });
            });
    }
    #[cfg(web)]
    {
        let extensions: Vec<String> = DATABASES
            .iter()
            .step_by(2)
            .map(|e| format!(".{e}"))
            .collect();
        if super::web::remembered::supported() {
            super::web::remembered::pick(&extensions, then);
        } else {
            super::web::page::pick_files(&extensions.join(","), move |files| {
                then(gather(&files, false).0)
            });
        }
    }
}

/// Asks the user for a database's folder (a ChessBase database is several files): its
/// databases are found by their files' names. Chrome and Edge remember the folder.
#[cfg(web)]
pub fn pick_database_folder(then: impl FnOnce(Picked) + Send + 'static) {
    if super::web::remembered::supported_folder() {
        super::web::remembered::pick_folder(then);
    } else {
        super::web::page::pick_folder(move |files| then(gather(&files, true).0));
    }
}

/// The browser remembers the files chosen in its dialog for the next visit (Chrome,
/// Edge); the others, and a computer, have no need or no way.
pub fn remembers_chosen() -> bool {
    #[cfg(web)]
    return super::web::remembered::supported();
    #[cfg(not(web))]
    false
}

/// The browser remembers the file `path` of an earlier visit and asks the user before
/// the page reads it again ([`allow_reading`]). Never on a computer.
pub fn waiting_to_read(path: &std::path::Path) -> bool {
    #[cfg(web)]
    return super::web::remembered::waiting(path);
    #[cfg(not(web))]
    {
        let _ = path;
        false
    }
}

/// Asks the browser to let the page read the remembered file `path` again (soon after
/// the user's click); `then`: whether it is back.
#[cfg(web)]
pub fn allow_reading(path: &std::path::Path, then: impl FnOnce(bool) + 'static) {
    super::web::remembered::allow(path, then);
}

/// The file `path` left the library: a browser forgets it too.
pub fn forget(path: &std::path::Path) {
    #[cfg(web)]
    super::web::remembered::forget(path);
    #[cfg(not(web))]
    let _ = path;
}

/// The files dropped on the window in this frame, as databases to open: `then` gets
/// their paths once read (in a browser they are kept for the session like chosen
/// ones; on a computer they are opened where they are, as «Add database» does).
pub fn take_dropped(ctx: &egui::Context, then: impl FnOnce(Picked) + Send + 'static) {
    #[cfg(not(web))]
    {
        let paths: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| p.is_absolute())
                .collect()
        });
        if paths.is_empty() {
            return;
        }
        // Folders are listed and files' neighbours looked at: off the screen's thread.
        let _ = std::thread::Builder::new()
            .name("dropped".into())
            .spawn(move || then(sort_dropped(&paths)));
    }
    #[cfg(web)]
    {
        let files: Vec<web_sys::File> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.web_file().cloned())
                .collect()
        });
        if files.is_empty() {
            return;
        }
        then(gather(&files, false).0);
    }
}

/// Files are dragged over the window now (the page shows where they go).
pub fn dragging(ctx: &egui::Context) -> bool {
    ctx.input(|i| !i.raw.hovered_files.is_empty())
}

/// The extensions of the other files of a ChessBase database (beside its `.cbh`).
#[cfg(not(web))]
const CHESSBASE_PARTS: [&str; 14] = [
    "cbg", "cba", "cbp", "cbt", "cbc", "cbs", "cbj", "cbe", "cbm", "cbb", "cbgi", "cbtt", "cbpi",
    "cbl",
];

/// The databases of the files and folders dropped on a computer's window: a file of
/// [`chess_import::EXTENSIONS`] is one; another file of a ChessBase database (its
/// `.cbg`, `.cba`, …: [`CHESSBASE_PARTS`]) stands for the `.cbh` of the same name
/// beside it; a folder gives the databases in it. Each database once (its real file),
/// in the order dropped; a file no longer there and a file of no database are named.
#[cfg(not(web))]
fn sort_dropped(paths: &[PathBuf]) -> Picked {
    use std::collections::{HashMap, HashSet};
    let mut picked = Picked::default();
    let mut seen = HashSet::new();
    // Each folder is listed once, however many of its files were dropped.
    let mut listed: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut list = |dir: &std::path::Path| -> Vec<PathBuf> {
        listed
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .flatten()
                            .map(|e| e.path())
                            .filter(|f| f.is_file())
                            .collect()
                    })
                    .unwrap_or_default();
                files.sort();
                files
            })
            .clone()
    };
    let name = |p: &std::path::Path| {
        p.file_name()
            .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
    };
    for p in paths {
        let found = if p.is_dir() {
            let found: Vec<PathBuf> = list(p)
                .into_iter()
                .filter(|f| chess_import::EXTENSIONS.contains(&extension(f).as_str()))
                .collect();
            if found.is_empty() {
                picked.empty_folder = true;
            }
            found
        } else if !p.is_file() {
            picked.missing.push(name(p));
            continue;
        } else {
            let ext = extension(p);
            if chess_import::EXTENSIONS.contains(&ext.as_str()) {
                vec![p.clone()]
            } else if CHESSBASE_PARTS.contains(&ext.as_str())
                && let Some(cbh) = p.parent().and_then(|dir| cbh_beside(p, &list(dir)))
            {
                vec![cbh]
            } else {
                picked.unsupported.push(name(p));
                continue;
            }
        };
        for f in found {
            if seen.insert(f.canonicalize().unwrap_or_else(|_| f.clone())) {
                picked.paths.push(f);
            }
        }
    }
    picked
}

/// The extension of `p`, in lower case.
#[cfg(not(web))]
fn extension(p: &std::path::Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The `.cbh` of the ChessBase database the file `part` belongs to, among the files
/// of its folder (names differ in case in sets copied from Windows).
#[cfg(not(web))]
fn cbh_beside(part: &std::path::Path, folder: &[PathBuf]) -> Option<PathBuf> {
    let stem = part.file_stem()?.to_string_lossy().to_lowercase();
    let want = format!("{stem}.cbh");
    folder
        .iter()
        .find(|f| {
            f.file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase() == want)
        })
        .cloned()
}

/// Gives `pgn` to the user as the file `name` (a download, where the browser puts
/// them). Only a browser does this: the desktop program saves into files it chose.
#[cfg(web)]
pub fn download_pgn(name: &str, pgn: &str) -> Result<(), String> {
    super::web::page::download(name, "application/x-chess-pgn", pgn.as_bytes())
        .map_err(|e| format!("{e:?}"))
}

#[cfg(all(test, not(web)))]
mod tests {
    use super::*;

    #[test]
    fn dropped_files_are_sorted_into_databases() {
        let dir = std::env::temp_dir().join(format!("cp-dropped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("Mega");
        std::fs::create_dir_all(&base).unwrap();
        let touch = |p: &std::path::Path| std::fs::write(p, b"x").unwrap();
        let games = dir.join("games.PGN");
        let archive = dir.join("set.7z");
        let notes = dir.join("notes.txt");
        let cbh = base.join("Mega.CBH");
        let parts = ["Mega.cbg", "Mega.cba", "Mega.cbj", "Mega.CBTT"].map(|n| base.join(n));
        let ini = base.join("Mega.ini");
        let lonely = dir.join("lost.cbg");
        for p in [&games, &archive, &notes, &cbh, &lonely, &ini] {
            touch(p);
        }
        for p in &parts {
            touch(p);
        }
        let cbv = base.join("Other.cbv");
        touch(&cbv);
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        touch(&empty.join("readme.md"));

        // Files of each kind; all the files of a ChessBase database give it once.
        let mut dropped = vec![games.clone(), archive.clone()];
        dropped.extend(parts.iter().cloned());
        dropped.push(cbh.clone());
        let picked = sort_dropped(&dropped);
        assert_eq!(
            picked.paths,
            vec![games.clone(), archive.clone(), cbh.clone()]
        );
        assert!(picked.unsupported.is_empty() && picked.missing.is_empty());
        assert!(!picked.empty_folder);

        // What is no database is named: another file, a ChessBase part without its
        // .cbh, a file of an unknown kind beside a .cbh.
        let picked = sort_dropped(&[notes.clone(), lonely.clone(), ini.clone(), games.clone()]);
        assert_eq!(picked.paths, vec![games.clone()]);
        assert_eq!(picked.unsupported, ["notes.txt", "lost.cbg", "Mega.ini"]);

        // A file no longer there: named, not taken.
        let picked = sort_dropped(&[dir.join("gone.pgn"), games.clone()]);
        assert_eq!(picked.paths, vec![games.clone()]);
        assert_eq!(picked.missing, ["gone.pgn"]);
        assert!(picked.unsupported.is_empty());

        // A folder: its databases (by name); one without any is told.
        let picked = sort_dropped(&[base.clone(), empty.clone()]);
        assert_eq!(picked.paths, vec![cbh.clone(), cbv.clone()]);
        assert!(picked.empty_folder);

        // The same file by a link and by its own path: once.
        #[cfg(unix)]
        {
            let link = dir.join("link.pgn");
            std::os::unix::fs::symlink(&games, &link).unwrap();
            let picked = sort_dropped(&[link.clone(), games.clone()]);
            assert_eq!(picked.paths, vec![link]);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
