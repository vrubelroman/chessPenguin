//! Downloading the Maia-3 weights on request: a published checkpoint of a pinned
//! revision from huggingface.co/UofTCSSLab (the large `maia3-79m.pt`; the small
//! `maia3-5m.pt` of earlier versions is still used when it is there), checked against
//! its SHA-256 and loaded once before use. Nothing is downloaded without the user
//! asking; the file is the authors' own, not redistributed by us.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

pub use maia::strength::Model;

/// Where each model is published.
pub trait Published {
    fn repo(self) -> &'static str;
    fn revision(self) -> &'static str;
    fn file(self) -> &'static str;
    fn sha256(self) -> &'static str;
    fn size(self) -> u64;
}

impl Published for Model {
    fn repo(self) -> &'static str {
        match self {
            Model::Small => "Maia3-5M",
            Model::Large => "Maia3-79M",
        }
    }

    fn revision(self) -> &'static str {
        match self {
            Model::Small => "b6559de2398d7140b985f28fd2c19fb5e47ddabe",
            Model::Large => "a107d6ceb7b298cb04ae1da4edffe2939858b894",
        }
    }

    fn file(self) -> &'static str {
        match self {
            Model::Small => "maia3-5m.pt",
            Model::Large => "maia3-79m.pt",
        }
    }

    fn sha256(self) -> &'static str {
        match self {
            Model::Small => "ba14208b2992d85502f5fb501934abf6aaaeb355e9f3fdf90e326911f562524f",
            Model::Large => "3fc6181d5db789b45a15305732148757ae74efa3e0028e81ba335b462dac45c2",
        }
    }

    fn size(self) -> u64 {
        match self {
            Model::Small => 20_968_049,
            Model::Large => 315_651_851,
        }
    }
}

/// `<local data dir>/maia/<file>`.
fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("maia"))
}

/// The weights of `model`, when they are there. `CP_MAIA` (development) points to a
/// file elsewhere: the large model when its name says 79m, else the small one.
pub fn installed(model: Model) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CP_MAIA") {
        let large = p.to_string_lossy().contains("79m");
        let p = PathBuf::from(p);
        return (large == (model == Model::Large) && p.is_file()).then_some(p);
    }
    let p = dir()?.join(model.file());
    p.is_file().then_some(p)
}

/// The downloaded weights did not load: removed, so that the dialog offers them again.
pub fn forget_broken(model: Model) {
    if std::env::var_os("CP_MAIA").is_none()
        && let Some(d) = dir()
    {
        let _ = std::fs::remove_file(d.join(model.file()));
    }
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
    pub model: Model,
    stage: Arc<Mutex<Stage>>,
    cancel: Arc<AtomicBool>,
}

impl Download {
    /// A browser downloads no weights into files: there is no download for it.
    #[cfg(web)]
    pub fn start(_ctx: &egui::Context, model: Model) -> Download {
        Download {
            model,
            stage: Arc::new(Mutex::new(Stage::Failed(
                crate::i18n::tr("not yet for this system").to_owned(),
            ))),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(not(web))]
    pub fn start(ctx: &egui::Context, model: Model) -> Download {
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
                let end = match run(model, &report, &c) {
                    Ok(p) => Stage::Done(p),
                    Err(e) => Stage::Failed(e),
                };
                report(end);
            });
        Download {
            model,
            stage,
            cancel,
        }
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

#[cfg(not(web))]
fn run(model: Model, report: &dyn Fn(Stage), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let dir = dir().ok_or("no data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = model.file();
    let part = dir.join(format!("{name}.part"));
    let url = format!(
        "https://huggingface.co/UofTCSSLab/{}/resolve/{}/{name}",
        model.repo(),
        model.revision()
    );
    let result = (|| {
        let mut response = ureq::get(&url).call().map_err(|e| e.to_string())?;
        let mut body = response
            .body_mut()
            .with_config()
            .limit(model.size() + 1)
            .reader();
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
        if done != model.size() || digest != model.sha256() {
            return Err(crate::i18n::tr("The file is not the published one").to_owned());
        }
        report(Stage::Checking);
        maia::Maia::load(&part).map_err(|e| e.to_string())?;
        let dest = dir.join(name);
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        Ok(dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}
