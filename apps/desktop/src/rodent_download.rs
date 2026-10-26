//! Downloading Rodent V on request (docs/decisions.md «Соперник с характером»): the
//! official release 1.2 of github.com/nescitus/Rodent-V (GPL-3.0), checked against its
//! SHA-256, unpacked into the user's data directory with its personalities, networks and
//! opening books (the engine finds them from its own folder). Linux and Windows on x86-64
//! only: the release has no other builds. Nothing is downloaded without the user asking.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::engine_download::{Asset, Download, Failure, Stage, disk, fetch};

const TAG: &str = "Rodent_v_1.2";

const ARCHIVE: Asset = Asset {
    file: "release-1-2.zip",
    sha256: "81c46c45d3f26393b375f6be03d60f4207bb79f4f924156e53fee6ac75a7be6d",
    size: 48_241_374,
    zip: true,
};

/// The engine's file in the archive for this platform, if there is one.
fn binary() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("rodent_v_1.2"),
        ("windows", "x86_64") => Some("rodent_v_1.2.exe"),
        _ => None,
    }
}

#[cfg(windows)]
const EXE: &str = "rodent_v.exe";
#[cfg(not(windows))]
const EXE: &str = "rodent_v";

/// Whether Rodent can be had on this platform.
pub fn available() -> bool {
    binary().is_some()
}

pub fn size_mb() -> u64 {
    ARCHIVE.size.div_ceil(1_000_000)
}

/// `<local data dir>/engines/rodent_v_1.2/` (`CP_RODENT` elsewhere: development).
pub fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CP_RODENT") {
        return Some(PathBuf::from(d));
    }
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("engines").join("rodent_v_1.2"))
}

/// The installed engine, if it is there (with its personalities beside it).
pub fn installed() -> Option<PathBuf> {
    let d = dir()?;
    let exe = d.join(EXE);
    (exe.is_file() && d.join("personalities").is_dir()).then_some(exe)
}

/// Removes the downloaded engine (a broken one, or to free its space).
pub fn remove() -> std::io::Result<()> {
    if std::env::var_os("CP_RODENT").is_some() {
        return Ok(());
    }
    match dir() {
        Some(d) if d.exists() => std::fs::remove_dir_all(d),
        _ => Ok(()),
    }
}

pub fn start(ctx: &egui::Context) -> Download {
    Download::spawn(ctx, ARCHIVE.size, run)
}

fn run(report: &dyn Fn(Stage), cancel: &AtomicBool) -> Result<PathBuf, Failure> {
    let bin = binary().ok_or(Failure::Unsupported)?;
    let dir = dir().ok_or_else(|| Failure::Disk("no data directory".into()))?;
    std::fs::create_dir_all(&dir).map_err(disk)?;
    let archive = dir.join(format!("{}.part", ARCHIVE.file));
    let url = format!(
        "https://github.com/nescitus/Rodent-V/releases/download/{TAG}/{}",
        ARCHIVE.file
    );
    let result = fetch(&url, &archive, &ARCHIVE, report, cancel)
        .and_then(|()| {
            report(Stage::Unpacking);
            unpack(&archive, &dir, bin)
        })
        .and_then(|exe| check_runs(&exe).map(|()| exe));
    let _ = std::fs::remove_file(&archive);
    if result.is_err() {
        // Nothing half-installed is left behind.
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Takes this platform's engine and the folders it reads (personalities, nets, books)
/// out of the archive; the other builds and the pictures of the manual stay out.
fn unpack(archive: &Path, dir: &Path, bin: &str) -> Result<PathBuf, Failure> {
    let exe = dir.join(EXE);
    let file = std::fs::File::open(archive).map_err(disk)?;
    let mut z = zip::ZipArchive::new(file).map_err(|e| Failure::Disk(e.to_string()))?;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| Failure::Disk(e.to_string()))?;
        if !entry.is_file() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| Failure::Disk(e.to_string()))?
            .into_owned();
        // "release-1-2/personalities/tal_nnue.txt" → ["personalities", "tal_nnue.txt"].
        let parts: Vec<&str> = name.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        let dest = match parts.as_slice() {
            [_, b] if *b == bin => exe.clone(),
            [_, folder, f]
                if ["personalities", "nets", "books"].contains(folder) && !f.contains("..") =>
            {
                let d = dir.join(folder);
                std::fs::create_dir_all(&d).map_err(disk)?;
                d.join(f)
            }
            _ => continue,
        };
        let mut out = std::fs::File::create(&dest).map_err(disk)?;
        std::io::copy(&mut entry, &mut out).map_err(disk)?;
    }
    if !exe.is_file() || !dir.join("personalities").is_dir() {
        return Err(Failure::Disk("no engine in the archive".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(disk)?;
    }
    Ok(exe)
}

/// Starts the engine once from its folder and waits for its name: it runs here (the
/// release needs AVX2: an older processor fails this).
fn check_runs(exe: &Path) -> Result<(), Failure> {
    use std::io::Write;
    let mut cmd = std::process::Command::new(exe);
    if let Some(d) = exe.parent() {
        cmd.current_dir(d);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|_| Failure::Broken)?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"uci\nquit\n");
    }
    let out = child.wait_with_output().map_err(|_| Failure::Broken)?;
    if String::from_utf8_lossy(&out.stdout).contains("Rodent") {
        Ok(())
    } else {
        Err(Failure::Broken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unpacks the real archive when it is at hand (`RODENT_ZIP=…/release-1-2.zip`).
    #[test]
    #[ignore = "needs the release archive (RODENT_ZIP)"]
    fn unpacks_the_engine_and_its_folders() {
        let zip = std::env::var("RODENT_ZIP").expect("RODENT_ZIP");
        let dir = std::env::temp_dir().join(format!("cp-rodent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = unpack(Path::new(&zip), &dir, "rodent_v_1.2").unwrap();
        assert!(exe.is_file());
        assert!(dir.join("personalities/tal_nnue.txt").is_file());
        assert!(dir.join("nets/rodent_4kb_768hl_8ob_v2.bin").is_file());
        assert!(dir.join("books/rodent.bin").is_file());
        assert!(!dir.join("rodent_v_1.2.exe").exists());
        check_runs(&exe).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
