// No console window for release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod app;
mod assets;
mod board;
mod board_search;
mod engines;
mod game_view;
mod i18n;
mod large_db;
mod library;
mod notation;
mod report;
mod review;
mod settings;
mod settings_ui;
mod sound;
mod viewed;
mod widgets;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("io.github.vrubelroman.chessPenguin")
            .with_title("ChessPenguin")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 600.0])
            // X11 uses this; on Wayland the icon comes from the .desktop entry matching the app id.
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "ChessPenguin",
        options,
        Box::new(|cc| {
            assets::install(&cc.egui_ctx);
            Ok(Box::new(app::App::new(&cc.egui_ctx)))
        }),
    )
}
