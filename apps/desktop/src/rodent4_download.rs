//! Downloading Rodent IV on request (docs/decisions.md «Rodent IV»): its authors publish
//! no builds for Linux and Windows, so the engine is built from their sources at a pinned
//! commit by our workflow (.github/workflows/rodent-iv.yml) and published as a release of
//! its own; its personalities and opening books come from the authors' repository at the
//! same commit. Every file is checked against its SHA-256. Nothing is downloaded without
//! the user asking.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::engine_download::{Asset, Download, Failure, Stage, already, disk, fetch};
use ::play::rodent::{BOOK_NAMES, FILES, file_url};

/// This platform's build: the release of our builds it is in (the macOS build of
/// `rodent-iv-0.33` searched garbage, `think-thread.patch`; the one of `rodent-iv-0.33-2`
/// had no home folder and offered no personality, `home-dir.patch`; the GCC builds of
/// Linux and Windows had neither fault) and the file there, checked against its SHA-256.
fn binary() -> Option<(&'static str, Asset)> {
    let (tag, file, sha256, size) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "rodent-iv-0.33",
            "rodent-iv-linux-x86_64",
            "bb2cd533d89d6431e50a101242a74806d5c022f4d5893fe0d16dbd24b2f62efd",
            1_291_352,
        ),
        ("windows", "x86_64") => (
            "rodent-iv-0.33",
            "rodent-iv-windows-x86_64.exe",
            "aa0f58ee10a2366cf86064d152a29ab344faf02e1d199934232eb98f813aac4b",
            1_288_192,
        ),
        ("macos", "aarch64") => (
            "rodent-iv-0.33-3",
            "rodent-iv-macos-arm64",
            "4e84159598196e164400c056eff04baebcaf1a045a9b7aaf63ffabe98b9df6af",
            249_096,
        ),
        _ => return None,
    };
    (!sha256.is_empty()).then_some((
        tag,
        Asset {
            file,
            sha256,
            size,
            zip: false,
        },
    ))
}

#[cfg(windows)]
const EXE: &str = "rodent_iv.exe";
#[cfg(not(windows))]
const EXE: &str = "rodent_iv";

/// Whether Rodent IV can be had on this platform (or is at hand for development).
pub fn available() -> bool {
    std::env::var_os("CP_RODENT4").is_some() || binary().is_some()
}

/// The download in all: the engine, its personalities and books.
fn total() -> u64 {
    binary().map_or(0, |(_, b)| b.size) + FILES.iter().map(|f| f.1).sum::<u64>()
}

pub fn size_mb() -> u64 {
    (total() + 500_000) / 1_000_000
}

/// `<local data dir>/engines/rodent_iv_0.33/` (`CP_RODENT4` elsewhere: development).
pub fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CP_RODENT4") {
        return Some(PathBuf::from(d));
    }
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("engines").join("rodent_iv_0.33"))
}

/// Written last, once the engine started from its folder with everything beside it: the
/// release its build came from.
const COMPLETE: &str = ".complete";

/// The installed engine, if it is all there and its build is this version's (an older
/// one is downloaded again; the files that did not change are kept).
pub fn installed() -> Option<PathBuf> {
    let d = dir()?;
    let exe = d.join(EXE);
    let whole = d.join("personalities").join("basic.ini").is_file();
    let marked = std::env::var_os("CP_RODENT4").is_some()
        || binary().is_some_and(|(tag, _)| {
            std::fs::read_to_string(d.join(COMPLETE)).is_ok_and(|m| m.trim() == tag)
        });
    (exe.is_file() && whole && marked).then_some(exe)
}

/// Removes the downloaded engine (a broken one).
pub fn remove() -> std::io::Result<()> {
    if std::env::var_os("CP_RODENT4").is_some() {
        return Ok(());
    }
    match dir() {
        Some(d) if d.exists() => std::fs::remove_dir_all(d),
        _ => Ok(()),
    }
}

pub fn start(ctx: &egui::Context) -> Download {
    Download::spawn(ctx, total(), run)
}

fn run(report: &dyn Fn(Stage), cancel: &AtomicBool) -> Result<PathBuf, Failure> {
    let (tag, bin) = binary().ok_or(Failure::Unsupported)?;
    let dir = dir().ok_or_else(|| Failure::Disk("no data directory".into()))?;
    std::fs::create_dir_all(&dir).map_err(disk)?;
    let grand = total();
    let mut base = 0u64;
    // One file's progress as part of the whole.
    let step = |base: u64| {
        move |s: Stage| match s {
            Stage::Downloading { done, .. } => report(Stage::Downloading {
                done: base + done,
                total: grand,
            }),
            other => report(other),
        }
    };
    let result = (|| {
        let exe = dir.join(EXE);
        let url = format!(
            "https://github.com/vrubelroman/chessPenguin/releases/download/{tag}/{}",
            bin.file
        );
        if !already(&exe, &bin) {
            fetch(&url, &exe, &bin, &step(base), cancel)?;
        }
        base += bin.size;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(disk)?;
        }
        for &(path, size, sha256) in FILES {
            let dest = dir.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(disk)?;
            }
            let asset = Asset {
                file: path,
                sha256,
                size,
                zip: false,
            };
            if !already(&dest, &asset) {
                fetch(&file_url(path), &dest, &asset, &step(base), cancel)?;
            }
            base += size;
        }
        report(Stage::Unpacking);
        for (file, name) in BOOK_NAMES {
            // Where names do not differ by case (Windows, macOS) it is the same file.
            if !dir.join(name).exists() {
                std::fs::copy(dir.join(file), dir.join(name)).map_err(disk)?;
            }
        }
        crate::rodent_download::check_runs(&exe)?;
        std::fs::write(dir.join(COMPLETE), tag).map_err(disk)?;
        Ok(exe)
    })();
    if result.is_err() {
        // Nothing half-installed is left behind.
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}
