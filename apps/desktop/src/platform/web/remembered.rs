//! Files a Chromium browser remembers (Chrome, Edge: the File System Access API): a
//! file chosen through `showOpenFilePicker` leaves a handle the page keeps in the
//! browser's storage; at the next visit the file is read again through it — at once
//! when the browser still allows it, else after the user's «Allow reading» (the
//! browser's own question). Only reading: the page never writes the user's files
//! (decisions «Веб-версия»). Firefox and Safari have no such handles: there the user
//! chooses the file again ([`super::page::pick_files`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::page::Read;

thread_local! {
    /// Files of earlier visits the browser asks about first: the page's path, the handle.
    static WAITING: RefCell<HashMap<PathBuf, JsValue>> = RefCell::new(HashMap::new());
}

fn get(object: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

/// Calls `object.method(arg)` and waits for the promise it gives.
async fn call(object: &JsValue, method: &str, arg: &JsValue) -> Result<JsValue, JsValue> {
    let f: js_sys::Function = get(object, method).dyn_into()?;
    let promise: js_sys::Promise = f.call1(object, arg)?.dyn_into()?;
    wasm_bindgen_futures::JsFuture::from(promise).await
}

/// `{mode: "read"}`: the permission asked for.
fn read_mode() -> JsValue {
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&o, &"mode".into(), &"read".into());
    o.into()
}

/// The browser remembers chosen files (it has `showOpenFilePicker`).
pub fn supported() -> bool {
    web_sys::window().is_some_and(|w| get(&w, "showOpenFilePicker").is_function())
}

/// Reads the file behind `handle` (`None`: too big, its name and size go to `read`).
async fn read_file(handle: &JsValue, read: &mut Read) -> Result<Option<Vec<u8>>, JsValue> {
    let file: web_sys::File = call(handle, "getFile", &JsValue::UNDEFINED)
        .await?
        .dyn_into()?;
    let size = file.size() as u64;
    if size > crate::platform::files::TOO_BIG {
        read.too_big.push((file.name(), size));
        return Ok(None);
    }
    let buf = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await?;
    Ok(Some(js_sys::Uint8Array::new(&buf).to_vec()))
}

/// Asks the user for files (the browser's dialog, `extensions` like `.pgn`); `then`
/// gets the paths of those read, each remembered. Must run soon after the user's click
/// or key, like [`super::page::pick_files`].
pub fn pick(extensions: &[String], then: impl FnOnce(Vec<PathBuf>, Vec<(String, u64)>) + 'static) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let types = js_sys::Array::new();
    let accept = js_sys::Object::new();
    let list = extensions
        .iter()
        .map(|e| JsValue::from_str(e))
        .collect::<js_sys::Array>();
    let _ = js_sys::Reflect::set(&accept, &"application/octet-stream".into(), &list);
    let kind = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&kind, &"accept".into(), &accept);
    types.push(&kind);
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"multiple".into(), &JsValue::TRUE);
    let _ = js_sys::Reflect::set(&options, &"types".into(), &types);
    let _ = js_sys::Reflect::set(&options, &"id".into(), &"chesspenguin-databases".into());
    // The dialog is opened now, within the user's gesture; the rest waits for it.
    let chosen = get(&window, "showOpenFilePicker")
        .dyn_into::<js_sys::Function>()
        .and_then(|f| f.call1(&window, &options))
        .and_then(|p| p.dyn_into::<js_sys::Promise>());
    wasm_bindgen_futures::spawn_local(async move {
        let mut read = Read {
            read: Vec::new(),
            too_big: Vec::new(),
        };
        let mut paths = Vec::new();
        // Cancelled by the user: an error, nothing chosen.
        let handles = match chosen {
            Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await,
            Err(e) => Err(e),
        };
        if let Ok(handles) = handles.and_then(|h| h.dyn_into::<js_sys::Array>()) {
            for handle in handles.iter() {
                let name = get(&handle, "name").as_string().unwrap_or_default();
                match read_file(&handle, &mut read).await {
                    Ok(Some(bytes)) => {
                        let path = super::vfs::add_opened(&name, bytes);
                        super::idb::put_handle(&path.to_string_lossy(), &handle);
                        paths.push(path);
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("cannot read {name}: {e:?}"),
                }
            }
        }
        then(paths, read.too_big);
    });
}

/// Reads again the files remembered at earlier visits that the browser still lets the
/// page read; the others wait for the user ([`waiting`], [`allow`]). Called once
/// before the interface starts.
pub async fn restore() {
    if !supported() {
        return;
    }
    for (path, handle) in super::idb::handles().await {
        let path = PathBuf::from(path);
        let state = call(&handle, "queryPermission", &read_mode())
            .await
            .ok()
            .and_then(|s| s.as_string());
        if state.as_deref() == Some("granted")
            && let Ok(Some(bytes)) = read_file(
                &handle,
                &mut Read {
                    read: Vec::new(),
                    too_big: Vec::new(),
                },
            )
            .await
        {
            log::info!("remembered file read again: {}", path.display());
            super::vfs::put_opened(&path, bytes);
        } else {
            WAITING.with_borrow_mut(|w| w.insert(path, handle));
        }
    }
}

/// The browser remembers the file `path` and asks the user before it is read again.
pub fn waiting(path: &Path) -> bool {
    WAITING.with_borrow(|w| w.contains_key(path))
}

/// Asks the browser to let the page read `path` again (its question to the user; soon
/// after the user's click). `then`: whether the file is read and back at its path.
pub fn allow(path: &Path, then: impl FnOnce(bool) + 'static) {
    let Some(handle) = WAITING.with_borrow(|w| w.get(path).cloned()) else {
        then(false);
        return;
    };
    let path = path.to_owned();
    // Asked now, within the user's gesture.
    let asked = get(&handle, "requestPermission")
        .dyn_into::<js_sys::Function>()
        .and_then(|f| f.call1(&handle, &read_mode()))
        .and_then(|p| p.dyn_into::<js_sys::Promise>());
    wasm_bindgen_futures::spawn_local(async move {
        let granted = match asked {
            Ok(p) => wasm_bindgen_futures::JsFuture::from(p)
                .await
                .ok()
                .and_then(|s| s.as_string())
                .is_some_and(|s| s == "granted"),
            Err(_) => false,
        };
        let mut read = Read {
            read: Vec::new(),
            too_big: Vec::new(),
        };
        let bytes = if granted {
            read_file(&handle, &mut read).await.ok().flatten()
        } else {
            None
        };
        let back = bytes.is_some();
        if let Some(bytes) = bytes {
            WAITING.with_borrow_mut(|w| w.remove(&path));
            super::vfs::put_opened(&path, bytes);
        }
        then(back);
    });
}

/// The file `path` is no longer the library's: the browser's handle is let go.
pub fn forget(path: &Path) {
    WAITING.with_borrow_mut(|w| w.remove(path));
    super::idb::delete_handle(&path.to_string_lossy());
}
