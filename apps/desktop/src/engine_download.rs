//! Downloading the official Stockfish on request (decision 14): the "universal" build of
//! a pinned release from github.com/official-stockfish/Stockfish, checked against its
//! SHA-256 before use and unpacked into the user's data directory. Nothing is downloaded
//! without the user asking.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// The release offered; keep in step with `packaging/stockfish.sh` (Flatpak bundles it).
const TAG: &str = "sf_19";

/// An archive of the release for this platform.
pub struct Asset {
    pub file: &'static str,
    pub sha256: &'static str,
    /// Bytes, for the size shown before the download.
    pub size: u64,
    pub zip: bool,
}

fn asset() -> Option<Asset> {
    let (file, sha256, size, zip) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "stockfish-linux-x86-64-universal.tar.gz",
            "9defc0d4e55d49c65a6d042f3e571a39fcea499ade6dbe741b53b8c65e03611f",
            81_388_977,
            false,
        ),
        ("linux", "aarch64") => (
            "stockfish-linux-arm64-universal.tar.gz",
            "fe26cfd1d9db4c8af3d21e24d9ff34cacb31c1f940085a7583da11796f2bac01",
            80_181_250,
            false,
        ),
        ("macos", _) => (
            "stockfish-macos-universal.tar.gz",
            "a1f0e3bcc5a6927a11fe6fc8e54a779754645f3c2bae2cf13420fd1957adaa77",
            82_323_876,
            false,
        ),
        ("windows", "x86_64") => (
            "stockfish-windows-x86-64-universal.zip",
            "3c8bf1f9ea66a09350a40df4f632288285ac206d99f33ab5842c408fc30b48a7",
            81_431_614,
            true,
        ),
        ("windows", "aarch64") => (
            "stockfish-windows-arm64-universal.zip",
            "8372ad3f0d7276deb2c70f801f541ec7db463219fc6d9c7592864e542aa4f401",
            80_190_536,
            true,
        ),
        _ => return None,
    };
    Some(Asset {
        file,
        sha256,
        size,
        zip,
    })
}

/// Whether a download is offered on this platform at all.
pub fn available() -> bool {
    asset().is_some()
}

/// Size of the download in megabytes (rounded), for the offer.
pub fn size_mb() -> u64 {
    asset().map_or(0, |a| a.size.div_ceil(1_000_000))
}

#[cfg(windows)]
const EXE: &str = "stockfish.exe";
#[cfg(not(windows))]
const EXE: &str = "stockfish";

/// Where the downloaded engine lives: `<local data dir>/engines/sf_19/` (on Windows
/// the local, not the roaming, profile).
fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("engines").join(TAG))
}

