//! The page around the program: choosing files, downloads, the next task.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The elements through which files and a folder are chosen (one each for the page;
/// checks give them files).
pub const OPEN_INPUT_ID: &str = "cp-open-files";
pub const OPEN_FOLDER_ID: &str = "cp-open-folder";
/// The program's canvas (`page/index.html`).
const CANVAS_ID: &str = "cp-canvas";

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

type Picked = Box<dyn FnOnce(Vec<web_sys::File>)>;

thread_local! {
    /// What to do with the files chosen next.
    static ON_PICKED: RefCell<Option<Picked>> = const { RefCell::new(None) };
}

/// The page's file input `id`, made once: hidden, several files, `accept` the
/// extensions; `folder`: it chooses a folder (`webkitdirectory`, every browser has it).
fn open_input(id: &str, accept: &str, folder: bool) -> Result<web_sys::HtmlInputElement, JsValue> {
    let doc = document().ok_or("no document")?;
    if let Some(el) = doc.get_element_by_id(id) {
        let input: web_sys::HtmlInputElement = el.dyn_into()?;
        input.set_accept(accept);
        return Ok(input);
    }
    let input: web_sys::HtmlInputElement = doc.create_element("input")?.dyn_into()?;
    input.set_id(id);
    input.set_type("file");
    input.set_multiple(true);
    input.set_accept(accept);
    if folder {
        input.set_attribute("webkitdirectory", "")?;
    }
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
        // The keys go to the program again, not to the hidden input.
        if let Some(canvas) = document()
            .and_then(|d| d.get_element_by_id(CANVAS_ID))
            .and_then(|c| c.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = canvas.focus();
        }
        let files = if folder {
            // The folder's own files: a database's folder, not what is below it.
            files
                .into_iter()
                .filter(|f| {
                    js_sys::Reflect::get(f, &"webkitRelativePath".into())
                        .ok()
                        .and_then(|p| p.as_string())
                        .is_none_or(|p| p.matches('/').count() <= 1)
                })
                .collect()
        } else {
            files
        };
        if let Some(then) = ON_PICKED.with_borrow_mut(Option::take) {
            then(files);
        }
    });
    input.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())?;
    // The input lives as long as the page.
    on_change.forget();
    doc.body().ok_or("no body")?.append_child(&input)?;
    Ok(input)
}

/// Asks the user for files (the browser's dialog); `then` gets them once chosen (not
/// read: the page's reader reads them). Must run soon after the user's click or key (a
/// browser opens the dialog only then: egui handles input in the next frame, well
/// within that time).
pub fn pick_files(accept: &str, then: impl FnOnce(Vec<web_sys::File>) + 'static) {
    pick(OPEN_INPUT_ID, accept, false, then);
}

/// Asks the user for a folder; `then` gets the files right in it.
pub fn pick_folder(then: impl FnOnce(Vec<web_sys::File>) + 'static) {
    pick(OPEN_FOLDER_ID, "", true, then);
}

fn pick(id: &str, accept: &str, folder: bool, then: impl FnOnce(Vec<web_sys::File>) + 'static) {
    match open_input(id, accept, folder) {
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
