//! What the browser gives the site to keep: the space used and left
//! (`navigator.storage.estimate()`: the site's quota, not the disk's free space) and
//! whether its data is kept for good (`persist()`: not cleared when space runs low).
//! Both are asked for in the background; the screen shows what came last.

use std::cell::Cell;

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    static USAGE: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    static PERSISTED: Cell<Option<bool>> = const { Cell::new(None) };
}

fn manager() -> Option<web_sys::StorageManager> {
    Some(web_sys::window()?.navigator().storage())
}

fn number(o: &wasm_bindgen::JsValue, key: &str) -> Option<u64> {
    js_sys::Reflect::get(o, &key.into())
        .ok()?
        .as_f64()
        .map(|n| n as u64)
}

/// Asks the browser again how much is used and left; `then` once it answered.
pub fn refresh(then: impl FnOnce() + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(m) = manager() {
            if let Ok(p) = m.estimate()
                && let Ok(e) = JsFuture::from(p).await
                && let (Some(usage), Some(quota)) = (number(&e, "usage"), number(&e, "quota"))
            {
                USAGE.set(Some((usage, quota)));
            }
            if let Ok(p) = m.persisted()
                && let Ok(v) = JsFuture::from(p).await
            {
                PERSISTED.set(v.as_bool());
            }
        }
        then();
    });
}

/// (used, quota) in bytes, as the browser last said.
pub fn usage() -> Option<(u64, u64)> {
    USAGE.get()
}

/// Whether the site's data is kept for good (`None`: not known yet).
pub fn persisted() -> Option<bool> {
    PERSISTED.get()
}

/// Asks the browser to keep the site's data for good (Firefox asks the user; Chromium
/// and Safari decide themselves); `then` with the answer.
pub fn persist(then: impl FnOnce(bool) + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        let granted = match manager().and_then(|m| m.persist().ok()) {
            Some(p) => JsFuture::from(p)
                .await
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Boolean>().ok())
                .is_some_and(|b| b.value_of()),
            None => false,
        };
        PERSISTED.set(Some(granted));
        then(granted);
    });
}
