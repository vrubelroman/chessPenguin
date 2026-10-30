//! Databases a Chromium browser remembers (Chrome, Edge: the File System Access API):
//! files chosen through `showOpenFilePicker`, or a database's folder chosen through
//! `showDirectoryPicker`, leave handles the page keeps in the browser's storage; at the
//! next visit the database is there again through them — at once when the browser still
//! allows reading, else after the user's «Allow reading» (the browser's own question).
//! Only reading: the page never writes the user's files (decisions «Веб-версия»).
//! Firefox and Safari have no such handles: there the user chooses the file again
//! ([`super::page::pick_files`]).
//!
//! What is kept for a database (its path on the page): `{handles: [file handles]}`, its
//! own file first, or `{dir: folder handle, names: [its files' names]}`; a bare file
//! handle (an earlier version) is a database of one file.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::platform::files::{Picked, gather};

thread_local! {
    /// Databases of earlier visits the browser asks about first: the page's path, what
    /// is kept for it.
    static WAITING: RefCell<HashMap<PathBuf, JsValue>> = RefCell::new(HashMap::new());
    /// The handle of a `.cbh` chosen alone: the folder's dialog opens where it is.
    static LONE_CBH: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

fn get(object: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn set(object: &JsValue, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(object, &key.into(), value);
}

/// Calls `object.method(arg)` (its promise, not awaited): within the user's gesture.
fn start(object: &JsValue, method: &str, arg: &JsValue) -> Result<js_sys::Promise, JsValue> {
    let f: js_sys::Function = get(object, method).dyn_into()?;
    f.call1(object, arg)?.dyn_into()
}

/// Calls `object.method(arg)` and waits for the promise it gives.
async fn call(object: &JsValue, method: &str, arg: &JsValue) -> Result<JsValue, JsValue> {
    wasm_bindgen_futures::JsFuture::from(start(object, method, arg)?).await
}

/// `{mode: "read"}`: the permission asked for.
fn read_mode() -> JsValue {
    let o = js_sys::Object::new();
    set(&o, "mode", &"read".into());
    o.into()
}

/// The browser remembers chosen files (it has `showOpenFilePicker`).
pub fn supported() -> bool {
    web_sys::window().is_some_and(|w| get(&w, "showOpenFilePicker").is_function())
}

/// The browser remembers a chosen folder (it has `showDirectoryPicker`).
pub fn supported_folder() -> bool {
    web_sys::window().is_some_and(|w| get(&w, "showDirectoryPicker").is_function())
}

async fn file_of(handle: &JsValue) -> Result<web_sys::File, JsValue> {
    call(handle, "getFile", &JsValue::UNDEFINED)
        .await?
        .dyn_into()
}

/// Opens a dialog of the window (`showOpenFilePicker`, `showDirectoryPicker`) now,
/// within the user's gesture.
fn dialog(method: &str, options: &js_sys::Object) -> Result<js_sys::Promise, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    start(&window, method, options)
}

/// Asks the user for files (the browser's dialog, `extensions` like `.pgn`); `then`
/// gets the databases they make, each remembered. Must run soon after the user's click
/// or key, like [`super::page::pick_files`].
pub fn pick(extensions: &[String], then: impl FnOnce(Picked) + 'static) {
    let accept = js_sys::Object::new();
    let list = extensions
        .iter()
        .map(|e| JsValue::from_str(e))
        .collect::<js_sys::Array>();
    set(&accept, "application/octet-stream", &list);
    let kind = js_sys::Object::new();
    set(&kind, "accept", &accept);
    let options = js_sys::Object::new();
    set(&options, "multiple", &JsValue::TRUE);
    set(&options, "types", &js_sys::Array::of1(&kind));
    set(&options, "id", &"chesspenguin-databases".into());
    let chosen = dialog("showOpenFilePicker", &options);
    wasm_bindgen_futures::spawn_local(async move {
        // Cancelled by the user: an error, nothing chosen.
        let handles: Vec<JsValue> = match chosen {
            Ok(p) => wasm_bindgen_futures::JsFuture::from(p)
                .await
                .and_then(|h| h.dyn_into::<js_sys::Array>())
                .map(|a| a.to_vec())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let mut files = Vec::new();
        let mut kept_handles = Vec::new();
        for h in handles {
            match file_of(&h).await {
                Ok(f) => {
                    files.push(f);
                    kept_handles.push(h);
                }
                Err(e) => log::warn!("cannot read a chosen file: {e:?}"),
            }
        }
        let (picked, kept) = gather(&files, false);
        for (path, members) in kept {
            let entry = js_sys::Object::new();
            let list: js_sys::Array = members.iter().map(|&i| kept_handles[i].clone()).collect();
            set(&entry, "handles", &list);
            super::idb::put_handle(&path.to_string_lossy(), &entry);
        }
        // A .cbh alone: the folder's dialog will open beside it.
        let lone = picked.incomplete.first().and_then(|name| {
            files
                .iter()
                .position(|f| f.name() == *name)
                .map(|i| kept_handles[i].clone())
        });
        LONE_CBH.with_borrow_mut(|l| *l = lone);
        then(picked);
    });
}

/// Asks the user for a database's folder; `then` gets the databases in it (the files
/// right in it, found by their names), each remembered with the folder.
pub fn pick_folder(then: impl FnOnce(Picked) + 'static) {
    let options = js_sys::Object::new();
    set(&options, "id", &"chesspenguin-databases".into());
    set(&options, "mode", &"read".into());
    if let Some(cbh) = LONE_CBH.with_borrow_mut(Option::take) {
        set(&options, "startIn", &cbh);
    }
    let chosen = dialog("showDirectoryPicker", &options);
    wasm_bindgen_futures::spawn_local(async move {
        let dir = match chosen {
            Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok(),
            Err(_) => None,
        };
        let Some(dir) = dir else {
            return then(Picked::default());
        };
        let mut files = Vec::new();
        if let Err(e) = folder_files(&dir, &mut files).await {
            log::warn!("cannot read the folder: {e:?}");
        }
        let (mut picked, kept) = gather(&files, true);
        for (path, members) in kept {
            let entry = js_sys::Object::new();
            set(&entry, "dir", &dir);
            let names: js_sys::Array = members
                .iter()
                .map(|&i| JsValue::from_str(&files[i].name()))
                .collect();
            set(&entry, "names", &names);
            super::idb::put_handle(&path.to_string_lossy(), &entry);
        }
        picked.empty_folder =
            picked.paths.is_empty() && picked.too_big.is_empty() && picked.incomplete.is_empty();
        then(picked);
    });
}

/// The files right in the folder `dir` (its `values()`, an asynchronous iterator).
async fn folder_files(dir: &JsValue, out: &mut Vec<web_sys::File>) -> Result<(), JsValue> {
    let entries = js_sys::Reflect::apply(
        &get(dir, "values").dyn_into::<js_sys::Function>()?,
        dir,
        &js_sys::Array::new(),
    )?;
    loop {
        let step = call(&entries, "next", &JsValue::UNDEFINED).await?;
        if get(&step, "done").is_truthy() {
            return Ok(());
        }
        let handle = get(&step, "value");
        if get(&handle, "kind").as_string().as_deref() == Some("file") {
            out.push(file_of(&handle).await?);
        }
    }
}

/// What the browser is asked to let the page read: the folder, or the files.
fn targets(kept: &JsValue) -> Vec<JsValue> {
    let dir = get(kept, "dir");
    if !dir.is_undefined() {
        return vec![dir];
    }
    match get(kept, "handles").dyn_into::<js_sys::Array>() {
        Ok(list) => list.to_vec(),
        // A bare file handle.
        Err(_) => vec![kept.clone()],
    }
}

/// The database's files through what is kept for it, its own first.
async fn files_of(kept: &JsValue) -> Result<Vec<web_sys::File>, JsValue> {
    let dir = get(kept, "dir");
    let mut files = Vec::new();
    if dir.is_undefined() {
        for h in targets(kept) {
            files.push(file_of(&h).await?);
        }
    } else {
        let names: js_sys::Array = get(kept, "names").dyn_into()?;
        for name in names.iter() {
            files.push(file_of(&call(&dir, "getFileHandle", &name).await?).await?);
        }
    }
    Ok(files)
}

/// Brings back the databases remembered at earlier visits that the browser still lets
/// the page read (nothing is read now: the page's reader reads them when opened); the
/// others wait for the user ([`waiting`], [`allow`]). Called once before the
/// interface starts.
pub async fn restore() {
    if !supported() {
        return;
    }
    for (path, kept) in super::idb::handles().await {
        let path = PathBuf::from(path);
        let mut granted = true;
        for t in targets(&kept) {
            let state = call(&t, "queryPermission", &read_mode())
                .await
                .ok()
                .and_then(|s| s.as_string());
            granted &= state.as_deref() == Some("granted");
        }
        if !granted {
            WAITING.with_borrow_mut(|w| w.insert(path, kept));
            continue;
        }
        match files_of(&kept).await {
            Ok(files) => {
                log::info!("remembered file read again: {}", path.display());
                super::vfs::put_opened(&path, files);
            }
            Err(e) => log::warn!("a remembered file is gone: {e:?}"),
        }
    }
}

/// The browser remembers the database `path` and asks the user before it is read again.
pub fn waiting(path: &Path) -> bool {
    WAITING.with_borrow(|w| w.contains_key(path))
}

/// Asks the browser to let the page read `path` again (its question to the user; soon
/// after the user's click). `then`: whether the database is back at its path.
pub fn allow(path: &Path, then: impl FnOnce(bool) + 'static) {
    let Some(kept) = WAITING.with_borrow(|w| w.get(path).cloned()) else {
        then(false);
        return;
    };
    let path = path.to_owned();
    // Asked now, all within the user's gesture.
    let asked: Vec<_> = targets(&kept)
        .iter()
        .map(|t| start(t, "requestPermission", &read_mode()))
        .collect();
    wasm_bindgen_futures::spawn_local(async move {
        let mut granted = true;
        for a in asked {
            granted &= match a {
                Ok(p) => wasm_bindgen_futures::JsFuture::from(p)
                    .await
                    .ok()
                    .and_then(|s| s.as_string())
                    .is_some_and(|s| s == "granted"),
                Err(_) => false,
            };
        }
        let files = if granted {
            files_of(&kept).await.ok()
        } else {
            None
        };
        let back = files.is_some();
        if let Some(files) = files {
            log::info!("remembered file allowed again: {}", path.display());
            WAITING.with_borrow_mut(|w| w.remove(&path));
            super::vfs::put_opened(&path, files);
        }
        then(back);
    });
}

/// The database `path` is no longer the library's: the browser's handles are let go.
pub fn forget(path: &Path) {
    WAITING.with_borrow_mut(|w| w.remove(path));
    super::idb::delete_handle(&path.to_string_lossy());
}
