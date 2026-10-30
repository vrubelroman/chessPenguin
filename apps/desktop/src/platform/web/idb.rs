//! The program's data in the browser's storage: one IndexedDB database, a store of
//! files by path, read whole at the start (settings and marks are small), every write
//! going there behind the interface, in the order made; and a store of the user's files
//! a Chromium browser remembers (their handles, by the page's path: [`super::remembered`]).

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

const DB: &str = "chesspenguin";
const STORE: &str = "files";
const HANDLES: &str = "handles";
/// 2: the store of handles.
const VERSION: u32 = 2;

/// The request's outcome once it is done.
async fn done(request: &web_sys::IdbRequest) -> Result<JsValue, JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let ok = Closure::once_into_js(move |ev: web_sys::Event| {
            let r = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::IdbRequest>().ok())
                .and_then(|r| r.result().ok())
                .unwrap_or(JsValue::UNDEFINED);
            let _ = resolve.call1(&JsValue::NULL, &r);
        });
        let err = Closure::once_into_js(move |ev: web_sys::Event| {
            let _ = reject.call1(&JsValue::NULL, &ev);
        });
        request.set_onsuccess(Some(ok.unchecked_ref()));
        request.set_onerror(Some(err.unchecked_ref()));
    });
    wasm_bindgen_futures::JsFuture::from(promise).await
}

async fn open() -> Result<web_sys::IdbDatabase, JsValue> {
    let factory = web_sys::window()
        .ok_or("no window")?
        .indexed_db()?
        .ok_or("no IndexedDB")?;
    let request = factory.open_with_u32(DB, VERSION)?;
    let upgrade = Closure::once_into_js(move |ev: web_sys::Event| {
        if let Some(db) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::IdbOpenDbRequest>().ok())
            .and_then(|r| r.result().ok())
            .and_then(|r| r.dyn_into::<web_sys::IdbDatabase>().ok())
        {
            for store in [STORE, HANDLES] {
                if !db.object_store_names().contains(store) {
                    let _ = db.create_object_store(store);
                }
            }
        }
    });
    request.set_onupgradeneeded(Some(upgrade.unchecked_ref()));
    done(&request).await?.dyn_into()
}

thread_local! {
    /// The database once opened (`None` until then, or when the browser refused it:
    /// a private window of some browsers; then nothing is kept between visits).
    static OPENED: std::cell::RefCell<Option<web_sys::IdbDatabase>> =
        const { std::cell::RefCell::new(None) };
}

/// Everything kept: (path, bytes). Empty when the browser keeps nothing for the page.
pub async fn load_all() -> Vec<(String, Vec<u8>)> {
    let db = match open().await {
        Ok(db) => db,
        Err(e) => {
            log::warn!("browser storage unavailable: {e:?}");
            return Vec::new();
        }
    };
    let read = async {
        let tx = db.transaction_with_str(STORE)?;
        let store = tx.object_store(STORE)?;
        let keys = done(&store.get_all_keys()?).await?;
        let values = done(&store.get_all()?).await?;
        let keys: js_sys::Array = keys.dyn_into()?;
        let values: js_sys::Array = values.dyn_into()?;
        Ok::<_, JsValue>(
            keys.iter()
                .zip(values.iter())
                .filter_map(|(k, v)| {
                    let bytes = v.dyn_into::<js_sys::Uint8Array>().ok()?.to_vec();
                    Some((k.as_string()?, bytes))
                })
                .collect(),
        )
    };
    let files = read.await.unwrap_or_else(|e| {
        log::warn!("browser storage unreadable: {e:?}");
        Vec::new()
    });
    OPENED.with_borrow_mut(|o| *o = Some(db));
    files
}

fn store_named(name: &str) -> Option<web_sys::IdbObjectStore> {
    OPENED.with_borrow(|db| {
        let tx = db
            .as_ref()?
            .transaction_with_str_and_mode(name, web_sys::IdbTransactionMode::Readwrite)
            .ok()?;
        tx.object_store(name).ok()
    })
}

fn store() -> Option<web_sys::IdbObjectStore> {
    store_named(STORE)
}

/// The handles kept: (the page's path, the handle). Read after [`load_all`].
pub async fn handles() -> Vec<(String, JsValue)> {
    let Some(db) = OPENED.with_borrow(|db| db.clone()) else {
        return Vec::new();
    };
    let read = async {
        let tx = db.transaction_with_str(HANDLES)?;
        let store = tx.object_store(HANDLES)?;
        let keys: js_sys::Array = done(&store.get_all_keys()?).await?.dyn_into()?;
        let values: js_sys::Array = done(&store.get_all()?).await?.dyn_into()?;
        Ok::<_, JsValue>(
            keys.iter()
                .zip(values.iter())
                .filter_map(|(k, v)| Some((k.as_string()?, v)))
                .collect(),
        )
    };
    read.await.unwrap_or_else(|e: JsValue| {
        log::warn!("browser storage: remembered files unreadable: {e:?}");
        Vec::new()
    })
}

pub fn put_handle(path: &str, handle: &JsValue) {
    if let Some(store) = store_named(HANDLES)
        && let Err(e) = store.put_with_key(handle, &JsValue::from_str(path))
    {
        log::warn!("browser storage: cannot remember {path}: {e:?}");
    }
}

pub fn delete_handle(path: &str) {
    if let Some(store) = store_named(HANDLES) {
        let _ = store.delete(&JsValue::from_str(path));
    }
}

/// Keeps `bytes` as `path` (transactions of one page run in the order made).
pub fn put(path: String, bytes: Vec<u8>) {
    if let Some(store) = store() {
        let value = js_sys::Uint8Array::from(bytes.as_slice());
        if let Err(e) = store.put_with_key(&value, &JsValue::from_str(&path)) {
            log::warn!("browser storage: cannot keep {path}: {e:?}");
        }
    }
}

pub fn delete(path: String) {
    if let Some(store) = store()
        && let Err(e) = store.delete(&JsValue::from_str(&path))
    {
        log::warn!("browser storage: cannot delete {path}: {e:?}");
    }
}
