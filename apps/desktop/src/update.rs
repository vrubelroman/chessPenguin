//! Updates: at start the program asks its site (else GitHub) for the latest release
//! (once, in the background; can be turned off in Settings → About; the site counts the
//! installation, see `releases::check_latest`). When it is newer, a button next to
//! Settings offers it. What "update" does depends on how ChessPenguin was installed:
//!
//! - the Windows zip, the AppImage and the Linux tarball: the new file is downloaded,
//!   checked against `SHA256SUMS.txt` and put in place of the running one; the program
//!   offers to restart (Windows can rename a running executable, Linux can replace it);
//! - macOS: the disk image is downloaded, checked and opened; the user drags the app
//!   over the old one, as at the first installation;
//! - deb, rpm, Arch and Flatpak packages: their files belong to the system's package
//!   manager, so the package is downloaded into Downloads, checked, and the one command
//!   that installs it is shown with a Copy button;
//! - the AUR package: it is updated with the AUR helper (the command is shown);
//! - anything else (a build from the sources): a link to the download page.
//!
//! `CP_UPDATE_SOURCE=<dir>` reads `latest.json` (the GitHub answer) and the assets from
//! a directory instead (tests, manual checks); `CP_UPDATE_KIND=<kind>` pretends another
//! kind of installation.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use releases::Asset;
pub use releases::{DOWNLOAD_PAGE, Release, parse_version};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// How this copy of ChessPenguin was installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// The Windows zip: the executable can be replaced.
    Windows(PathBuf),
    /// macOS: the app bundle (replaced by the user from the disk image).
    Mac,
    /// An AppImage at this path (replaced in place).
    AppImage(PathBuf),
    /// The Linux tarball unpacked here: its `chesspenguin` is replaced.
    Tarball(PathBuf),
    Deb,
    Rpm,
    Arch,
    /// The AUR package `chesspenguin-bin`.
    Aur,
    Flatpak,
    /// A build from the sources, or a place we cannot write to.
    Other,
}

impl Install {
    /// The suffix of the release asset for this installation.
    fn asset_suffix(&self) -> Option<&'static str> {
        Some(match self {
            Install::Windows(_) => "-windows-x86_64.zip",
            Install::Mac => "-macos-arm64.dmg",
            Install::AppImage(_) => "-x86_64.AppImage",
            Install::Tarball(_) => "-linux-x86_64.tar.gz",
            Install::Deb => "_amd64.deb",
            Install::Rpm => ".x86_64.rpm",
            Install::Arch => "-x86_64.pkg.tar.zst",
            Install::Flatpak => "-x86_64.flatpak",
            Install::Aur | Install::Other => return None,
        })
    }

    /// The program replaces its own files.
    pub fn replaces_itself(&self) -> bool {
        matches!(
            self,
            Install::Windows(_) | Install::AppImage(_) | Install::Tarball(_)
        )
    }

    /// Short name for the dialog ("deb package").
    pub fn label(&self) -> &'static str {
        match self {
            Install::Windows(_) => "Windows",
            Install::Mac => "macOS",
            Install::AppImage(_) => "AppImage",
            Install::Tarball(_) => "tar.gz",
            Install::Deb => "deb",
            Install::Rpm => "rpm",
            Install::Arch => "Arch Linux",
            Install::Aur => "AUR",
            Install::Flatpak => "Flatpak",
            Install::Other => "—",
        }
    }
}

