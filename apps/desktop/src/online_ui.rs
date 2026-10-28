//! The window "Games from Lichess and Chess.com": a player's nickname (anyone's: the
//! user's own, a pupil's, an opponent's; another one for Chess.com if it differs), the
//! platforms to look on, what was found, then the downloads with their progress. Only the
//! platforms that answered the probe at start are offered (`online::Probe`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use egui::{RichText, Ui};

use crate::board_search::Background;
use crate::i18n::{games, tr, trf};
use crate::online::{self, Account, Done, Failure, Lookup, Platform, Probe};

/// What the window asks of the application.
pub enum OnlineEvent {
    /// A database was downloaded (or partly, then cancelled): put it on the shelf.
    Added(PathBuf),
    /// A database got new games: reload it.
    Updated(PathBuf),
    /// Show this database (and the window closes).
    Open(PathBuf),
    /// A download finished while the window was closed: say how it went.
    Message(String),
}

struct Job {
    /// One account, or several downloaded into one database.
    accounts: Vec<Account>,
    path: PathBuf,
    /// Updating a database already on the shelf (not a first download).
    update: bool,
    /// The database is new (so the totals of the accounts are what is left to fetch).
    fresh: bool,
    status: Arc<online::Progress>,
    work: Background<Result<Done, Failure>>,
    /// When Stop was pressed: a download that does not stop soon (a stalled
    /// connection) leaves the list; its database stays claimed until it ends.
    stopped: Option<std::time::Instant>,
    /// Started on its own at the program's start: says something only when it added
    /// games (no "no new games" on every start, no error over a network that is off).
    quiet: bool,
}

/// A finished download.
struct Outcome {
    accounts: Vec<Account>,
    path: PathBuf,
    update: bool,
    /// The platform that failed (`None`: the disk, or not known).
    platform: Option<Platform>,
    result: Result<Done, Failure>,
}

#[derive(Default)]
pub struct OnlineDialog {
    pub open: bool,
    nick: String,
    /// A different nickname on Chess.com, when asked for.
    other: Option<String>,
    pick: [bool; 2],
    lookup: Option<Background<Vec<(Platform, Lookup)>>>,
    found: Vec<(Platform, Lookup)>,
    /// The nicknames `found` answers.
    found_for: (String, String),
    jobs: Vec<Job>,
    results: Vec<Outcome>,
    /// Two accounts found: download them into separate databases (one by default).
    separate: bool,
    /// Dev hook (screenshots): look the nickname up once the probe answered, and
    /// download what was found.
    auto_find: bool,
    auto_download: bool,
    /// Put the cursor into the nickname field (the window was just opened).
    focus: bool,
    /// Online databases to update on their own (at the start), one after another: the
    /// sites ask for one export at a time.
    queued: std::collections::VecDeque<PathBuf>,
}

/// Where the found accounts go (see [`online_games::targets`]).
fn targets(start: &[Account], together: bool) -> Vec<(Vec<Account>, PathBuf)> {
    online::targets(start, together)
}