/// Removes what an interrupted download left (the program was closed meanwhile).
pub fn clean_leftovers() {
    let Some(entries) = dir().and_then(|d| std::fs::read_dir(d).ok()) else {
        return;
    };
    for e in entries.flatten() {
        if e.path().extension().is_some_and(|x| x == "part") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The downloaded engine, if it is there.
pub fn installed() -> Option<PathBuf> {
    let p = dir()?.join(EXE);
    p.is_file().then_some(p)
}

/// Removes the downloaded engine (frees its space).
pub fn remove() -> std::io::Result<()> {
    match dir() {
        Some(d) if d.exists() => std::fs::remove_dir_all(d),
        _ => Ok(()),
    }
}

/// What went wrong, in terms of what the user can do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No connection, or the connection broke.
    Network(String),
    /// The server answered with an error status.
    Server(u16),
    /// The engine was installed but does not start on this computer.
    Broken,
    /// The file is not the published one (damaged on the way or replaced).
    Checksum,
    /// Writing to the disk failed (space, permissions).
    Disk(String),
    /// No download for this platform.
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Downloading { done: u64, total: u64 },
    Unpacking,
    Done(PathBuf),
    Failed(Failure),
    Cancelled,
}

/// What a download thread does: reports its stages, stops when told; the file in the end.
pub type Job = fn(&dyn Fn(Stage), &AtomicBool) -> Result<PathBuf, Failure>;

/// A download running in a background thread.
pub struct Download {
    stage: Arc<Mutex<Stage>>,
    cancel: Arc<AtomicBool>,
}

impl Download {
    pub fn start(ctx: &egui::Context) -> Download {
        let total = asset().map_or(0, |a| a.size);
        Download::spawn(ctx, total, run)
    }

    /// Runs `job` (a download and what follows it) in a thread of its own; `total`: the
    /// bytes expected, shown until the server says.
    pub fn spawn(
        ctx: &egui::Context,
        total: u64,
        job: Job,
    ) -> Download {
        let stage = Arc::new(Mutex::new(Stage::Downloading { done: 0, total }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c, ctx) = (stage.clone(), cancel.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("engine-download".into())
            .spawn(move || {
                let report = |st: Stage| {
                    if let Ok(mut g) = s.lock() {
                        *g = st;
                    }
                    ctx.request_repaint();
                };
                let end = match job(&report, &c) {
                    Ok(path) => Stage::Done(path),
                    Err(_) if c.load(Ordering::Relaxed) => Stage::Cancelled,
                    Err(f) => Stage::Failed(f),
                };
                report(end);
            });
        if spawned.is_err()
            && let Ok(mut g) = stage.lock()
        {
            *g = Stage::Failed(Failure::Disk("cannot start a thread".into()));
        }
        Download { stage, cancel }
    }

    pub fn stage(&self) -> Stage {
        self.stage
            .lock()
            .map(|g| g.clone())
            .unwrap_or(Stage::Cancelled)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn running(&self) -> bool {
        matches!(self.stage(), Stage::Downloading { .. } | Stage::Unpacking)
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn run(report: &dyn Fn(Stage), cancel: &AtomicBool) -> Result<PathBuf, Failure> {
    let asset = asset().ok_or(Failure::Unsupported)?;
    let dir = dir().ok_or_else(|| Failure::Disk("no data directory".into()))?;
    std::fs::create_dir_all(&dir).map_err(disk)?;
    let archive = dir.join(format!("{}.part", asset.file));
    let url = format!(
        "https://github.com/official-stockfish/Stockfish/releases/download/{TAG}/{}",
        asset.file
    );
    let result = fetch(&url, &archive, &asset, report, cancel)
        .and_then(|()| {
            report(Stage::Unpacking);
            unpack(&archive, &dir, asset.zip)
        })
        .and_then(|exe| check_runs(&exe).map(|()| exe));
    let _ = std::fs::remove_file(&archive);
    if result.is_err() {
        // Nothing half-installed is left behind.
        let _ = std::fs::remove_file(dir.join(EXE));
    }
    result
}

/// Downloads `url` into `to`, checking the size and the SHA-256 on the way.
pub fn fetch(
    url: &str,
    to: &Path,
    asset: &Asset,
    report: &dyn Fn(Stage),
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    let mut response = ureq::get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => Failure::Server(code),
        e => Failure::Network(e.to_string()),
    })?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(asset.size);
    let mut body = response
        .body_mut()
        .with_config()
        .limit(asset.size + 1)
        .reader();
    let mut file = std::fs::File::create(to).map_err(disk)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut reported = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Failure::Network("cancelled".into()));
        }
        let n = body
            .read(&mut buf)
            .map_err(|e| Failure::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(disk)?;
        done += n as u64;
        if done - reported >= 512 * 1024 {
            reported = done;
            report(Stage::Downloading { done, total });
        }
    }
    file.sync_all().map_err(disk)?;
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if done != asset.size || digest != asset.sha256 {
        return Err(Failure::Checksum);
    }
    Ok(())
}

/// Takes the engine executable and its license files out of the archive.
fn unpack(archive: &Path, dir: &Path, zip: bool) -> Result<PathBuf, Failure> {
    let exe = dir.join(EXE);
    let wanted = |name: &str| -> Option<PathBuf> {
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if base.starts_with("stockfish") && (base.ends_with(".exe") || !base.contains('.')) {
            Some(exe.clone())
        } else if base == "Copying.txt" || base == "AUTHORS" {
            Some(dir.join(base))
        } else {
            None
        }
    };
    let file = std::fs::File::open(archive).map_err(disk)?;
    if zip {
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
            if let Some(dest) = wanted(&name) {
                let mut out = std::fs::File::create(&dest).map_err(disk)?;
                std::io::copy(&mut entry, &mut out).map_err(disk)?;
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries().map_err(disk)? {
            let mut entry = entry.map_err(disk)?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let name = entry.path().map_err(disk)?.to_string_lossy().into_owned();
            if let Some(dest) = wanted(&name) {
                let mut out = std::fs::File::create(&dest).map_err(disk)?;
                std::io::copy(&mut entry, &mut out).map_err(disk)?;
            }
        }
    }
    if !exe.is_file() {
        return Err(Failure::Disk("no engine in the archive".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(disk)?;
    }
    Ok(exe)
}

/// Starts the engine once and waits for its name: the binary runs on this computer.
fn check_runs(exe: &Path) -> Result<(), Failure> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("quit");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|_| Failure::Broken)?;
    if String::from_utf8_lossy(&out.stdout).contains("Stockfish") {
        Ok(())
    } else {
        Err(Failure::Broken)
    }
}

pub fn disk(e: std::io::Error) -> Failure {
    Failure::Disk(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_platform_has_a_pinned_download() {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            let a = asset().expect("asset");
            assert_eq!(a.sha256.len(), 64);
            assert!(!a.zip);
            assert_eq!(size_mb(), 82);
        }
    }

    /// Downloads the real release (≈ 80 MB): `XDG_DATA_HOME=… cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_the_real_engine() {
        let ctx = egui::Context::default();
        let d = Download::start(&ctx);
        while d.running() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let Stage::Done(exe) = d.stage() else {
            panic!("{:?}", d.stage());
        };
        let out = std::process::Command::new(&exe)
            .arg("quit")
            .output()
            .expect("runs");
        assert!(String::from_utf8_lossy(&out.stdout).contains("Stockfish"));
        assert_eq!(installed(), Some(exe));
    }

    #[test]
    fn unpacks_the_engine_and_licenses_from_a_tarball() {
        let dir = std::env::temp_dir().join(format!("cp-sf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("a.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut b = tar::Builder::new(gz);
            for (name, data) in [
                ("stockfish/stockfish-linux-x86-64-universal", &b"engine"[..]),
                ("stockfish/Copying.txt", b"GPL"),
                ("stockfish/README.md", b"x"),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, data).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let exe = unpack(&archive, &dir, false).expect("unpacked");
        assert_eq!(std::fs::read(&exe).unwrap(), b"engine");
        assert!(dir.join("Copying.txt").is_file());
        assert!(!dir.join("README.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