fn writable_dir(dir: &Path) -> bool {
    let probe = dir.join(".chesspenguin-write-test");
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// How this copy was installed (see [`Install`]).
pub fn detect() -> Install {
    if let Ok(kind) = std::env::var("CP_UPDATE_KIND") {
        let exe = std::env::current_exe().unwrap_or_default();
        return match kind.as_str() {
            "windows" => Install::Windows(exe),
            "mac" => Install::Mac,
            "appimage" => {
                Install::AppImage(std::env::var_os("APPIMAGE").map_or(exe, PathBuf::from))
            }
            "tarball" => Install::Tarball(exe.parent().map(Path::to_owned).unwrap_or_default()),
            "deb" => Install::Deb,
            "rpm" => Install::Rpm,
            "arch" => Install::Arch,
            "aur" => Install::Aur,
            "flatpak" => Install::Flatpak,
            _ => Install::Other,
        };
    }
    let Ok(exe) = std::env::current_exe() else {
        return Install::Other;
    };
    if cfg!(windows) {
        return match exe.parent() {
            Some(d) if writable_dir(d) => Install::Windows(exe),
            _ => Install::Other,
        };
    }
    if cfg!(target_os = "macos") {
        return if exe.to_string_lossy().contains(".app/Contents/MacOS/") {
            Install::Mac
        } else {
            Install::Other
        };
    }
    if std::env::var_os("FLATPAK_ID").is_some() {
        return Install::Flatpak;
    }
    // An AppImage of ours: the AppImage runtime sets APPIMAGE and APPDIR, and this
    // executable lies inside APPDIR (a variable inherited from another AppImage, such
    // as a terminal started from one, must not make us replace that one).
    if let (Some(p), Some(appdir)) = (
        std::env::var_os("APPIMAGE").map(PathBuf::from),
        std::env::var_os("APPDIR").map(PathBuf::from),
    ) && exe.starts_with(&appdir)
    {
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        return match p.parent() {
            Some(d) if p.is_file() && writable_dir(d) => Install::AppImage(p),
            _ => Install::Other,
        };
    }
    if exe == Path::new("/usr/bin/chesspenguin") {
        return package_kind();
    }
    // The tarball: the executable next to the files it was unpacked with.
    if let Some(dir) = exe.parent()
        && dir.join("THIRD_PARTY_NOTICES.md").is_file()
        && dir.join("examples").is_dir()
        && !dir.join("Cargo.toml").exists()
        && writable_dir(dir)
    {
        return Install::Tarball(dir.to_owned());
    }
    Install::Other
}

/// Which package manager owns `/usr/bin/chesspenguin`.
fn package_kind() -> Install {
    if let Ok(entries) = std::fs::read_dir("/var/lib/pacman/local") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("chesspenguin-bin-") {
                return Install::Aur;
            }
            if name
                .strip_prefix("chesspenguin-")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            {
                return Install::Arch;
            }
        }
    }
    if Path::new("/var/lib/dpkg/info/chesspenguin.list").is_file() {
        return Install::Deb;
    }
    let rpm = std::process::Command::new("rpm")
        .args(["-q", "chesspenguin"])
        .output();
    if rpm.is_ok_and(|o| o.status.success()) {
        return Install::Rpm;
    }
    Install::Other
}

/// The file of a release for a kind of installation.
pub trait ReleaseExt {
    fn asset_for(&self, kind: &Install) -> Option<&Asset>;
}

impl ReleaseExt for Release {
    fn asset_for(&self, kind: &Install) -> Option<&Asset> {
        self.asset_ending(kind.asset_suffix()?)
    }
}

/// A local directory standing in for GitHub (tests).
fn source_dir() -> Option<PathBuf> {
    std::env::var_os("CP_UPDATE_SOURCE").map(PathBuf::from)
}

/// Who asks the site and GitHub.
fn client() -> String {
    format!("ChessPenguin/{}", env!("CARGO_PKG_VERSION"))
}

static INSTALL_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// This installation's id for the update check (`Settings::install_id`).
pub fn set_install_id(id: &str) {
    let _ = INSTALL_ID.set(id.to_owned());
}

/// A new random installation id: 32 hex digits.
pub fn new_install_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // No randomness: no id; the check then goes to GitHub, uncounted.
        return String::new();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Install {
    /// The platform the site counts this installation under.
    fn platform(&self) -> &'static str {
        match self {
            Install::Windows(_) => "windows",
            Install::Mac => "macos",
            Install::AppImage(_) => "appimage",
            Install::Tarball(_) => "tarball",
            Install::Deb => "deb",
            Install::Rpm => "rpm",
            Install::Arch => "arch",
            Install::Aur => "aur",
            Install::Flatpak => "flatpak",
            Install::Other => "other",
        }
    }
}

/// Asks for the latest release (through the site, which counts this installation; see
/// `releases::check_latest`). `Err` with a short reason when there is no answer.
pub fn latest() -> Result<Release, String> {
    match source_dir() {
        Some(dir) => {
            let text =
                std::fs::read_to_string(dir.join("latest.json")).map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            releases::parse_release(&v).ok_or_else(|| "unexpected answer".to_owned())
        }
        None => match INSTALL_ID.get().filter(|id| !id.is_empty()) {
            Some(id) => releases::check_latest(id, detect().platform(), &client()),
            None => releases::fetch_latest(releases::API, &client()),
        },
    }
}

