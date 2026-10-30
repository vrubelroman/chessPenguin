//! Opening a database without holding the page: a Web Worker runs a second instance of
//! the program (`import-worker.js` → [`answer`]) that reads the file — PGN, CBV, ZIP,
//! 7Z, converted and indexed — and gives back its text and index; the page puts the
//! database together from them without reading it again
//! (`chess_core::PgnDatabase::from_index`); the cards of the shelf only ask it how many
//! games a file holds. One worker serves the visit and reads the files one after
//! another, in the order they were given.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chess_core::{PgnDatabase, PgnIndex};
use chess_import::{Database, Format, ImportError, Opened, PgnDb, Unsupported};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The worker's script, beside the page.
const SCRIPT: &str = "import-worker.js";

/// What the worker gives back besides the text.
#[derive(Serialize, Deserialize)]
struct Read {
    index: PgnIndex,
    /// Its place in [`Format::ALL`].
    format: usize,
    notes: Vec<String>,
}

/// Why the worker could not read a file: [`ImportError`] in serde's terms.
#[derive(Serialize, Deserialize)]
enum Failure {
    Io(String),
    Corrupt(String),
    Encrypted,
    ChessBase2,
    Empty,
    Cancelled,
}

impl Failure {
    fn of(e: ImportError) -> Failure {
        match e {
            ImportError::Io { source, .. } => Failure::Io(source.to_string()),
            ImportError::Corrupt(s) => Failure::Corrupt(s),
            ImportError::Unsupported(Unsupported::Encrypted) => Failure::Encrypted,
            ImportError::Unsupported(Unsupported::ChessBase2) => Failure::ChessBase2,
            ImportError::Empty => Failure::Empty,
            ImportError::Cancelled => Failure::Cancelled,
        }
    }

    fn error(self, path: &Path) -> ImportError {
        match self {
            Failure::Io(e) => io_error(path, e),
            Failure::Corrupt(s) => ImportError::Corrupt(s),
            Failure::Encrypted => ImportError::Unsupported(Unsupported::Encrypted),
            Failure::ChessBase2 => ImportError::Unsupported(Unsupported::ChessBase2),
            Failure::Empty => ImportError::Empty,
            Failure::Cancelled => ImportError::Cancelled,
        }
    }
}

fn io_error(path: &Path, e: impl ToString) -> ImportError {
    ImportError::Io {
        path: path.to_owned(),
        source: std::io::Error::other(e.to_string()),
    }
}

fn set(object: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(object, &key.into(), value);
}

/// The worker's side: reads `bytes` of the file `name`. The answer: `{text, index}`
/// (the text and the index in JSON), only `{games}` when `count` is asked, or `{error}`
/// (a [`Failure`] in JSON).
pub fn answer(name: &str, bytes: Vec<u8>, count: bool) -> JsValue {
    let never = std::sync::atomic::AtomicBool::new(false);
    let answer = js_sys::Object::new();
    let read = chess_import::open_bytes(name, bytes, &mut |_| {}, &never).and_then(|opened| {
        // Read from bytes, a database is always converted to PGN.
        let Database::Pgn(pgn) = opened.db else {
            return Err(ImportError::Corrupt("not a PGN database".into()));
        };
        Ok((pgn.db, opened.format, opened.notes))
    });
    match read {
        Ok((db, _, _)) if count => set(&answer, "games", &JsValue::from(db.len() as f64)),
        Ok((db, format, notes)) => {
            let (text, index) = db.into_index();
            let read = Read {
                index,
                format: Format::ALL.iter().position(|f| *f == format).unwrap_or(0),
                notes,
            };
            set(&answer, "text", &js_sys::Uint8Array::from(text.as_slice()));
            let json = serde_json::to_string(&read).unwrap_or_default();
            set(&answer, "index", &JsValue::from_str(&json));
        }
        Err(e) => {
            let json = serde_json::to_string(&Failure::of(e)).unwrap_or_default();
            set(&answer, "error", &JsValue::from_str(&json));
        }
    }
    answer.into()
}

/// Who waits for an answer.
enum Waiting {
    Open(PathBuf, Box<dyn FnOnce(Result<Opened, ImportError>)>),
    Count(Box<dyn FnOnce(Option<usize>)>),
}

impl Waiting {
    fn failed(self, e: impl ToString) {
        match self {
            Waiting::Open(path, done) => done(Err(io_error(&path, e))),
            Waiting::Count(done) => done(None),
        }
    }
}

