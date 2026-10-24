//! Starting the window on machines whose graphics do not cooperate (an old driver, a
//! broken Vulkan, a virtual machine, remote desktop). A start that fails or dies before
//! the first frames tries the next, more modest mode; the mode that worked is kept.
//! A failure no mode overcomes, and any panic, is shown in a message box and written to
//! `chesspenguin.log` beside the settings: a Windows program has no console to print to.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu;

/// The ways to draw, from the usual one to software rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// wgpu's own choice: Vulkan, DirectX 12 or Metal, then OpenGL.
    Default,
    /// Without Vulkan (a broken Vulkan driver can kill the process on Windows).
    NoVulkan,
    /// OpenGL only.
    OpenGl,
    /// The processor draws (WARP on Windows, llvmpipe on Linux): slow but works.
    Software,
}

const MODES: [Mode; 4] = [Mode::Default, Mode::NoVulkan, Mode::OpenGl, Mode::Software];

static CURRENT: OnceLock<Mode> = OnceLock::new();
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn state_file() -> Option<PathBuf> {
    crate::settings::Settings::config_dir().map(|d| d.join("graphics"))
}

pub fn log_file() -> Option<PathBuf> {
    crate::settings::Settings::config_dir().map(|d| d.join("chesspenguin.log"))
}

/// The state file: "trying N" while a start in mode N is not yet known to work,
/// "ok N" once it drew its first frames.
fn read_state() -> Option<(bool, usize)> {
    let text = std::fs::read_to_string(state_file()?).ok()?;
    let (word, n) = text.trim().split_once(' ')?;
    let n: usize = n.parse().ok()?;
    (n < MODES.len()).then_some((word == "ok", n))
}

fn write_state(ok: bool, n: usize) {
    if let Some(f) = state_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, format!("{} {n}", if ok { "ok" } else { "trying" }));
    }
}

/// Set for a process started again after a failed start: the modes tried so far.
const TRIED: &str = "CP_GRAPHICS_TRIED";

/// The mode for this start: the one that worked last time, the next one after a start
/// that died before its first frame, or the one a failed start chose for this process.
/// `WGPU_BACKEND` set by hand wins.
pub fn choose() -> Mode {
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return *CURRENT.get_or_init(|| Mode::Default);
    }
    let retrying = std::env::var_os(TRIED).is_some();
    let n = match read_state() {
        Some((ok, n)) if ok || retrying => n,
        Some((_, n)) => {
            let next = (n + 1) % MODES.len();
            log(&format!(
                "the last start in mode {:?} did not finish, trying {:?}",
                MODES[n], MODES[next]
            ));
            next
        }
        None => 0,
    };
    write_state(false, n);
    *CURRENT.get_or_init(|| MODES[n])
}

/// The first frame is shown: keep this mode for the next starts.
pub fn started() {
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Some(&mode) = CURRENT.get()
        && std::env::var_os("WGPU_BACKEND").is_none()
        && let Some(n) = MODES.iter().position(|m| *m == mode)
        && read_state() != Some((true, n))
    {
        write_state(true, n);
        if mode != Mode::Default {
            log(&format!("started in mode {mode:?}"));
        }
    }
}

/// The start in this mode failed with an error: start again in a mode not tried yet
/// (a new process: the window system allows one event loop per process). False when
/// every mode has failed.
pub fn retry(error: &str) -> bool {
    let mode = CURRENT.get().copied().unwrap_or(Mode::Default);
    log(&format!("the start in mode {mode:?} failed: {error}"));
    let n = MODES.iter().position(|m| *m == mode).unwrap_or(0);
    let mut tried: Vec<usize> = std::env::var(TRIED)
        .unwrap_or_default()
        .split(',')
        .filter_map(|t| t.parse().ok())
        .collect();
    tried.push(n);
    let next = (1..MODES.len())
        .map(|k| (n + k) % MODES.len())
        .find(|m| !tried.contains(m));
    let exe = std::env::current_exe();
    let (Some(next), Ok(exe), None) = (next, exe, std::env::var_os("WGPU_BACKEND")) else {
        if let Some(f) = state_file() {
            let _ = std::fs::remove_file(f);
        }
        return false;
    };
    write_state(false, next);
    let tried: Vec<String> = tried.iter().map(|t| t.to_string()).collect();
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(TRIED, tried.join(","))
        .spawn()
        .is_ok()
}

pub fn options(mode: Mode) -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    let backends = &mut setup.instance_descriptor.backends;
    match mode {
        Mode::Default => return WgpuConfiguration::default(),
        Mode::NoVulkan => {
            *backends = (wgpu::Backends::PRIMARY | wgpu::Backends::GL) - wgpu::Backends::VULKAN
        }
        Mode::OpenGl => *backends = wgpu::Backends::GL,
        Mode::Software => {
            *backends = wgpu::Backends::all();
            setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
                adapters
                    .iter()
                    .filter(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
                    .find(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
                    .cloned()
                    .ok_or_else(|| "no software renderer".to_owned())
            }));
        }
    }
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// The warnings of the window and graphics libraries (wgpu, eframe, winit…) go to the log
/// and to stderr: a window that stops drawing says why there. One message repeated (a
/// dropped frame each frame) is written once a minute with how many times it came.
struct LibraryLog {
    last: std::sync::Mutex<Option<(String, std::time::Instant, u32)>>,
    /// `CP_LOG_FRAMES=1`: also how each frame reaches the screen (egui_wgpu's debug and
    /// trace messages: a frame skipped as "occluded" is said only there).
    frames: bool,
}

impl log::Log for LibraryLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
            || (self.frames && metadata.target().starts_with("egui_wgpu"))
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = format!("{} {}: {}", record.level(), record.target(), record.args());
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((prev, since, count)) = last.as_mut()
            && *prev == text
            && since.elapsed() < std::time::Duration::from_secs(60)
        {
            *count += 1;
            return;
        }
        if let Some((prev, _, count)) = last.take()
            && count > 0
        {
            log(&format!("{prev} (and {count} times more)"));
        }
        eprintln!("{text}");
        log(&text);
        *last = Some((text, std::time::Instant::now(), 0));
    }

    fn flush(&self) {}
}

/// Starts writing the libraries' warnings (see [`LibraryLog`]).
pub fn init_library_log() {
    static LOGGER: OnceLock<LibraryLog> = OnceLock::new();
    let logger = LOGGER.get_or_init(|| LibraryLog {
        last: std::sync::Mutex::new(None),
        frames: std::env::var_os("CP_LOG_FRAMES").is_some(),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(if logger.frames {
            log::LevelFilter::Trace
        } else {
            log::LevelFilter::Warn
        });
    }
}

/// Appends a line to the log (kept under a megabyte).
pub fn log(line: &str) {
    let Some(f) = log_file() else {
        return;
    };
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(&f).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&f);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&f)
    {
        let _ = writeln!(
            file,
            "[{now}] ChessPenguin {}: {line}",
            env!("CARGO_PKG_VERSION")
        );
    }
}

/// A message box for what ended the program (there may be no window to show it in).
pub fn fatal(text: &str) {
    let path = log_file()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("ChessPenguin")
        .set_description(format!(
            "{}\n\n{text}\n\n{}",
            crate::i18n::tr("ChessPenguin could not start."),
            crate::i18n::trf("Details are in {path}", &[("path", &path)]),
        ))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Panics go to the log; one on the main thread also to a message box.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("?").to_owned();
        let backtrace = std::backtrace::Backtrace::force_capture();
        log(&format!("panic in thread {name}: {info}\n{backtrace}"));
        default(info);
        if name == "main" {
            fatal(&info.to_string());
        }
    }));
}
