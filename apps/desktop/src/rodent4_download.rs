//! Downloading Rodent IV on request (docs/decisions.md «Rodent IV»): its authors publish
//! no builds for Linux and Windows, so the engine is built from their sources at a pinned
//! commit by our workflow (.github/workflows/rodent-iv.yml) and published as a release of
//! its own; its personalities and opening books come from the authors' repository at the
//! same commit. Every file is checked against its SHA-256. Nothing is downloaded without
//! the user asking.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::engine_download::{Asset, Download, Failure, Stage, disk, fetch};
use crate::rodent4_files::{COMMIT, FILES};

/// The release of our builds.
const TAG: &str = "rodent-iv-0.33";

/// This platform's build in the release, if there is one (checked against its SHA-256).
fn binary() -> Option<Asset> {
    let (file, sha256, size) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "rodent-iv-linux-x86_64",
            "bb2cd533d89d6431e50a101242a74806d5c022f4d5893fe0d16dbd24b2f62efd",
            1_291_352,
        ),
        ("windows", "x86_64") => (
            "rodent-iv-windows-x86_64.exe",
            "aa0f58ee10a2366cf86064d152a29ab344faf02e1d199934232eb98f813aac4b",
            1_288_192,
        ),
        ("macos", "aarch64") => (
            "rodent-iv-macos-arm64",
            "c1abd5cc7970427f34632d8d3b7337e2c17499d182ab3f9be8e625fb3535c135",
            248_920,
        ),
        _ => return None,
    };
    (!sha256.is_empty()).then_some(Asset {
        file,
        sha256,
        size,
        zip: false,
    })
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
    binary().map_or(0, |b| b.size) + FILES.iter().map(|f| f.1).sum::<u64>()
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

/// Written last, once the engine started from its folder with everything beside it.
const COMPLETE: &str = ".complete";

/// The installed engine, if it is all there.
pub fn installed() -> Option<PathBuf> {
    let d = dir()?;
    let exe = d.join(EXE);
    let whole = d.join("personalities").join("basic.ini").is_file();
    let marked = std::env::var_os("CP_RODENT4").is_some() || d.join(COMPLETE).is_file();
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

/// Books three personalities name by a path not in the authors' files (a case or a dash
/// off): a copy goes under that name, so that they open their book (the engine takes
/// books only from the personality's file). (file in the repository, name it is read by)
const BOOK_NAMES: &[(&str, &str)] = &[
    ("books/guide/Strangler.bin", "books/guide/strangler.bin"),
    (
        "books/players/ph-spassky2.bin",
        "books/players/ph_spassky2.bin",
    ),
    ("books/guide.bin", "books/guide/guide.bin"),
];

/// A path of the repository as it goes into a URL («#readme.txt» has a «#»).
fn url_path(path: &str) -> String {
    path.replace('#', "%23").replace(' ', "%20")
}

fn run(report: &dyn Fn(Stage), cancel: &AtomicBool) -> Result<PathBuf, Failure> {
    let bin = binary().ok_or(Failure::Unsupported)?;
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
            "https://github.com/vrubelroman/chessPenguin/releases/download/{TAG}/{}",
            bin.file
        );
        fetch(&url, &exe, &bin, &step(base), cancel)?;
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
            let url = format!(
                "https://raw.githubusercontent.com/nescitus/rodent-iv/{COMMIT}/{}",
                url_path(path)
            );
            fetch(&url, &dest, &asset, &step(base), cancel)?;
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
        std::fs::write(dir.join(COMPLETE), TAG).map_err(disk)?;
        Ok(exe)
    })();
    if result.is_err() {
        // Nothing half-installed is left behind.
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_where_the_engine_looks_for_it() {
        assert_eq!(COMMIT.len(), 40);
        assert!(FILES.iter().any(|f| f.0 == "personalities/basic.ini"));
        assert!(FILES.iter().any(|f| f.0 == "personalities/tal.txt"));
        assert!(FILES.iter().any(|f| f.0 == "books/rodent.bin"));
        assert!(FILES.iter().all(|f| f.2.len() == 64 && !f.0.contains("..")));
        assert_eq!(url_path("books/#readme.txt"), "books/%23readme.txt");
        // The books copied under the names the personalities use are downloaded.
        for (file, _) in BOOK_NAMES {
            assert!(FILES.iter().any(|f| f.0 == *file), "{file}");
        }
    }
}