/// Opens an asset for reading (from GitHub or from the test directory).
fn open(asset: &Asset) -> Result<Box<dyn Read + Send>, String> {
    match source_dir() {
        Some(dir) => Ok(Box::new(
            std::fs::File::open(dir.join(&asset.name)).map_err(|e| e.to_string())?,
        )),
        None => releases::open(asset, &client()),
    }
}

/// The SHA-256 of `name` in the release's `SHA256SUMS.txt`.
fn expected_sum(release: &Release, name: &str) -> Result<String, String> {
    let sums = release.sums().ok_or("the release has no SHA256SUMS.txt")?;
    let mut text = String::new();
    open(sums)?
        .take(1 << 20)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    releases::sum_in(&text, name).ok_or_else(|| format!("{name} is not in SHA256SUMS.txt"))
}

/// Downloads `asset` into `to` (a `.part` file first), checking its SHA-256;
/// `progress` counts the bytes.
fn download(
    release: &Release,
    asset: &Asset,
    to: &Path,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let expected = expected_sum(release, &asset.name)?;
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let part = to.with_extension("part");
    let result = (|| {
        let mut input = open(asset)?;
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_owned());
            }
            let n = input.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            progress.fetch_add(n, Ordering::Relaxed);
        }
        file.sync_all().map_err(|e| e.to_string())?;
        let got = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if got != expected {
            return Err("checksum".to_owned());
        }
        std::fs::rename(&part, to).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// What an update ended with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The program's files are replaced: restarting runs the new version.
    Restart(PathBuf),
    /// The package is in Downloads: this command installs it.
    Command { file: PathBuf, command: String },
    /// The macOS disk image is open: drag the app over the old one.
    DiskImageOpened(PathBuf),
}

#[cfg(unix)]
fn make_executable(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Puts `new` in place of `target` (the running program): `new` is moved next to it
/// first, so the switch is one rename on the same disk. On Windows the running
/// executable cannot be overwritten but can be renamed: it becomes `<name>.old` (removed
/// at the next start, see [`clean_leftovers`]).
pub fn replace_file(new: &Path, target: &Path) -> std::io::Result<()> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staged = target.with_file_name(format!(".{name}.new"));
    std::fs::copy(new, &staged)?;
    // The permissions of the file it replaces (an executable stays one).
    match std::fs::metadata(target) {
        Ok(m) => std::fs::set_permissions(&staged, m.permissions())?,
        Err(_) => make_executable(&staged)?,
    }
    if cfg!(windows) {
        let old = target.with_file_name(format!("{name}.old"));
        let _ = std::fs::remove_file(&old);
        if target.exists() {
            std::fs::rename(target, &old)?;
        }
        if let Err(e) = std::fs::rename(&staged, target) {
            // Put the old one back rather than leave nothing.
            let _ = std::fs::rename(&old, target);
            return Err(e);
        }
        Ok(())
    } else {
        std::fs::rename(&staged, target)
    }
}

/// Removes what an update left: the previous Windows executable, a staged file.
pub fn clean_leftovers() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(exe.with_file_name(format!("{name}.old")));
    let _ = std::fs::remove_file(exe.with_file_name(format!(".{name}.new")));
}

fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CP_UPDATE_CACHE") {
        return PathBuf::from(d);
    }
    crate::settings::Settings::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("update")
}

/// `XDG_DOWNLOAD_DIR` of the user's own `~/.config/user-dirs.dirs` (a Flatpak has its
/// own config folder, where the file is not).
fn host_download_dir(home: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(home.join(".config/user-dirs.dirs")).ok()?;
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?;
    let v = line
        .trim()
        .trim_matches('"')
        .replace("$HOME", &home.to_string_lossy());
    let p = PathBuf::from(v);
    p.is_dir().then_some(p)
}

/// Downloads (the system's, else `~/Downloads` when it exists, else the home folder).
fn downloads_dir() -> PathBuf {
    let dirs = directories::UserDirs::new();
    dirs.as_ref()
        .and_then(|u| u.download_dir().map(Path::to_owned))
        .or_else(|| dirs.as_ref().and_then(|u| host_download_dir(u.home_dir())))
        .or_else(|| {
            dirs.as_ref()
                .map(|u| u.home_dir().join("Downloads"))
                .filter(|d| d.is_dir())
        })
        .or_else(|| dirs.as_ref().map(|u| u.home_dir().to_owned()))
        .unwrap_or_else(std::env::temp_dir)
}