thread_local! {
    static WORKER: RefCell<Option<web_sys::Worker>> = const { RefCell::new(None) };
    static WAITING: RefCell<HashMap<u32, Waiting>> = RefCell::new(HashMap::new());
    static NEXT: RefCell<u32> = const { RefCell::new(0) };
}

/// The worker's answer to a request: the database, or why there is none.
fn answered(data: JsValue) {
    let get = |k: &str| js_sys::Reflect::get(&data, &k.into()).unwrap_or(JsValue::UNDEFINED);
    let id = get("id").as_f64().unwrap_or(-1.0) as u32;
    let (path, done) = match WAITING.with_borrow_mut(|w| w.remove(&id)) {
        Some(Waiting::Open(path, done)) => (path, done),
        Some(Waiting::Count(done)) => {
            done(get("games").as_f64().map(|n| n as usize));
            return;
        }
        None => return,
    };
    if let Some(json) = get("error").as_string() {
        let e = serde_json::from_str::<Failure>(&json)
            .map_or_else(|_| io_error(&path, json.clone()), |f| f.error(&path));
        done(Err(e));
        return;
    }
    let text = get("text")
        .dyn_into::<js_sys::Uint8Array>()
        .map(|a| a.to_vec());
    let read = get("index")
        .as_string()
        .and_then(|j| serde_json::from_str::<Read>(&j).ok());
    let (Ok(text), Some(read)) = (text, read) else {
        done(Err(io_error(
            &path,
            "the reader's answer is not a database",
        )));
        return;
    };
    done(Ok(Opened {
        db: Database::Pgn(PgnDb::new(PgnDatabase::from_index(text, read.index))),
        format: Format::ALL.get(read.format).copied().unwrap_or(Format::Pgn),
        notes: read.notes,
    }))
}

fn worker() -> Result<web_sys::Worker, JsValue> {
    if let Some(w) = WORKER.with_borrow(|w| w.clone()) {
        return Ok(w);
    }
    let options = web_sys::WorkerOptions::new();
    options.set_type(web_sys::WorkerType::Module);
    let w = web_sys::Worker::new_with_options(SCRIPT, &options)?;
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(|e: web_sys::MessageEvent| {
            answered(e.data());
        });
    w.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    // The worker serves the whole visit.
    on_message.forget();
    let on_error = Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(|e: web_sys::ErrorEvent| {
        // The worker itself failed (out of memory, its script not loaded): the files
        // waiting for it fail, and the next one gets a new worker.
        let message = e.message();
        if let Some(w) = WORKER.with_borrow_mut(Option::take) {
            w.terminate();
        }
        for (_, waiting) in WAITING.with_borrow_mut(std::mem::take) {
            waiting.failed(&message);
        }
    });
    w.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();
    WORKER.with_borrow_mut(|slot| *slot = Some(w.clone()));
    Ok(w)
}

/// Reads the page's file `path` in the worker; `done` gets the database on the page's
/// thread.
pub fn open(path: &Path, done: impl FnOnce(Result<Opened, ImportError>) + 'static) {
    ask(path, false, Waiting::Open(path.to_owned(), Box::new(done)));
}

/// How many games the page's file `path` holds (`None`: it cannot be read), read in
/// the worker; `done` gets it on the page's thread.
pub fn count(path: &Path, done: impl FnOnce(Option<usize>) + 'static) {
    ask(path, true, Waiting::Count(Box::new(done)));
}

fn ask(path: &Path, count: bool, waiting: Waiting) {
    let bytes = match super::vfs::read(path) {
        Ok(b) => b,
        Err(e) => return waiting.failed(e),
    };
    let w = match worker() {
        Ok(w) => w,
        Err(e) => return waiting.failed(format!("{e:?}")),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = NEXT.with_borrow_mut(|n| {
        *n += 1;
        *n
    });
    WAITING.with_borrow_mut(|m| m.insert(id, waiting));
    let array = js_sys::Uint8Array::from(bytes.as_slice());
    drop(bytes);
    let message = js_sys::Object::new();
    set(&message, "id", &JsValue::from(id));
    set(&message, "name", &JsValue::from_str(&name));
    set(&message, "bytes", &array);
    set(&message, "count", &JsValue::from_bool(count));
    // The copy goes to the worker, not copied again.
    let transfer = js_sys::Array::of1(&array.buffer());
    if let Err(e) = w.post_message_with_transfer(&message, &transfer)
        && let Some(waiting) = WAITING.with_borrow_mut(|m| m.remove(&id))
    {
        waiting.failed(format!("{e:?}"));
    }
}
