//! Downloading the Maia-3 weights on request: the published checkpoint `maia3-5m.pt`
//! of a pinned revision from huggingface.co/UofTCSSLab/Maia3-5M, checked against its
//! SHA-256 and loaded once before use. Nothing is downloaded without the user asking;
//! the file is the authors' own, not redistributed by us.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

const REVISION: &str = "b6559de2398d7140b985f28fd2c19fb5e47ddabe";
const FILE: &str = "maia3-5m.pt";
const SHA256: &str = "ba14208b2992d85502f5fb501934abf6aaaeb355e9f3fdf90e326911f562524f";
pub const SIZE: u64 = 20_968_049;

/// `<local data dir>/maia/maia3-5m.pt`.
fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("maia"))
}

/// The weights, when they are there (`CP_MAIA` points elsewhere: development).
pub fn installed() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CP_MAIA") {
        return Some(PathBuf::from(p));
    }
    let p = dir()?.join(FILE);
    p.is_file().then_some(p)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Downloading { done: u64 },
    Checking,
    Done(PathBuf),
    Failed(String),
}

/// A download in a background thread.
pub struct Download {
    stage: Arc<Mutex<Stage>>,
    cancel: Arc<AtomicBool>,
}

impl Download {
    pub fn start(ctx: &egui::Context) -> Download {
        let stage = Arc::new(Mutex::new(Stage::Downloading { done: 0 }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c, ctx) = (stage.clone(), cancel.clone(), ctx.clone());
        let _ = std::thread::Builder::new()
            .name("maia-download".into())
            .spawn(move || {
                let report = |st: Stage| {
                    if let Ok(mut g) = s.lock() {
                        *g = st;
                    }
                    ctx.request_repaint();
                };
                let end = match run(&report, &c) {
                    Ok(p) => Stage::Done(p),
                    Err(e) => Stage::Failed(e),
                };
                report(end);
            });
        Download { stage, cancel }
    }

    pub fn stage(&self) -> Stage {
        self.stage
            .lock()
            .map(|g| g.clone())
            .unwrap_or(Stage::Failed(String::new()))
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn run(report: &dyn Fn(Stage), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let dir = dir().ok_or("no data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{FILE}.part"));
    let url = format!("https://huggingface.co/UofTCSSLab/Maia3-5M/resolve/{REVISION}/{FILE}");
    let result = (|| {
        let mut response = ureq::get(&url).call().map_err(|e| e.to_string())?;
        let mut body = response.body_mut().with_config().limit(SIZE + 1).reader();
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_owned());
            }
            let n = body.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            report(Stage::Downloading { done });
        }
        file.sync_all().map_err(|e| e.to_string())?;
        let digest: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if done != SIZE || digest != SHA256 {
            return Err(crate::i18n::tr("The file is not the published one").to_owned());
        }
        report(Stage::Checking);
        maia::Maia::load(&part).map_err(|e| e.to_string())?;
        let dest = dir.join(FILE);
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        Ok(dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}
