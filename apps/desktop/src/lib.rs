//! ChessPenguin's window: the library, the game, analysis, play, preparation.
//!
//! One interface for two platforms. The desktop program (`src/main.rs`) starts it in a
//! native window ([`run_native`]); the web version (`apps/web`) in a page's canvas
//! ([`start_web`]). What differs between them — files, background work, storage,
//! dialogs, the clipboard — is behind [`platform`]. Features a platform does not have
//! are not built for it (`cfg(web)`: WebAssembly in a browser, set by `build.rs`), or,
//! where the interface only needs to know, hidden by [`platform::CAPS`]. The web build
//! compiles parts of the interface it never shows: what is unused there is the desktop
//! build's to find (the platform layer, [`platform`], is checked in both).
// (A browser's `std::fs::File` has no destructor: dropping one there is no closing.)
#![cfg_attr(web, allow(unused, clippy::drop_non_drop))]

mod adjourned;
mod analysis;
mod app;
mod assets;
mod board;
mod board_search;
mod chatgpt;
mod db_counts;
mod engine_download;
mod engine_offer;
mod engines;
mod examples;
mod file_state;
mod game_view;
#[cfg(not(web))]
mod graphics;
mod i18n;
mod large_db;
mod library;
mod maia_download;
#[cfg(web)]
mod my_games;
mod notation;
mod online;
#[cfg(not(web))]
mod online_ui;
mod own_db;
pub mod platform;
mod play;
mod player_games;
mod prep_ui;
mod reference;
mod review;
mod review_engine;
mod rodent4_download;
mod rodent_download;
mod san;
mod settings;
mod settings_ui;
mod sound;
mod translate;
mod translate_ui;
#[cfg(not(web))]
mod update;
#[cfg(not(web))]
mod update_ui;
mod viewed;
mod widgets;

/// Learning an opening: shared with the Android app.
use trainer as train;

/// The web version in the page's `canvas`: the program's data is read from the
/// browser's storage first, then the interface starts. `lang`: the language the page
/// was opened in (`?lang=` of the site's link), over the one chosen before.
#[cfg(web)]
pub async fn start_web(
    canvas: web_sys::HtmlCanvasElement,
    lang: Option<String>,
) -> Result<(), wasm_bindgen::JsValue> {
    platform::web::vfs::load().await;
    platform::web::remembered::restore().await;
    platform::web::engine::probe().await;
    platform::web::storage::refresh(|| {});
    let mut settings = settings::Settings::load();
    if let Some(chosen) = lang.as_deref().and_then(i18n::Lang::from_locale)
        && chosen != settings.language
    {
        settings.language = chosen;
        let _ = settings.save();
    }
    i18n::set_lang(settings.language);
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| {
                assets::install(&cc.egui_ctx);
                Ok(Box::new(app::App::new(&cc.egui_ctx)))
            }),
        )
        .await
}

/// The page's reader (`import-worker.js`, a worker with an instance of its own): takes
/// the files of a database one by one ([`reader_give`]), reads it and answers with its
/// text and index, or only how many games it holds (`count`; `platform::web::import`).
#[cfg(web)]
pub fn reader_give(name: String, bytes: Vec<u8>) {
    platform::web::import::give(name, bytes);
}

#[cfg(web)]
pub fn reader_read(count: bool) -> wasm_bindgen::JsValue {
    platform::web::import::answer(count)
}

/// The desktop program: `chesspenguin [--bench <database> [report.txt]]`.
#[cfg(not(web))]
pub fn run_native() -> eframe::Result {
    // `chesspenguin --bench <database> [report.txt]`: timings instead of the window
    // (`docs/dev/import.md`, "Замер на другой машине").
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--bench") {
        let Some(db) = args.get(1) else {
            eprintln!("usage: chesspenguin --bench <database> [report.txt]");
            std::process::exit(2);
        };
        let cache = settings::Settings::cache_dir().map(|d| d.join("boosters"));
        let result = match args.get(2) {
            Some(file) => std::fs::File::create(file)
                .and_then(|mut f| chess_import::bench::run(db.as_ref(), cache.as_deref(), &mut f)),
            None => chess_import::bench::run(db.as_ref(), cache.as_deref(), &mut std::io::stdout()),
        };
        if let Err(e) = result {
            eprintln!("bench: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    crate::i18n::set_lang(settings::Settings::load().language);
    // The runs of the opening trainer (`CP_TRAINING_FILE` elsewhere: development).
    if let Some(file) = std::env::var_os("CP_TRAINING_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
                .map(|d| d.data_local_dir().join("training.json"))
        })
    {
        train::set_file(file);
    }
    graphics::install_panic_hook();
    graphics::init_library_log();
    let mode = graphics::choose();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("io.github.vrubelroman.chessPenguin")
            .with_title("ChessPenguin")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([640.0, 480.0])
            // X11 uses this; on Wayland the icon comes from the .desktop entry matching the app id.
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
                    .unwrap_or_default(),
            ),
        wgpu_options: graphics::options(mode),
        ..Default::default()
    };
    let result = eframe::run_native(
        "ChessPenguin",
        options,
        Box::new(|cc| {
            assets::install(&cc.egui_ctx);
            Ok(Box::new(app::App::new(&cc.egui_ctx)))
        }),
    );
    if let Err(e) = &result {
        if graphics::retry(&e.to_string()) {
            return Ok(());
        }
        graphics::fatal(&e.to_string());
    }
    result
}
