//! The program as a page of a browser (docs/design/web-version.md §1.1, §1.3): a game
//! on the screen is an entry of the browser's history, so its «Back» means «← Library»;
//! unsaved work makes the browser ask before the page is left. A desktop program has
//! neither: nothing happens there.

#[cfg(web)]
mod imp {
    use std::cell::Cell;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    thread_local! {
        /// The history has our entry for the game on the screen.
        static IN_GAME: Cell<bool> = const { Cell::new(false) };
        /// «Back» was pressed (the entry left) and not taken yet.
        static BACK: Cell<bool> = const { Cell::new(false) };
        /// Our own `history.back()`: its `popstate` is not the user's.
        static OWN_BACK: Cell<bool> = const { Cell::new(false) };
        static LISTENING: Cell<bool> = const { Cell::new(false) };
        static WARN: Cell<bool> = const { Cell::new(false) };
        /// The page's `beforeunload`: the browser asks before the page is left.
        static ASK: Closure<dyn FnMut(web_sys::BeforeUnloadEvent)> =
            Closure::new(|e: web_sys::BeforeUnloadEvent| {
                e.prevent_default();
                e.set_return_value("");
            });
    }

    fn listen(ctx: &egui::Context) {
        if LISTENING.replace(true) {
            return;
        }
        let ctx = ctx.clone();
        let on_pop = Closure::<dyn FnMut()>::new(move || {
            if OWN_BACK.replace(false) {
                return;
            }
            if IN_GAME.replace(false) {
                BACK.set(true);
                ctx.request_repaint();
            }
        });
        if let Some(w) = web_sys::window() {
            let _ = w.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
        }
        // Listens as long as the page lives.
        on_pop.forget();
    }

    pub fn game_shown(ctx: &egui::Context, shown: bool) {
        listen(ctx);
        let Some(history) = web_sys::window().and_then(|w| w.history().ok()) else {
            return;
        };
        if shown && !IN_GAME.get() {
            let state = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&state, &"cpGame".into(), &JsValue::TRUE);
            if history.push_state(&state, "").is_ok() {
                IN_GAME.set(true);
            }
        } else if !shown && IN_GAME.get() {
            // The game left by the program's own button: its entry goes too.
            IN_GAME.set(false);
            OWN_BACK.set(true);
            let _ = history.back();
        }
    }

    pub fn take_back() -> bool {
        BACK.replace(false)
    }

    pub fn warn_on_leave(warn: bool) {
        if WARN.replace(warn) == warn {
            return;
        }
        let Some(w) = web_sys::window() else { return };
        if warn {
            ASK.with(|ask| w.set_onbeforeunload(Some(ask.as_ref().unchecked_ref())));
        } else {
            w.set_onbeforeunload(None);
        }
    }
}

/// The game is on the screen (`true`) or not: kept in step with the browser's history.
pub fn game_shown(ctx: &egui::Context, shown: bool) {
    #[cfg(web)]
    imp::game_shown(ctx, shown);
    #[cfg(not(web))]
    let _ = (ctx, shown);
}

/// The browser's «Back» left the game since the last call (then it is «← Library»).
pub fn take_back() -> bool {
    #[cfg(web)]
    return imp::take_back();
    #[cfg(not(web))]
    false
}

/// Whether leaving the page (closing the tab, reloading) should be asked about: the
/// browser asks in its own words.
pub fn warn_on_leave(warn: bool) {
    #[cfg(web)]
    imp::warn_on_leave(warn);
    #[cfg(not(web))]
    let _ = warn;
}
