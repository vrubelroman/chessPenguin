//! Games from Lichess and Chess.com on the desktop: the databases live in
//! `<local data dir>/online/`; the downloads themselves are the `online-games` crate's
//! (shared with the Android app). Both public APIs are probed at start; a platform that
//! does not answer as expected (offline, the API changed) is simply not offered. The
//! web version has no online games: there only the names of such databases are known.

use std::path::{Path, PathBuf};

pub use online_games::{Account, Platform, State};
#[cfg(not(web))]
pub use online_games::{
    Done, Failure, LICHESS_RATE, Lookup, Progress, download, lookup, probe_all, valid_nick,
};

/// Where the online databases live.
pub fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
        .map(|d| d.data_local_dir().join("online"))
}

/// The online databases already holding `account` (see [`online_games::holding`]).
pub fn holding(account: &Account) -> Vec<PathBuf> {
    dir().map_or_else(Vec::new, |d| online_games::holding(&d, account))
}

/// An online database (it has a state file).
pub fn is_online(pgn: &Path) -> bool {
    dir().is_some_and(|d| online_games::is_online(&d, pgn))
}

/// Where the found accounts go (see [`online_games::targets`]).
pub fn targets(start: &[Account], together: bool) -> Vec<(Vec<Account>, PathBuf)> {
    dir().map_or_else(Vec::new, |d| online_games::targets(&d, start, together))
}

/// "5 min ago", "3 h ago", "2 d ago" for a time in Unix seconds.
pub fn ago(t: i64) -> String {
    use crate::i18n::trf;
    let s = (online_games::now_secs() - t).max(0);
    match s {
        0..3600 => trf("{n} min ago", &[("n", &(s / 60).max(1))]),
        3600..86400 => trf("{n} h ago", &[("n", &(s / 3600))]),
        _ => trf("{n} d ago", &[("n", &(s / 86400))]),
    }
}

#[cfg(not(web))]
/// The probe results and what the window shows, kept by the application.
pub struct Probe {
    job: Option<std::sync::mpsc::Receiver<[bool; 2]>>,
    pub available: [bool; 2],
    /// When the last probe finished (a platform that did not answer is asked again
    /// after [`Probe::RETRY`]: the machine may have come online).
    last: Option<web_time::Instant>,
    ctx: egui::Context,
}

#[cfg(not(web))]
impl Probe {
    const RETRY: std::time::Duration = std::time::Duration::from_secs(300);

    /// Starts probing both platforms in the background.
    pub fn start(ctx: &egui::Context) -> Probe {
        let mut p = Probe {
            job: None,
            available: [false; 2],
            last: None,
            ctx: ctx.clone(),
        };
        p.spawn();
        p
    }

    fn spawn(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("online-probe".into())
            .spawn(move || {
                let _ = tx.send(probe_all());
                ctx.request_repaint();
            });
        self.job = spawned.ok().map(|_| rx);
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.job
            && let Ok(a) = rx.try_recv()
        {
            // A platform that answered once stays offered for the session (a later
            // failure shows as an error in the window).
            for (have, now) in self.available.iter_mut().zip(a) {
                *have |= now;
            }
            self.job = None;
            self.last = Some(web_time::Instant::now());
        }
        if self.job.is_none() && !self.available.iter().all(|a| *a) {
            match self.last {
                Some(t) if t.elapsed() >= Self::RETRY => self.spawn(),
                // Wake up for the retry even when nothing else repaints.
                Some(t) => self.ctx.request_repaint_after(Self::RETRY - t.elapsed()),
                None => {}
            }
        }
    }

    pub fn any(&self) -> bool {
        self.available.iter().any(|a| *a)
    }

    pub fn has(&self, p: Platform) -> bool {
        self.available[p.index()]
    }
}
