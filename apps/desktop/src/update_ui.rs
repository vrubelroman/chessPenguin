//! The update window: the new version and the one step this installation needs (see
//! `update.rs`): update and restart; download and open the disk image; or download the
//! package and copy the command that installs it. Closing the window does not stop a
//! download (the button next to Settings shows its progress).

use egui::{RichText, Ui};

use crate::board_search::Background;
use crate::i18n::{tr, trf};
use crate::update::{self, Applied, Install, Release};

/// What the window asks of the application.
pub enum UpdateEvent {
    /// Quit (asking about unsaved games as usual) and start this executable.
    Restart(std::path::PathBuf),
    /// Quit (to replace the app from the disk image, or to start the installed package).
    Quit,
    CopyText(String),
    /// Hide the offer of this version.
    Skip(String),
}

#[derive(Default)]
pub struct UpdateDialog {
    pub open: bool,
    /// Dev hook (screenshots, manual checks): `CP_UPDATE_OPEN=1` opens the window once a
    /// newer release is known, `=start` also starts the update.
    auto: Option<bool>,
    job: Option<(u64, Background<Result<Applied, String>>)>,
    result: Option<Result<Applied, String>>,
}

fn mb(bytes: u64) -> String {
    if bytes < 1_000_000 {
        tr("less than 1").to_owned()
    } else {
        format!("{:.0}", bytes as f64 / 1e6)
    }
}

/// The installation for the footnote (for support, not for the decision).
fn install_name(kind: &Install) -> String {
    match kind {
        Install::Deb => tr("deb package").to_owned(),
        Install::Rpm => tr("rpm package").to_owned(),
        Install::Arch => tr("Arch Linux package").to_owned(),
        Install::Tarball(_) => tr("archive (tar.gz)").to_owned(),
        Install::Flatpak => "Flatpak".to_owned(),
        Install::Aur => "AUR (chesspenguin-bin)".to_owned(),
        other => other.label().to_owned(),
    }
}

/// A failure in words a chess player can act on; the technical text goes under
/// "Details".
fn failure_text(e: &str) -> String {
    let lower = e.to_lowercase();
    if e == "checksum" {
        tr("The file got damaged while downloading, so it was not installed. Try again.").to_owned()
    } else if e == "cancelled" {
        tr("The download was cancelled.").to_owned()
    } else if lower.contains("no file for this installation")
        || lower.contains("not in sha256sums")
        || lower.contains("no sha256sums")
    {
        tr("This release has no file for your system yet. Download the new version from the website.").to_owned()
    } else if lower.contains("permission denied")
        || lower.contains("no space")
        || lower.contains("read-only")
        || lower.contains("os error 13")
        || lower.contains("os error 28")
        || lower.contains("os error 30")
    {
        tr("The file could not be saved: no permission to write there, or the disk is full.")
            .to_owned()
    } else {
        tr("The new version could not be downloaded: no connection to the server. Check the internet and try again.").to_owned()
    }
}

impl UpdateDialog {
    pub fn from_env() -> UpdateDialog {
        let auto = std::env::var("CP_UPDATE_OPEN").ok().map(|v| v == "start");
        UpdateDialog {
            auto,
            ..UpdateDialog::default()
        }
    }

    /// The update is under way or done (its button stays even for a skipped version).
    pub fn active(&self) -> bool {
        self.job.is_some() || matches!(self.result, Some(Ok(_)))
    }

    /// What the button next to Settings says: the offer, the download's progress, or
    /// that the new version waits for a restart / for its command.
    pub fn badge(&self, release: &Release) -> String {
        if let Some((size, job)) = &self.job {
            let pct = if *size > 0 {
                (100 * job.progress() as u64 / size).min(100)
            } else {
                0
            };
            return trf("⬇ {v}: {p} %", &[("v", &release.version), ("p", &pct)]);
        }
        match &self.result {
            Some(Ok(Applied::Restart(_))) => trf("⟳ Restart for {v}", &[("v", &release.version)]),
            Some(Ok(Applied::Command { .. } | Applied::DiskImageOpened(_))) => {
                trf("⬆ {v}: install", &[("v", &release.version)])
            }
            _ => trf("⬆ Update {v}", &[("v", &release.version)]),
        }
    }

