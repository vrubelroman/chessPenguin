//! Reading the clipboard on request («Paste PGN», «Paste a line»): at once on a
//! computer; in a browser asked of it (it may ask the user, and answers later).
//! Writing needs nothing of this: `egui::Context::copy_text` works everywhere.

use std::sync::{Arc, Mutex};

/// A reading of the clipboard under way.
pub struct Reading {
    text: Arc<Mutex<Option<Option<String>>>>,
}

impl Reading {
    /// The text once read (`Some(None)`: empty, refused or unavailable).
    pub fn poll(&self) -> Option<Option<String>> {
        self.text.lock().ok()?.take()
    }
}

/// Starts reading the clipboard's text; `ctx` is woken when it is there.
pub fn read_text(ctx: &egui::Context) -> Reading {
    let text = Arc::new(Mutex::new(None));
    #[cfg(not(web))]
    {
        let _ = ctx;
        let read = arboard::Clipboard::new()
            .and_then(|mut c| c.get_text())
            .ok();
        if let Ok(mut t) = text.lock() {
            *t = Some(read);
        }
    }
    #[cfg(web)]
    {
        let (slot, ctx) = (text.clone(), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let read = match web_sys::window().map(|w| w.navigator().clipboard()) {
                Some(c) => wasm_bindgen_futures::JsFuture::from(c.read_text())
                    .await
                    .ok()
                    .and_then(|v| v.as_string()),
                None => None,
            };
            if let Ok(mut t) = slot.lock() {
                *t = Some(read);
            }
            ctx.request_repaint();
        });
    }
    Reading { text }
}
