//! The page around the program: choosing files, downloads, the next task.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The element through which files are chosen (one for the page; checks give it files).
pub const OPEN_INPUT_ID: &str = "cp-open-files";

fn document() -> Option<web_sys::Document> {
    web_sys::window()?.document()
}

/// Resolves after the tasks queued now: the page draws its frame first.
pub async fn next_task() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback(&resolve);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

type Picked = Box<dyn FnOnce(Read)>;

/// Files read: their names and contents, and the ones too big to read (name, bytes).
pub struct Read {
    pub read: Vec<(String, Vec<u8>)>,
    pub too_big: Vec<(String, u64)>,
}

thread_local! {
    /// What to do with the files chosen next.
    static ON_PICKED: RefCell<Option<Picked>> = const { RefCell::new(None) };
}

/// The page's file input, made once: hidden, several files, `accept` the extensions.
fn open_input(accept: &str) -> Result<web_sys::HtmlInputElement, JsValue> {
    let doc = document().ok_or("no document")?;
    if let Some(el) = doc.get_element_by_id(OPEN_INPUT_ID) {
        let input: web_sys::HtmlInputElement = el.dyn_into()?;
        input.set_accept(accept);
        return Ok(input);
    }
    let input: web_sys::HtmlInputElement = doc.create_element("input")?.dyn_into()?;
    input.set_id(OPEN_INPUT_ID);
    input.set_type("file");
    input.set_multiple(true);
    input.set_accept(accept);
    input.style().set_property("display", "none")?;
    let on_change = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        let Some(input) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let files: Vec<web_sys::File> = input
            .files()
            .map(|list| (0..list.length()).filter_map(|i| list.get(i)).collect())
            .unwrap_or_default();
        // The same file chosen again is a change too.
        input.set_value("");
        let Some(then) = ON_PICKED.with_borrow_mut(Option::take) else {
            return;
        };
        wasm_bindgen_futures::spawn_local(async move {
            then(read_files(files).await);
        });
    });
    input.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())?;
    // The input lives as long as the page.
    on_change.forget();
    doc.body().ok_or("no body")?.append_child(&input)?;
    Ok(input)
}

/// The contents of `files`, with their names (a file that cannot be read is left
/// out; one larger than [`crate::platform::files::TOO_BIG`] is not read).
pub async fn read_files(files: Vec<web_sys::File>) -> Read {
    let mut out = Read {
        read: Vec::with_capacity(files.len()),
        too_big: Vec::new(),
    };
    for f in files {
        let size = f.size() as u64;
        if size > crate::platform::files::TOO_BIG {
            out.too_big.push((f.name(), size));
            continue;
        }
        match wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await {
            Ok(buf) => out
                .read
                .push((f.name(), js_sys::Uint8Array::new(&buf).to_vec())),
            Err(e) => log::warn!("cannot read {}: {e:?}", f.name()),
        }
    }
    out
}

/// Asks the user for files (the browser's dialog); `then` gets their contents once
/// chosen. Must run soon after the user's click or key (a browser opens the dialog
/// only then: egui handles input in the next frame, well within that time).
pub fn pick_files(accept: &str, then: impl FnOnce(Read) + 'static) {
    match open_input(accept) {
        Ok(input) => {
            ON_PICKED.with_borrow_mut(|p| *p = Some(Box::new(then)));
            input.click();
        }
        Err(e) => log::warn!("cannot choose files: {e:?}"),
    }
}

/// Gives the user `bytes` as a downloaded file named `name`.
pub fn download(name: &str, mime: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let doc = document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let a: web_sys::HtmlAnchorElement = doc.create_element("a")?.dyn_into()?;
    a.set_href(&url);
    a.set_download(name);
    a.style().set_property("display", "none")?;
    let body = doc.body().ok_or("no body")?;
    body.append_child(&a)?;
    a.click();
    body.remove_child(&a)?;
    // The download has taken the data by the next task.
    wasm_bindgen_futures::spawn_local(async move {
        next_task().await;
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    Ok(())
}
