//! The files the user opens and saves: the system's dialogs on a computer; in a
//! browser its file chooser, files dropped on the page, and downloads. A file the user
//! gives the page is only read (decisions «Веб-версия»): it gets a path of the page's
//! file tree for the session and is opened by that path like a file on a disk.

use std::path::PathBuf;

/// The extensions of the databases the program opens (both cases for the dialogs of
/// systems that tell them apart).
const DATABASES: [&str; 10] = [
    "pgn", "PGN", "cbh", "CBH", "cbv", "CBV", "zip", "ZIP", "7z", "7Z",
];

/// Files a browser does not open: larger than this (decisions «Веб-версия»: a warning
/// above 1 GB — the program for the computer opens them).
pub const TOO_BIG: u64 = 1 << 30;

/// What the user chose or dropped: the databases to open, and the files left out
/// because they are too big for a browser (name, bytes).
#[derive(Default)]
pub struct Picked {
    pub paths: Vec<PathBuf>,
    pub too_big: Vec<(String, u64)>,
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

/// Keeps a copy of the session file `path` in the browser's storage, where it stays
/// between visits; the copy's path. Refused when the browser gives the site too little
/// room (the size and what is free, for the message).
#[cfg(web)]
pub fn keep_copy(path: &std::path::Path) -> Result<PathBuf, KeepCopy> {
    let bytes = super::fs::read(path).map_err(|e| KeepCopy::Failed(e.to_string()))?;
    let need = bytes.len() as u64;
    if let Some(free) = super::storage_free()
        && need + need / 10 > free
    {
        return Err(KeepCopy::NoRoom { need, free });
    }
    let name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    let dir = super::data_dir().unwrap_or_default().join("copies");
    let mut copy = dir.join(&name);
    let mut n = 1;
    while super::fs::exists(&copy) {
        n += 1;
        let stem = std::path::Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = std::path::Path::new(&name)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        copy = dir.join(format!("{stem} ({n}).{ext}"));
    }
    super::fs::write_atomic(&copy, &bytes).map_err(|e| KeepCopy::Failed(e.to_string()))?;
    Ok(copy)
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
                    too_big: Vec::new(),
                });
            });
    }
    #[cfg(web)]
    {
        let accept = DATABASES
            .iter()
            .step_by(2)
            .map(|e| format!(".{e}"))
            .collect::<Vec<_>>()
            .join(",");
        super::web::page::pick_files(&accept, move |files| then(opened(files)));
    }
}

/// The files dropped on the window in this frame, as databases to open: `then` gets
/// their paths once read (in a browser they are kept for the session like chosen
/// ones). The desktop program does not take dropped files: nothing comes there.
pub fn take_dropped(ctx: &egui::Context, then: impl FnOnce(Picked) + Send + 'static) {
    #[cfg(not(web))]
    {
        let _ = (ctx, then);
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
        wasm_bindgen_futures::spawn_local(async move {
            then(opened(super::web::page::read_files(files).await));
        });
    }
}

/// The files read, kept for the session.
#[cfg(web)]
fn opened(files: super::web::page::Read) -> Picked {
    Picked {
        paths: files
            .read
            .into_iter()
            .map(|(name, bytes)| super::web::vfs::add_opened(&name, bytes))
            .collect(),
        too_big: files.too_big,
    }
}

/// Files are dragged over the window now (the page shows where they go).
pub fn dragging(ctx: &egui::Context) -> bool {
    cfg!(web) && ctx.input(|i| !i.raw.hovered_files.is_empty())
}

/// Gives `pgn` to the user as the file `name` (a download, where the browser puts
/// them). Only a browser does this: the desktop program saves into files it chose.
#[cfg(web)]
pub fn download_pgn(name: &str, pgn: &str) -> Result<(), String> {
    super::web::page::download(name, "application/x-chess-pgn", pgn.as_bytes())
        .map_err(|e| format!("{e:?}"))
}
