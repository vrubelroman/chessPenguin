//! Stockfish 19 for the web version (npm `stockfish@19.0.1`, nmrugg/stockfish.js,
//! GPL-3.0): the single-threaded build, served by our site beside the program
//! (`/app/engine/`, apps/web/build.sh). Its `.wasm` (99 MB, 78 MB in Brotli) is
//! downloaded only when the user agrees (decisions «Веб-версия»), checked against its
//! SHA-256 and kept in the browser's Cache Storage. A worker of the engine gets it as a
//! `blob:` address after `#` (the script takes the wasm's address from there): no worker
//! downloads it again.
//!
//! Its search is that of the official Stockfish 19 (the same evaluations, node for node:
//! docs/research/web-version-tech.md §2.2), so a game review gives the same marks.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

/// The engine's worker script and its WebAssembly, relative to the page.
pub const SCRIPT: &str = "engine/stockfish-19-single.js";
const WASM: &str = "engine/stockfish-19-single.wasm";
/// The wasm as published (`stockfish@19.0.1`, `bin/stockfish-19-single.wasm`).
pub const WASM_BYTES: u64 = 99_169_054;
const WASM_SHA256: &str = "991bede2f253ebb08ff01fcc48270f4b9c8e66b12d5bd5f8d6f52eb3e3b3a740";
/// What travels over the network (the site sends it in Brotli).
pub const DOWNLOAD_MB: u64 = 78;
/// The cache of the engine; another version of it gets another name.
const CACHE: &str = "chesspenguin-engine-sf19.0.1";

thread_local! {
    /// The cached wasm as a `blob:` address, once it is there.
    static BLOB: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Why the engine could not be downloaded.
#[derive(Debug)]
pub enum Fetch {
    Network(String),
    Server(u16),
    /// Not the published file.
    Checksum,
    /// The browser would not keep it (space, a private window).
    Storage(String),
    Cancelled,
}

fn js(e: JsValue) -> String {
    e.as_string()
        .or_else(|| {
            e.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("{e:?}"))
}

fn caches() -> Result<web_sys::CacheStorage, String> {
    web_sys::window().ok_or("no window")?.caches().map_err(js)
}

async fn open() -> Result<web_sys::Cache, String> {
    JsFuture::from(caches()?.open(CACHE))
        .await
        .map_err(js)?
        .dyn_into()
        .map_err(js)
}

/// Makes the cached engine usable (a `blob:` address); called at the start.
pub async fn probe() {
    let found = async {
        let cache = open().await?;
        let hit = JsFuture::from(cache.match_with_str(WASM))
            .await
            .map_err(js)?;
        let Ok(response) = hit.dyn_into::<web_sys::Response>() else {
            return Ok::<_, String>(None);
        };
        let blob: web_sys::Blob = JsFuture::from(response.blob().map_err(js)?)
            .await
            .map_err(js)?
            .dyn_into()
            .map_err(js)?;
        if blob.size() as u64 != WASM_BYTES {
            return Ok(None);
        }
        web_sys::Url::create_object_url_with_blob(&blob)
            .map(Some)
            .map_err(js)
    };
    match found.await {
        Ok(url) => BLOB.with_borrow_mut(|b| *b = url),
        Err(e) => log::warn!("the engine's cache: {e}"),
    }
}

/// The address of a worker of the engine, once it is downloaded.
pub fn worker_script() -> Option<String> {
    BLOB.with_borrow(|b| b.as_ref().map(|url| format!("{SCRIPT}#{url}")))
}

/// Downloads the engine into the cache; `progress(done, total)` on the way.
pub async fn download(progress: impl Fn(u64, u64), cancel: Arc<AtomicBool>) -> Result<(), Fetch> {
    let window = web_sys::window().ok_or_else(|| Fetch::Network("no window".into()))?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(WASM))
        .await
        .map_err(|e| Fetch::Network(js(e)))?
        .dyn_into()
        .map_err(|e| Fetch::Network(js(e)))?;
    if !response.ok() {
        return Err(Fetch::Server(response.status()));
    }
    let body = response
        .body()
        .ok_or_else(|| Fetch::Network("no body".into()))?;
    let reader: web_sys::ReadableStreamDefaultReader = body
        .get_reader()
        .dyn_into()
        .map_err(|e| Fetch::Network(js(e.into())))?;
    let mut bytes: Vec<u8> = Vec::with_capacity(WASM_BYTES as usize);
    let mut hasher = Sha256::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = reader.cancel();
            return Err(Fetch::Cancelled);
        }
        let chunk = JsFuture::from(reader.read())
            .await
            .map_err(|e| Fetch::Network(js(e)))?;
        let done = js_sys::Reflect::get(&chunk, &"done".into())
            .map(|d| d.is_truthy())
            .unwrap_or(true);
        if done {
            break;
        }
        let value: js_sys::Uint8Array = js_sys::Reflect::get(&chunk, &"value".into())
            .map_err(|e| Fetch::Network(js(e)))?
            .dyn_into()
            .map_err(|e| Fetch::Network(js(e)))?;
        let start = bytes.len();
        bytes.resize(start + value.length() as usize, 0);
        value.copy_to(&mut bytes[start..]);
        hasher.update(&bytes[start..]);
        if bytes.len() as u64 > WASM_BYTES {
            return Err(Fetch::Checksum);
        }
        progress(bytes.len() as u64, WASM_BYTES);
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if bytes.len() as u64 != WASM_BYTES || digest != WASM_SHA256 {
        return Err(Fetch::Checksum);
    }
    let storage = |e: JsValue| Fetch::Storage(js(e));
    let init = web_sys::ResponseInit::new();
    let headers = web_sys::Headers::new().map_err(storage)?;
    headers
        .set("Content-Type", "application/wasm")
        .map_err(storage)?;
    init.set_headers(&headers);
    let stored = web_sys::Response::new_with_opt_u8_array_and_init(Some(&mut bytes), &init)
        .map_err(storage)?;
    let cache = open().await.map_err(Fetch::Storage)?;
    JsFuture::from(cache.put_with_str(WASM, &stored))
        .await
        .map_err(storage)?;
    drop(bytes);
    probe().await;
    if worker_script().is_none() {
        return Err(Fetch::Storage("the browser did not keep the engine".into()));
    }
    Ok(())
}

/// Removes the downloaded engine from the browser.
pub fn remove() {
    if let Some(url) = BLOB.with_borrow_mut(Option::take) {
        let _ = web_sys::Url::revoke_object_url(&url);
    }
    if let Ok(c) = caches() {
        let _ = c.delete(CACHE);
    }
}

/// Whether the engine is downloaded and ready.
pub fn ready() -> bool {
    BLOB.with_borrow(Option::is_some)
}

/// Engines working at once for a game review: one per logical processor, as on a
/// computer, at most `limit` (each worker holds its own copy of the engine, ≈ 130 MB).
pub fn workers(limit: usize) -> usize {
    let cpus = web_sys::window().map_or(1.0, |w| w.navigator().hardware_concurrency()) as usize;
    cpus.clamp(1, limit)
}