/// The file name of a database without the extension.
fn db_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What went wrong, in words for a chess player (the technical text goes under
/// "Details").
fn failure_text(platform: Option<Platform>, e: &Failure) -> String {
    let p = platform.map_or("—", Platform::name);
    match e {
        Failure::Network(_) => trf(
            "{p} does not answer. Check the internet connection and try again.",
            &[("p", &p)],
        ),
        Failure::RateLimited => trf(
            "{p} asks for a pause: too many requests in a row. Wait a minute and try again; what is downloaded is kept.",
            &[("p", &p)],
        ),
        Failure::Unexpected(_) => trf(
            "{p} answered in a way ChessPenguin does not understand: the site may have changed. Try later or update ChessPenguin.",
            &[("p", &p)],
        ),
        Failure::Disk(_) => {
            tr("Could not write the database: no permission to write or the disk is full.")
                .to_owned()
        }
        Failure::Busy => tr("This database is being downloaded already.").to_owned(),
    }
}

/// "“Name”: +12 games, 6 600 in all" and the like.
fn outcome_text(o: &Outcome) -> String {
    let name = db_name(&o.path);
    match &o.result {
        Ok(d) if d.cancelled => trf(
            "“{name}”: stopped, {total} in the database",
            &[("name", &name), ("total", &games(d.total as usize))],
        ),
        Ok(d) if d.added == 0 => trf("“{name}”: no new games", &[("name", &name)]),
        Ok(d) => trf(
            "“{name}”: +{added}, {total} in all",
            &[
                ("name", &name),
                ("added", &games(d.added as usize)),
                ("total", &games(d.total as usize)),
            ],
        ),
        Err(e) => trf(
            "“{name}”: {e}",
            &[("name", &name), ("e", &failure_text(o.platform, e))],
        ),
    }
}

impl OnlineDialog {
    /// Opens the window with an empty nickname, ready to type: the databases already
    /// downloaded fetch their new games with "⟳ New games", so the next search is
    /// usually for another player. A search still running keeps its nickname.
    pub fn open(&mut self) {
        if !self.open && self.lookup.is_none() {
            self.nick.clear();
            self.other = None;
            self.pick = [true, true];
            self.found.clear();
            self.focus = true;
        }
        self.open = true;
    }

    /// Dev hook: `CP_ONLINE=<nick>` (`<lichess>,<chess.com>` for two nicknames) opens the
    /// window and looks the nickname up, `CP_ONLINE_DOWNLOAD=1` then downloads
    /// (screenshots, manual checks).
    pub fn from_env() -> OnlineDialog {
        let mut d = OnlineDialog::default();
        if let Ok(nick) = std::env::var("CP_ONLINE") {
            d.open = true;
            let (nick, other) = match nick.split_once(',') {
                Some((a, b)) => (a.to_owned(), Some(b.to_owned())),
                None => (nick, None),
            };
            d.nick = nick;
            d.other = other;
            d.pick = [true, true];
            d.auto_find = true;
            d.auto_download = std::env::var("CP_ONLINE_DOWNLOAD").is_ok();
        }
        d
    }

    /// Databases being downloaded or updated.
    pub fn busy(&self) -> Vec<PathBuf> {
        self.jobs.iter().map(|j| j.path.clone()).collect()
    }

    /// The games downloaded so far by the running jobs (for the shelf card), if any run.
    pub fn running(&self) -> Option<usize> {
        (!self.jobs.is_empty()).then(|| self.jobs.iter().map(|j| j.work.progress()).sum())
    }

    /// Stops the downloads and waits a little for them to save what they fetched (on
    /// quitting: the next update then continues without duplicates). Returns the new
    /// databases among them, to put on the shelf with what they got so far.
    pub fn stop_all(&mut self) -> Vec<PathBuf> {
        let new = self
            .jobs
            .iter()
            .filter(|j| !j.update)
            .map(|j| j.path.clone())
            .collect();
        for j in &self.jobs {
            j.work.cancel();
        }
        let start = std::time::Instant::now();
        while !self.jobs.is_empty() && start.elapsed() < std::time::Duration::from_secs(3) {
            self.jobs.retain(|j| j.work.poll().is_none());
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        new
    }

    /// Fetches the new games of an online database on the shelf.
    pub fn update(&mut self, path: &Path, ctx: &egui::Context) {
        let Some(state) = online::State::load(path) else {
            return;
        };
        self.start(state.accounts(), path.to_owned(), true, ctx);
    }

    /// Updates these online databases on their own, one after another, once a site
    /// answers (the program's start: every downloaded database gets its new games).
    pub fn update_quietly(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            if !self.queued.contains(&p) {
                self.queued.push_back(p);
            }
        }
    }

    /// The next queued update, when nothing else is being downloaded.
    fn next_queued(&mut self, probe: &Probe, ctx: &egui::Context) {
        if !self.jobs.is_empty() || !probe.any() {
            return;
        }
        while let Some(path) = self.queued.pop_front() {
            if let Some(state) = online::State::load(&path) {
                self.start(state.accounts(), path, true, ctx);
                if let Some(j) = self.jobs.last_mut() {
                    j.quiet = true;
                }
                return;
            }
        }
    }

    fn start(&mut self, accounts: Vec<Account>, path: PathBuf, update: bool, ctx: &egui::Context) {
        if self.jobs.iter().any(|j| j.path == path) {
            return;
        }
        let fresh = online::State::load(&path).is_none();
        let now = ctx.input(|i| i.time);
        let a = accounts.clone();
        let status = Arc::new(online::Progress::default());
        let (s, p) = (status.clone(), path.clone());
        if let Some(work) = Background::spawn(ctx, now, move |progress, cancel| {
            Some(online::download(&a, &p, progress, cancel, &s))
        }) {
            self.results.retain(|o| o.path != path);
            self.jobs.push(Job {
                accounts,
                path,
                update,
                fresh,
                status,
                work,
                stopped: None,
                quiet: false,
            });
        }
    }

    fn nick_for(&self, p: Platform) -> String {
        match (p, &self.other) {
            (Platform::ChessCom, Some(o)) => o.trim().to_owned(),
            _ => self.nick.trim().to_owned(),
        }
    }

    /// Collects finished work; the window itself when open.
    pub fn show(&mut self, ctx: &egui::Context, probe: &Probe) -> Vec<OnlineEvent> {
        let mut events = Vec::new();
        if let Some(l) = &self.lookup
            && let Some(r) = l.poll()
        {
            self.found = r.unwrap_or_default();
            self.lookup = None;
        }
        let mut i = 0;
        while i < self.jobs.len() {
            if let Some(r) = self.jobs[i].work.poll() {
                let job = self.jobs.remove(i);
                let result = r.unwrap_or(Err(Failure::Network("stopped".into())));
                // What a failed download got before the failure is kept: a new
                // database goes on the shelf with it, an updated one is reloaded.
                let total = match &result {
                    Ok(d) => d.total,
                    Err(_) => online::State::load(&job.path).map_or(0, |s| s.games),
                };
                if job.update {
                    if result.as_ref().map_or(true, |d| d.added > 0) {
                        events.push(OnlineEvent::Updated(job.path.clone()));
                    }
                } else if total > 0 {
                    events.push(OnlineEvent::Added(job.path.clone()));
                }
                let platform = match &result {
                    Err(Failure::Disk(_)) | Ok(_) => None,
                    Err(_) => job
                        .accounts
                        .get(job.status.account.load(Ordering::Relaxed))
                        .map(|a| a.platform),
                };
                let outcome = Outcome {
                    accounts: job.accounts,
                    path: job.path,
                    update: job.update,
                    platform,
                    result,
                };
                // An update of its own speaks only of new games.
                let worth = !job.quiet || outcome.result.as_ref().is_ok_and(|d| d.added > 0);
                if worth {
                    if !self.open {
                        events.push(OnlineEvent::Message(outcome_text(&outcome)));
                    }
                    self.results.push(outcome);
                }
            } else if self.jobs[i]
                .stopped
                .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(10))
            {
                self.jobs.remove(i);
            } else {
                i += 1;
            }
        }
        self.next_queued(probe, ctx);
        if !self.jobs.is_empty() || self.lookup.is_some() || !self.queued.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if !self.open {
            return events;
        }
        let mut open = true;
        egui::Window::new(tr("Games from Lichess and Chess.com"))
            .open(&mut open)
            .default_width(540.0)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 120.0))
            .show(ctx, |ui| {
                self.body(ui, probe, &mut events);
            });
        if !open || events.iter().any(|e| matches!(e, OnlineEvent::Open(_))) {
            self.open = false;
        }
        events
    }

    fn body(&mut self, ui: &mut Ui, probe: &Probe, events: &mut Vec<OnlineEvent>) {
        ui.label(
            RichText::new(tr(
                "Any player's games — yours, a pupil's, an opponent's — become a database here, to search and analyse. Only the nickname is needed, no login.",
            ))
            .weak(),
        );
        ui.add_space(6.0);
        let both = probe.has(Platform::Lichess) && probe.has(Platform::ChessCom);
        let mut enter = false;
        ui.horizontal(|ui| {
            ui.label(tr("Player's nickname"));
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.nick)
                    .hint_text(tr("as on the site"))
                    .desired_width(220.0),
            );
            if std::mem::take(&mut self.focus) {
                r.request_focus();
            }
            if r.changed() {
                self.found.clear();
            }
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        });
        ui.horizontal(|ui| {
            for p in Platform::ALL {
                if probe.has(p) {
                    let i = p as usize;
                    if ui.checkbox(&mut self.pick[i], p.name()).changed() {
                        self.found.clear();
                    }
                }
            }
        });
        if both && self.pick[1] {
            let mut clear = false;
            let mut same = false;
            match &mut self.other {
                Some(o) => {
                    ui.horizontal(|ui| {
                        ui.label(tr("Nickname on Chess.com"));
                        let r = ui.add(egui::TextEdit::singleline(o).desired_width(180.0));
                        clear |= r.changed();
                        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        same = ui
                            .small_button("🗙")
                            .on_hover_text(tr("The same nickname"))
                            .clicked();
                    });
                }
                None => {
                    if ui.link(tr("A different nickname on Chess.com")).clicked() {
                        self.other = Some(self.nick.clone());
                        clear = true;
                    }
                }
            }
            if same {
                self.other = None;
                clear = true;
            }
            if clear {
                self.found.clear();
            }
        }
        let wanted: Vec<Platform> = Platform::ALL
            .into_iter()
            .filter(|p| probe.has(*p) && self.pick[*p as usize])
            .collect();
        let nicks = (
            self.nick_for(Platform::Lichess),
            self.nick_for(Platform::ChessCom),
        );
        let valid = wanted
            .iter()
            .all(|p| online::valid_nick(&self.nick_for(*p)));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let searching = self.lookup.is_some();
            let auto = self.auto_find && probe.any();
            if auto {
                self.auto_find = false;
            }
            let can = !wanted.is_empty() && valid && !searching;
            if ui.add_enabled(can, egui::Button::new(tr("Find"))).clicked()
                || ((auto || enter) && can)
            {
                let list: Vec<(Platform, String)> =
                    wanted.iter().map(|p| (*p, self.nick_for(*p))).collect();
                self.found_for = nicks.clone();
                let now = ui.input(|i| i.time);
                self.lookup = Background::spawn(ui.ctx(), now, move |_, _| {
                    Some(
                        list.into_iter()
                            .map(|(p, n)| (p, online::lookup(p, &n)))
                            .collect(),
                    )
                });
            }
            if searching {
                ui.add(egui::Spinner::new());
            } else if !self.nick.trim().is_empty() && !valid {
                ui.label(
                    RichText::new(tr("Letters, digits, _ and -, 2 to 30 characters"))
                        .color(ui.visuals().warn_fg_color),
                );
            }
        });
        if self.found_for == nicks && !self.found.is_empty() {
            self.found_ui(ui);
        }
        self.jobs_ui(ui);
        self.results_ui(ui, events);
    }

    /// What was found, the choice of one database or separate ones, and Download.
    fn found_ui(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let start: Vec<Account> = self
            .found
            .iter()
            .filter_map(|(_, l)| match l {
                Lookup::Found(a)
                    if !self
                        .jobs
                        .iter()
                        .any(|j| j.accounts.iter().any(|x| x.same(a))) =>
                {
                    Some(a.clone())
                }
                _ => None,
            })
            .collect();
        // Accounts some database already holds (then the "first time" estimate is wrong).
        let held = |a: &Account| !online::holding(a).is_empty();
        for (p, l) in &self.found {
            match l {
                Lookup::Found(a) => {
                    let what = match (a.games, a.months) {
                        (Some(g), _) if !held(a) => {
                            let secs = g / online::LICHESS_RATE;
                            trf(
                                "{p}: {name} — {games}, about {m} min the first time",
                                &[
                                    ("p", &p.name()),
                                    ("name", &a.username),
                                    ("games", &games(g as usize)),
                                    ("m", &secs.div_ceil(60).max(1)),
                                ],
                            )
                        }
                        (Some(g), _) => trf(
                            "{p}: {name} — {games}",
                            &[
                                ("p", &p.name()),
                                ("name", &a.username),
                                ("games", &games(g as usize)),
                            ],
                        ),
                        (_, Some(m)) => trf(
                            "{p}: {name} — months with games: {m}",
                            &[("p", &p.name()), ("name", &a.username), ("m", &m)],
                        ),
                        _ => format!("{}: {}", p.name(), a.username),
                    };
                    ui.label(format!("✔ {what}"));
                }
                Lookup::NotFound => {
                    ui.label(
                        RichText::new(trf(
                            "✖ {p}: no player with this nickname — check the spelling",
                            &[("p", &p.name())],
                        ))
                        .weak(),
                    );
                }
                Lookup::Failed(e) => {
                    ui.label(
                        RichText::new(format!("✖ {}", failure_text(Some(*p), e)))
                            .color(ui.visuals().warn_fg_color),
                    );
                }
            }
        }
        if start.is_empty() {
            return;
        }
        // Both platforms: one database by default (the player's games together).
        let together = start.len() > 1 && !self.separate;
        if start.len() > 1 {
            let mut one = !self.separate;
            if ui
                .checkbox(
                    &mut one,
                    tr("Merge the games of both sites into one database"),
                )
                .on_hover_text(tr("Otherwise each site gets a database of its own"))
                .changed()
            {
                self.separate = !one;
            }
        }
        // Where the games go, and what is there already.
        let targets = targets(&start, together);
        for (_, path) in &targets {
            let text = match online::State::load(path) {
                Some(s) => trf(
                    "Into “{name}” (already {n} there; only new games are added)",
                    &[("name", &db_name(path)), ("n", &games(s.games as usize))],
                ),
                None => trf(
                    "Into the new database “{name}”",
                    &[("name", &db_name(path))],
                ),
            };
            ui.label(RichText::new(text).weak());
        }
        ui.add_space(4.0);
        let auto = std::mem::take(&mut self.auto_download);
        if crate::widgets::primary(ui, tr("⬇ Download")).clicked() || auto {
            // From the window the database goes (back) on the shelf, also when it was
            // downloaded before and only gets the new games now.
            for (t, path) in targets {
                self.start(t, path, false, ui.ctx());
            }
        }
    }

    /// The running downloads: the database, the site and how far it got, Stop.
    fn jobs_ui(&mut self, ui: &mut Ui) {
        if self.jobs.is_empty() {
            return;
        }
        ui.separator();
        let mut stop = None;
        for (k, j) in self.jobs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(db_name(&j.path)).strong());
                if ui
                    .small_button(tr("Stop"))
                    .on_hover_text(tr(
                        "What is downloaded is kept; downloading again continues from there",
                    ))
                    .clicked()
                {
                    stop = Some(k);
                }
            });
            let s = &j.status;
            let Some(a) = j.accounts.get(s.account.load(Ordering::Relaxed)) else {
                continue;
            };
            let n = s.games.load(Ordering::Relaxed) as u64;
            match (a.platform, a.games) {
                (Platform::Lichess, Some(total)) if j.fresh && total > 0 => {
                    let left = total.saturating_sub(n) / online::LICHESS_RATE;
                    ui.label(trf(
                        "Lichess: {n} of about {t} · about {m} min left",
                        &[
                            ("n", &crate::large_db::grouped(n as usize)),
                            ("t", &crate::large_db::grouped(total as usize)),
                            ("m", &left.div_ceil(60).max(1)),
                        ],
                    ));
                    ui.add(egui::ProgressBar::new((n as f32 / total as f32).min(1.0)));
                }
                (Platform::ChessCom, _) if s.months_total.load(Ordering::Relaxed) > 0 => {
                    let (done, all) = (
                        s.months_done.load(Ordering::Relaxed),
                        s.months_total.load(Ordering::Relaxed),
                    );
                    ui.label(trf(
                        "Chess.com: month {i} of {n} · {games}",
                        &[("i", &done), ("n", &all), ("games", &games(n as usize))],
                    ));
                    ui.add(egui::ProgressBar::new(done as f32 / all as f32));
                }
                (p, _) => {
                    ui.label(format!("{}: {}", p.name(), games(n as usize)));
                }
            }
        }
        if let Some(k) = stop {
            self.jobs[k].work.cancel();
            self.jobs[k]
                .stopped
                .get_or_insert_with(std::time::Instant::now);
        }
        ui.label(
            RichText::new(tr(
                "You can close the window: the download goes on, and what is downloaded is kept.",
            ))
            .weak(),
        );
    }

    /// The finished downloads: how they went, Open the database / Try again.
    fn results_ui(&mut self, ui: &mut Ui, events: &mut Vec<OnlineEvent>) {
        if self.results.is_empty() {
            return;
        }
        ui.separator();
        let mut retry = None;
        for (k, o) in self.results.iter().enumerate() {
            match &o.result {
                Ok(d) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("✔ {}", outcome_text(o)));
                        if d.total > 0 && ui.button(tr("Open the database")).clicked() {
                            events.push(OnlineEvent::Open(o.path.clone()));
                        }
                    });
                }
                Err(e) => {
                    ui.label(
                        RichText::new(format!("⚠ {}", outcome_text(o)))
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(tr("Try again")).clicked() {
                            retry = Some(k);
                        }
                        egui::CollapsingHeader::new(tr("Details"))
                            .id_salt(("online-details", k))
                            .show(ui, |ui| {
                                ui.label(RichText::new(e.to_string()).small().weak());
                            });
                    });
                }
            }
        }
        if let Some(k) = retry {
            let o = self.results.remove(k);
            self.start(o.accounts, o.path, o.update, ui.ctx());
        }
    }
}