/// A word for a shell command: as it is when it has only safe characters, else in
/// single quotes (nothing is expanded inside them in sh, bash, zsh and fish).
fn shell_word(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+:@%,".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The executable inside a downloaded Windows zip or Linux tarball.
fn unpack_executable(archive: &Path, kind: &Install, into: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let out = into.join(if matches!(kind, Install::Windows(_)) {
        "ChessPenguin.exe"
    } else {
        "chesspenguin"
    });
    match kind {
        Install::Windows(_) => {
            let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            let mut found = false;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
                let name = entry.name().map_err(|e| e.to_string())?.into_owned();
                if entry.is_file()
                    && (name.ends_with("/ChessPenguin.exe") || name == "ChessPenguin.exe")
                {
                    let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("ChessPenguin.exe is not in the archive".into());
            }
        }
        _ => {
            let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let mut found = false;
            for entry in tar.entries().map_err(|e| e.to_string())? {
                let mut entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path().map_err(|e| e.to_string())?.into_owned();
                if path.file_name().is_some_and(|n| n == "chesspenguin")
                    && entry.header().entry_type().is_file()
                {
                    let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("chesspenguin is not in the archive".into());
            }
        }
    }
    make_executable(&out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Downloads and applies the update for this installation (in a background thread).
pub fn update(
    release: &Release,
    kind: &Install,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Applied, String> {
    let asset = release
        .asset_for(kind)
        .ok_or("the release has no file for this installation")?
        .clone();
    match kind {
        Install::Windows(_) | Install::AppImage(_) | Install::Tarball(_) => {
            let dir = cache_dir();
            let archive = dir.join(&asset.name);
            download(release, &asset, &archive, progress, cancel)?;
            let result = (|| {
                let (new, target) = match kind {
                    Install::AppImage(path) => (archive.clone(), path.clone()),
                    Install::Windows(exe) => (
                        unpack_executable(&archive, kind, &dir.join("unpacked"))?,
                        exe.clone(),
                    ),
                    Install::Tarball(folder) => (
                        unpack_executable(&archive, kind, &dir.join("unpacked"))?,
                        folder.join("chesspenguin"),
                    ),
                    _ => unreachable!("kinds that replace themselves"),
                };
                replace_file(&new, &target).map_err(|e| e.to_string())?;
                Ok(Applied::Restart(target))
            })();
            let _ = std::fs::remove_dir_all(&dir);
            result
        }
        Install::Mac => {
            let file = downloads_dir().join(&asset.name);
            download(release, &asset, &file, progress, cancel)?;
            let _ = std::process::Command::new("open").arg(&file).spawn();
            Ok(Applied::DiskImageOpened(file))
        }
        Install::Deb | Install::Rpm | Install::Arch | Install::Flatpak => {
            let file = downloads_dir().join(&asset.name);
            download(release, &asset, &file, progress, cancel)?;
            Ok(Applied::Command {
                command: install_command(kind, &file).unwrap_or_default(),
                file,
            })
        }
        Install::Aur | Install::Other => Err("no automatic update for this installation".into()),
    }
}

/// openSUSE installs rpm packages with zypper, the others with dnf.
fn suse() -> bool {
    std::fs::read_to_string("/etc/os-release").is_ok_and(|t| {
        t.lines()
            .any(|l| (l.starts_with("ID=") || l.starts_with("ID_LIKE=")) && l.contains("suse"))
    })
}

/// A Flatpak installed for the user (not system-wide): `/.flatpak-info` says where.
fn flatpak_user() -> bool {
    std::fs::read_to_string("/.flatpak-info").map_or(true, |t| t.contains("/.local/share/flatpak"))
}

/// The command that installs a downloaded package (`None`: nothing to download): it
/// goes to the package's folder first, so the command stays short.
pub fn install_command(kind: &Install, file: &Path) -> Option<String> {
    let dir = shell_word(&file.parent()?.to_string_lossy());
    let name = shell_word(&format!("./{}", file.file_name()?.to_string_lossy()));
    let install = match kind {
        Install::Deb => "sudo apt install",
        Install::Rpm if suse() => "sudo zypper install",
        Install::Rpm => "sudo dnf install",
        Install::Arch => "sudo pacman -U",
        Install::Flatpak if flatpak_user() => "flatpak install --user --reinstall -y",
        Install::Flatpak => "flatpak install --system --reinstall -y",
        _ => return None,
    };
    Some(format!("cd {dir} && {install} {name}"))
}

/// The command for the AUR package (no download: the helper builds it).
pub const AUR_COMMAND: &str = "yay -Syu chesspenguin-bin";
/// The same with the other common AUR helper.
pub const AUR_COMMAND_PARU: &str = "paru -Syu chesspenguin-bin";

/// The release check at start, in the background.
pub struct Check {
    rx: Option<std::sync::mpsc::Receiver<Result<Release, String>>>,
    pub release: Option<Release>,
    pub error: Option<String>,
    pub done: bool,
}

impl Check {
    pub fn idle() -> Check {
        Check {
            rx: None,
            release: None,
            error: None,
            done: false,
        }
    }

    /// Asks again, keeping what the previous check found until the answer comes (a
    /// failed second check does not take an offer away).
    pub fn again(&self, ctx: &egui::Context) -> Check {
        let mut c = Check::start(ctx);
        c.release = self.release.clone();
        c
    }

    pub fn start(ctx: &egui::Context) -> Check {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let _ = tx.send(latest());
                ctx.request_repaint();
            });
        Check {
            rx: spawned.ok().map(|_| rx),
            ..Check::idle()
        }
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok(r) = rx.try_recv()
        {
            match r {
                Ok(rel) => {
                    self.release = Some(rel);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
            self.rx = None;
            self.done = true;
        }
    }

    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// A newer release, if one was found.
    pub fn newer(&self) -> Option<&Release> {
        self.release.as_ref().filter(|r| r.newer())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.7.1"), Some((0, 7, 1)));
        assert_eq!(parse_version("0.8"), Some((0, 8, 0)));
        assert_eq!(parse_version("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_version("x"), None);
        assert!((0, 10, 0) > (0, 9, 9));
    }

    fn release() -> Release {
        let names = [
            "ChessPenguin-9.9.9-windows-x86_64.zip",
            "ChessPenguin-9.9.9-macos-arm64.dmg",
            "ChessPenguin-9.9.9-x86_64.AppImage",
            "ChessPenguin-9.9.9-linux-x86_64.tar.gz",
            "chesspenguin_9.9.9_amd64.deb",
            "chesspenguin-9.9.9-1.x86_64.rpm",
            "chesspenguin-9.9.9-1-x86_64.pkg.tar.zst",
            "ChessPenguin-9.9.9-x86_64.flatpak",
            "SHA256SUMS.txt",
        ];
        Release {
            version: "9.9.9".into(),
            page: String::new(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    url: format!("https://example.org/{n}"),
                    size: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn an_asset_for_every_installation() {
        let r = release();
        assert!(r.newer());
        for (kind, name) in [
            (
                Install::Windows(PathBuf::new()),
                "ChessPenguin-9.9.9-windows-x86_64.zip",
            ),
            (Install::Mac, "ChessPenguin-9.9.9-macos-arm64.dmg"),
            (
                Install::AppImage(PathBuf::new()),
                "ChessPenguin-9.9.9-x86_64.AppImage",
            ),
            (
                Install::Tarball(PathBuf::new()),
                "ChessPenguin-9.9.9-linux-x86_64.tar.gz",
            ),
            (Install::Deb, "chesspenguin_9.9.9_amd64.deb"),
            (Install::Rpm, "chesspenguin-9.9.9-1.x86_64.rpm"),
            (Install::Arch, "chesspenguin-9.9.9-1-x86_64.pkg.tar.zst"),
            (Install::Flatpak, "ChessPenguin-9.9.9-x86_64.flatpak"),
        ] {
            assert_eq!(r.asset_for(&kind).map(|a| a.name.as_str()), Some(name));
        }
        assert!(r.asset_for(&Install::Aur).is_none());
    }

    #[test]
    fn commands() {
        let f = Path::new("/home/u/Downloads/chesspenguin_9.9.9_amd64.deb");
        assert_eq!(
            install_command(&Install::Deb, f).unwrap(),
            "cd /home/u/Downloads && sudo apt install ./chesspenguin_9.9.9_amd64.deb"
        );
        let spaced = Path::new("/home/u/My Downloads/x.flatpak");
        assert!(
            install_command(&Install::Flatpak, spaced)
                .unwrap()
                .starts_with("cd '/home/u/My Downloads' && flatpak install")
        );
        // Nothing in a path is expanded by the shell.
        for nasty in [
            "/home/a`id`/x.deb",
            "/home/$USER/x.deb",
            "/home/O'Brien/x.deb",
            "/h/a b/!x.deb",
        ] {
            let c = install_command(&Install::Deb, Path::new(nasty)).unwrap();
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(
                    c.replace("cd ", "printf '%s\\n' ")
                        .replace(" && sudo apt install ", " "),
                )
                .output()
                .unwrap();
            let text = String::from_utf8(out.stdout).unwrap();
            let p = Path::new(nasty);
            assert_eq!(
                text,
                format!(
                    "{}\n./{}\n",
                    p.parent().unwrap().display(),
                    p.file_name().unwrap().to_string_lossy()
                ),
                "{nasty}"
            );
        }
    }

    /// The running test executable can be put aside and back: what an update does to
    /// the running program (on Windows the rename is the point of the scheme).
    #[test]
    fn replacing_the_running_executable() {
        let exe = std::env::current_exe().unwrap();
        let dir = std::env::temp_dir().join(format!("cp-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A copy of the running executable stands for the program in its folder.
        let target = dir.join(exe.file_name().unwrap());
        std::fs::copy(&exe, &target).unwrap();
        let new = dir.join("new-version");
        std::fs::write(&new, b"new version").unwrap();
        replace_file(&new, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new version");
        if cfg!(windows) {
            let name = target.file_name().unwrap().to_string_lossy().into_owned();
            assert!(target.with_file_name(format!("{name}.old")).exists());
        }
        assert!(
            !dir.join(format!(
                ".{}.new",
                target.file_name().unwrap().to_string_lossy()
            ))
            .exists()
        );
        // And the running executable itself can be renamed and put back.
        let aside = exe.with_extension("cp-test-aside");
        std::fs::rename(&exe, &aside).unwrap();
        std::fs::rename(&aside, &exe).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A whole update from a local "release": the tarball's executable replaced, the
    /// checksum checked, a wrong checksum refused.
    #[test]
    fn tarball_update_from_a_local_release() {
        let root = std::env::temp_dir().join(format!("cp-update-tar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("release");
        let installed = root.join("installed");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::write(installed.join("chesspenguin"), b"old").unwrap();
        // The release tarball: a folder with the new executable.
        let name = "ChessPenguin-9.9.9-linux-x86_64.tar.gz";
        {
            let f = std::fs::File::create(src.join(name)).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut tar = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(
                &mut h,
                "ChessPenguin-9.9.9-linux-x86_64/chesspenguin",
                &b"new"[..],
            )
            .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let sum = |p: &Path| {
            Sha256::digest(std::fs::read(p).unwrap())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        std::fs::write(
            src.join("SHA256SUMS.txt"),
            format!("{}  {name}\n", sum(&src.join(name))),
        )
        .unwrap();
        let rel = Release {
            version: "9.9.9".into(),
            page: String::new(),
            assets: [name, "SHA256SUMS.txt"]
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    url: String::new(),
                    size: 0,
                })
                .collect(),
        };
        // SAFETY of the test: the variable is only read by this process's update code.
        unsafe {
            std::env::set_var("CP_UPDATE_SOURCE", &src);
            std::env::set_var("CP_UPDATE_CACHE", root.join("cache"));
        }
        let kind = Install::Tarball(installed.clone());
        let progress = AtomicUsize::new(0);
        let applied = update(&rel, &kind, &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(applied, Applied::Restart(installed.join("chesspenguin")));
        assert_eq!(
            std::fs::read(installed.join("chesspenguin")).unwrap(),
            b"new"
        );
        assert!(progress.load(Ordering::Relaxed) > 0);
        // A damaged download is refused and nothing is replaced.
        std::fs::write(installed.join("chesspenguin"), b"old").unwrap();
        std::fs::write(
            src.join("SHA256SUMS.txt"),
            format!("{}  {name}\n", "0".repeat(64)),
        )
        .unwrap();
        let err = update(&rel, &kind, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap_err();
        assert_eq!(err, "checksum");
        assert_eq!(
            std::fs::read(installed.join("chesspenguin")).unwrap(),
            b"old"
        );
        unsafe {
            std::env::remove_var("CP_UPDATE_SOURCE");
            std::env::remove_var("CP_UPDATE_CACHE");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
