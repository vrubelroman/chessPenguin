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

/// Asks the user for databases to open; `then` gets their paths (nothing when the
/// user cancelled). The dialog does not hold the screen: `then` comes later.
pub fn pick_databases(then: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
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
                then(picked.unwrap_or_default());
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
        super::web::page::pick_files(&accept, move |files| {
            then(
                files
                    .into_iter()
                    .map(|(name, bytes)| super::web::vfs::add_opened(&name, bytes))
                    .collect(),
            );
        });
    }
}

/// The files dropped on the window in this frame, as databases to open: `then` gets
/// their paths once read (in a browser they are kept for the session like chosen
/// ones). The desktop program does not take dropped files: nothing comes there.
pub fn take_dropped(ctx: &egui::Context, then: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
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
            let read = super::web::page::read_files(files).await;
            then(
                read.into_iter()
                    .map(|(name, bytes)| super::web::vfs::add_opened(&name, bytes))
                    .collect(),
            );
        });
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