    fn start(&mut self, release: &Release, kind: &Install, ctx: &egui::Context) {
        let size = release.asset_for(kind).map_or(0, |a| a.size);
        let (r, k) = (release.clone(), kind.clone());
        let now = ctx.input(|i| i.time);
        self.result = None;
        self.job = Background::spawn(ctx, now, move |progress, cancel| {
            Some(update::update(&r, &k, progress, cancel))
        })
        .map(|j| (size, j));
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        release: Option<&Release>,
        kind: &Install,
    ) -> Vec<UpdateEvent> {
        let mut events = Vec::new();
        if let Some((_, job)) = &self.job
            && let Some(r) = job.poll()
        {
            self.result = Some(r.unwrap_or_else(|| Err("cancelled".into())));
            self.job = None;
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let Some(release) = release else {
            return events;
        };
        if let Some(start) = self.auto.take() {
            self.open = true;
            if start {
                self.start(release, kind, ctx);
            }
        }
        if !self.open {
            return events;
        }
        let mut open = true;
        egui::Window::new(tr("ChessPenguin update"))
            .open(&mut open)
            .collapsible(false)
            .default_width(560.0)
            .max_width(620.0)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 110.0))
            .show(ctx, |ui| self.body(ui, release, kind, &mut events));
        if !open {
            // A running download goes on; the button next to Settings shows it.
            self.open = false;
        }
        events
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        release: &Release,
        kind: &Install,
        events: &mut Vec<UpdateEvent>,
    ) {
        let asset = release.asset_for(kind);
        // Running.
        if let Some((size, job)) = &self.job {
            heading(ui, release);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(trf(
                    "Downloading… {done} of {total} MB",
                    &[
                        ("done", &format!("{:.0}", job.progress() as f64 / 1e6)),
                        ("total", &mb(*size)),
                    ],
                ));
                if ui.button(tr("Cancel")).clicked() {
                    job.cancel();
                }
            });
            if *size > 0 {
                ui.add(egui::ProgressBar::new(
                    (job.progress() as f32 / *size as f32).min(1.0),
                ));
            }
            ui.label(RichText::new(tr("You can close this window: the download goes on.")).weak());
            return;
        }
        // Finished.
        match &self.result {
            Some(Ok(Applied::Restart(exe))) => {
                ui.label(
                    RichText::new(trf("Version {v} is installed", &[("v", &release.version)]))
                        .heading(),
                );
                ui.label(tr("It starts after a restart. Unsaved games are asked about first; databases and settings stay as they are."));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary(ui, tr("⟳ Restart now")).clicked() {
                        events.push(UpdateEvent::Restart(exe.clone()));
                    }
                    if ui.button(tr("Later")).clicked() {
                        self.open = false;
                    }
                });
                return;
            }
            Some(Ok(Applied::DiskImageOpened(_))) => {
                ui.label(RichText::new(tr("The disk image is open")).heading());
                ui.label(tr(
                    "1. Quit ChessPenguin (the button below): a running app cannot be replaced.",
                ));
                ui.label(tr("2. In the window of the disk image, drag the ChessPenguin icon onto the Applications folder and choose Replace."));
                ui.label(tr("3. Start ChessPenguin from Applications."));
                ui.add_space(6.0);
                if crate::widgets::primary(ui, tr("Quit ChessPenguin")).clicked() {
                    events.push(UpdateEvent::Quit);
                }
                return;
            }
            Some(Ok(Applied::Command { file, command })) => {
                ui.label(RichText::new(tr("The new version is downloaded")).heading());
                ui.horizontal_wrapped(|ui| {
                    let dir = file
                        .parent()
                        .and_then(|d| d.file_name())
                        .map(|n| format!("«{}»", n.to_string_lossy()))
                        .unwrap_or_default();
                    ui.label(trf("The file is in the folder {dir}.", &[("dir", &dir)]));
                    if ui.link(tr("Open the folder")).clicked()
                        && let Some(d) = file.parent()
                    {
                        open_folder(d);
                    }
                });
                ui.add_space(4.0);
                ui.label(tr("1. Copy the command."));
                command_line(ui, command, events);
                if matches!(kind, Install::Flatpak) {
                    ui.label(tr("2. Open the Terminal of your system (not inside the Flatpak), paste the command (Ctrl+Shift+V) and press Enter."));
                } else {
                    ui.label(tr("2. Open the Terminal, paste the command (Ctrl+Shift+V) and press Enter. It asks for your password: while you type it nothing is shown, that is normal."));
                }
                ui.label(tr(
                    "3. When the command has finished, start ChessPenguin again.",
                ));
                ui.add_space(6.0);
                if ui.button(tr("Quit ChessPenguin")).clicked() {
                    events.push(UpdateEvent::Quit);
                }
                return;
            }
            _ => {}
        }
        heading(ui, release);
        let mut retry = false;
        if let Some(Err(e)) = &self.result {
            ui.label(RichText::new(failure_text(e)).color(ui.visuals().warn_fg_color));
            if e != "cancelled" && e != "checksum" {
                egui::CollapsingHeader::new(tr("Details"))
                    .id_salt("update_details")
                    .show(ui, |ui| {
                        ui.label(RichText::new(e).monospace().weak());
                    });
            }
            ui.add_space(4.0);
            retry = true;
        }
        let action = |retry: bool, first: &'static str| if retry { tr("Try again") } else { first };
        // What to do.
        match (kind, asset) {
            (Install::Aur, _) => {
                ui.label(tr(
                    "ChessPenguin is installed from the AUR. Update it as usual, for example:",
                ));
                command_line(ui, update::AUR_COMMAND, events);
                ui.label(RichText::new(trf("or: {c}", &[("c", &update::AUR_COMMAND_PARU)])).weak());
            }
            (k, Some(a)) if k.replaces_itself() => {
                ui.label(trf(
                    "ChessPenguin downloads the new version ({mb} MB), checks it and puts it in place of this one. Databases, games and settings are not touched.",
                    &[("mb", &mb(a.size))],
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary(ui, action(retry, tr("⬆ Update"))).clicked() {
                        self.start(release, k, ui.ctx());
                    }
                    self.not_now(ui, release, events);
                });
            }
            (Install::Mac, Some(a)) => {
                ui.label(trf(
                    "ChessPenguin downloads the disk image ({mb} MB), checks it and opens it; then you drag the app over the old one, as when you installed it.",
                    &[("mb", &mb(a.size))],
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary(ui, action(retry, tr("⬇ Download"))).clicked() {
                        self.start(release, kind, ui.ctx());
                    }
                    self.not_now(ui, release, events);
                });
            }
            (k, Some(a)) => {
                ui.label(trf(
                    "ChessPenguin was installed through the system (its package manager), so it cannot replace itself. It downloads the new version ({mb} MB) into Downloads; to install it you run one command in a terminal, which you can copy.",
                    &[("mb", &mb(a.size))],
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary(ui, action(retry, tr("⬇ Download"))).clicked() {
                        self.start(release, k, ui.ctx());
                    }
                    self.not_now(ui, release, events);
                });
            }
            (Install::Mac | Install::Windows(_), None) => {
                ui.label(tr("This release has no file for your system yet. Download the new version from the website:"));
                ui.hyperlink_to(update::DOWNLOAD_PAGE, update::DOWNLOAD_PAGE);
            }
            _ => {
                ui.label(tr("Download the new version from the website:"));
                ui.hyperlink_to(update::DOWNLOAD_PAGE, update::DOWNLOAD_PAGE);
            }
        }
        if !matches!(kind, Install::Other) {
            ui.add_space(4.0);
            ui.label(
                RichText::new(trf(
                    "Installation: {kind}",
                    &[("kind", &install_name(kind))],
                ))
                .small()
                .weak(),
            );
        }
    }

    /// "Not now" and "Skip this version".
    fn not_now(&mut self, ui: &mut Ui, release: &Release, events: &mut Vec<UpdateEvent>) {
        if ui.button(tr("Not now")).clicked() {
            self.open = false;
        }
        if ui
            .link(tr("Skip this version"))
            .on_hover_text(tr(
                "The button next to Settings is hidden until the next version; Settings → About still offers this one",
            ))
            .clicked()
        {
            events.push(UpdateEvent::Skip(release.version.clone()));
            self.open = false;
        }
    }
}

/// "Version 0.8.0 is out" with the current version and the release notes under it.
fn heading(ui: &mut Ui, release: &Release) {
    ui.label(RichText::new(trf("Version {v} is out", &[("v", &release.version)])).heading());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(trf(
                "You have {old}",
                &[("old", &env!("CARGO_PKG_VERSION"))],
            ))
            .weak(),
        );
        ui.label(RichText::new("·").weak());
        ui.hyperlink_to(tr("What's new"), &release.page);
    });
    ui.add_space(6.0);
}

/// A command in a monospace box with a Copy button beside it.
fn command_line(ui: &mut Ui, command: &str, events: &mut Vec<UpdateEvent>) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 110.0).max(200.0);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .corner_radius(4.0)
            .show(ui, |ui| {
                ui.set_max_width(width);
                ui.add(egui::Label::new(RichText::new(command).monospace()).wrap());
            });
        if crate::widgets::primary(ui, tr("Copy")).clicked() {
            events.push(UpdateEvent::CopyText(command.to_owned()));
        }
    });
}

/// Shows a folder in the file manager.
fn open_folder(dir: &std::path::Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}
