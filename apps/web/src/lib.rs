//! ChessPenguin in a browser: the entry point the page (`page/index.html`) calls once it
//! has decided the device is a computer. The interface is the desktop program's
//! (`chesspenguin_app`), with the web platform under it (`docs/dev/web.md`).
#![cfg(target_family = "wasm")]

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The reader worker's work (`page/import-worker.js`): one more file of the database
/// to read (its own file first) …
#[wasm_bindgen]
pub fn reader_give(name: String, bytes: Vec<u8>) {
    chesspenguin_app::reader_give(name, bytes);
}

/// … and the database read and indexed away from the page (or only its games counted).
#[wasm_bindgen]
pub fn reader_read(count: bool) -> JsValue {
    chesspenguin_app::reader_read(count)
}

/// Starts ChessPenguin in the page's canvas `canvas_id`. Resolves once the interface
/// runs; an error says why it could not start (no WebGL, …).
#[wasm_bindgen]
pub async fn start(canvas_id: String) -> Result<(), JsValue> {
    // The program's log and its panics go to the browser's console.
    let _ = eframe::WebLogger::init(log::LevelFilter::Info);
    let window = web_sys::window().ok_or("no window")?;
    let canvas = window
        .document()
        .ok_or("no document")?
        .get_element_by_id(&canvas_id)
        .ok_or("no canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    let lang = web_sys::UrlSearchParams::new_with_str(&window.location().search()?)?.get("lang");
    chesspenguin_app::start_web(canvas, lang).await
}
