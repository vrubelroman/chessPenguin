// No console window for release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod app;
mod assets;
mod board;
mod board_search;
mod chatgpt;
mod engine_download;
mod engine_offer;
mod engines;
mod examples;
mod game_view;
mod graphics;
mod i18n;
mod large_db;
mod library;
mod maia_download;
mod notation;
mod online;
mod online_ui;
mod own_db;
mod play;
mod reference;
mod review;
mod san;
mod settings;
mod settings_ui;
mod sound;
mod train;
mod translate;
mod translate_ui;
mod update;
mod update_ui;
mod viewed;
mod widgets;

fn main() -> eframe::Result {
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
