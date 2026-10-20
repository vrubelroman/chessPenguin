//! The window "Games from Lichess and Chess.com": one nickname (another one for
//! Chess.com if it differs), the platforms to look on, what was found, then the
//! downloads with their progress. Only the platforms that answered the probe at start
//! are offered (`online::Probe`).

use egui::{RichText, Ui};

use crate::board_search::Background;
use crate::i18n::{tr, trf};
use crate::online::{self, Account, Done, Failure, Lookup, Platform, Probe};
use crate::settings::Settings;

/// What the window asks of the application.
pub enum OnlineEvent {
    /// A database was downloaded (or partly, then cancelled): put it on the shelf.
    Added(std::path::PathBuf),
    /// A database got new games: reload it.
    Updated(std::path::PathBuf),
}

struct Job {
    /// One account, or several downloaded into one database.
    accounts: Vec<Account>,
    /// Updating a database already on the shelf (not a first download).
    update: bool,
    work: Background<Result<Done, Failure>>,
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
    /// Per finished download: the platforms ("Lichess + Chess.com"), the database name
    /// and the outcome.
    results: Vec<(String, String, Result<Done, Failure>)>,
    /// Two accounts found: download them into separate databases (one by default).
    separate: bool,
    /// Dev hook (screenshots): look the nickname up once the probe answered, and
    /// download what was found.
    auto_find: bool,
    auto_download: bool,
}

fn count(n: u64) -> String {
    crate::large_db::grouped(n as usize)
}

/// "Lichess + Chess.com".
fn platforms(accounts: &[Account]) -> String {
    accounts
        .iter()
        .map(|a| a.platform.name())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The file name of a database without the extension.
fn db_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl OnlineDialog {
    /// Opens the window with the nicknames used last time.
    pub fn open_with(&mut self, settings: &Settings) {
        if !self.open && self.jobs.is_empty() && self.lookup.is_none() {
            self.nick = settings.online_nick.clone();
            self.other = settings.online_nick_chesscom.clone();
            self.pick = [true, true];
            self.found.clear();
        }
        self.open = true;
    }

    /// Dev hook: `CP_ONLINE=<nick>` opens the window and looks the nickname up,
    /// `CP_ONLINE_DOWNLOAD=1` then downloads (screenshots, manual checks).
    pub fn from_env() -> OnlineDialog {
        let mut d = OnlineDialog::default();
        if let Ok(nick) = std::env::var("CP_ONLINE") {
            d.open = true;
            d.nick = nick;
            d.pick = [true, true];
            d.auto_find = true;
            d.auto_download = std::env::var("CP_ONLINE_DOWNLOAD").is_ok();
        }
        d
    }

    /// Databases being downloaded or updated.
    pub fn busy(&self) -> Vec<std::path::PathBuf> {
        self.jobs
            .iter()
            .filter_map(|j| online::combined_path(&j.accounts))
            .collect()
    }

    /// Fetches the new games of an online database on the shelf.
    pub fn update(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        let Some(state) = online::State::load(path) else {
            return;
        };
        if self.busy().iter().any(|p| p == path) {
            return;
        }
        self.start(state.accounts(), true, ctx);
    }

    fn start(&mut self, accounts: Vec<Account>, update: bool, ctx: &egui::Context) {
        if accounts.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let a = accounts.clone();
        if let Some(work) = Background::spawn(ctx, now, move |progress, cancel| {
            Some(online::download(&a, progress, cancel))
        }) {
            self.jobs.push(Job {
                accounts,
                update,
                work,
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
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        probe: &Probe,
        settings: &mut Settings,
    ) -> (Vec<OnlineEvent>, bool) {
        let mut events = Vec::new();
        let mut changed = false;
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
                let r = r.unwrap_or(Err(Failure::Network("stopped".into())));
                if let Ok(done) = &r {
                    if job.update {
                        if done.added > 0 {
                            events.push(OnlineEvent::Updated(done.path.clone()));
                        }
                    } else if done.total > 0 {
                        events.push(OnlineEvent::Added(done.path.clone()));
                    }
                }
                let label = platforms(&job.accounts);
                let name = online::combined_path(&job.accounts)
                    .map(|p| db_name(&p))
                    .unwrap_or_default();
                self.results.retain(|(_, n, _)| *n != name);
                self.results.push((label, name, r));
                // An update finished with the window closed: say so once it is opened.
            } else {
                i += 1;
            }
        }
        if !self.jobs.is_empty() || self.lookup.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if !self.open {
            return (events, changed);
        }
        let mut open = true;
        egui::Window::new(tr("Your games from Lichess and Chess.com"))
            .open(&mut open)
            .default_width(520.0)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 120.0))
            .show(ctx, |ui| {
                changed |= self.body(ui, probe, settings);
            });
        if !open {
            self.open = false;
        }
        (events, changed)
    }

    fn body(&mut self, ui: &mut Ui, probe: &Probe, settings: &mut Settings) -> bool {
        let mut changed = false;
        ui.label(
            RichText::new(tr(
                "Your online games become a database here, to search and analyse. Only the nickname is needed, no login.",
            ))
            .weak(),
        );
        ui.add_space(6.0);
        let both = probe.has(Platform::Lichess) && probe.has(Platform::ChessCom);
        ui.horizontal(|ui| {
            ui.label(tr("Nickname"));
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.nick)
                    .hint_text(tr("as on the site"))
                    .desired_width(220.0),
            );
            if r.changed() {
                self.found.clear();
            }
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
                        clear |= ui
                            .add(egui::TextEdit::singleline(o).desired_width(180.0))
                            .changed();
                        same = ui
                            .small_button("✕")
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
            if ui
                .add_enabled(
                    !wanted.is_empty() && valid && !searching,
                    egui::Button::new(tr("Find")),
                )
                .clicked()
                || (auto && !wanted.is_empty() && valid)
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
                settings.online_nick = self.nick.trim().to_owned();
                settings.online_nick_chesscom = self.other.as_ref().map(|o| o.trim().to_owned());
                changed = true;
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
            ui.add_space(4.0);
            let mut start: Vec<Account> = Vec::new();
            for (p, l) in &self.found {
                ui.horizontal(|ui| match l {
                    Lookup::Found(a) => {
                        let what = match (a.games, a.months) {
                            (Some(g), _) => {
                                let secs = g / online::LICHESS_RATE;
                                trf(
                                    "{p}: {name} — up to {n} games (about {m} min the first time)",
                                    &[
                                        ("p", &p.name()),
                                        ("name", &a.username),
                                        ("n", &count(g)),
                                        ("m", &secs.div_ceil(60).max(1)),
                                    ],
                                )
                            }
                            (_, Some(m)) => trf(
                                "{p}: {name} — games of {m} months",
                                &[("p", &p.name()), ("name", &a.username), ("m", &m)],
                            ),
                            _ => format!("{}: {}", p.name(), a.username),
                        };
                        ui.label(format!("✔ {what}"));
                        if !self.jobs.iter().any(|j| j.accounts.contains(a)) {
                            start.push(a.clone());
                        }
                    }
                    Lookup::NotFound => {
                        ui.label(
                            RichText::new(trf("✖ {p}: no such player", &[("p", &p.name())])).weak(),
                        );
                    }
                    Lookup::Failed(e) => {
                        ui.label(
                            RichText::new(trf(
                                "✖ {p}: no answer ({e})",
                                &[("p", &p.name()), ("e", e)],
                            ))
                            .color(ui.visuals().warn_fg_color),
                        );
                    }
                });
            }
            // Both platforms: one database by default (the player's games together).
            let together = start.len() > 1 && !self.separate;
            if start.len() > 1 {
                let mut one = !self.separate;
                if ui
                    .checkbox(
                        &mut one,
                        tr("Put the games of both sites into one database"),
                    )
                    .on_hover_text(tr("Otherwise each site gets a database of its own"))
                    .changed()
                {
                    self.separate = !one;
                }
            }
            // Where the games go, and what is there already.
            let targets: Vec<Vec<Account>> = if together {
                vec![start.clone()]
            } else {
                start.iter().map(|a| vec![a.clone()]).collect()
            };
            for t in &targets {
                if let Some(path) = online::combined_path(t) {
                    let text = match online::State::load(&path) {
                        Some(s) => trf(
                            "Into «{name}» (already {n} games there; only new ones are added)",
                            &[("name", &db_name(&path)), ("n", &count(s.games))],
                        ),
                        None => trf(
                            "Into the new database «{name}»",
                            &[("name", &db_name(&path))],
                        ),
                    };
                    ui.label(RichText::new(text).weak());
                }
            }
            let auto = std::mem::take(&mut self.auto_download) && !start.is_empty();
            if !start.is_empty() && (ui.button(tr("⬇ Download")).clicked() || auto) {
                // From the window the database goes (back) on the shelf, also when it
                // was downloaded before and only gets the new games now.
                for t in targets {
                    self.start(t, false, ui.ctx());
                }
            }
        }
        if !self.jobs.is_empty() || !self.results.is_empty() {
            ui.separator();
        }
        let mut stop = None;
        for (k, j) in self.jobs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                let n = j.work.progress() as u64;
                let known: u64 = j.accounts.iter().filter_map(|a| a.games).sum();
                let label = platforms(&j.accounts);
                let text = if known > 0 && j.accounts.iter().all(|a| a.games.is_some()) {
                    trf(
                        "{p}: {n} of up to {t} games",
                        &[("p", &label), ("n", &count(n)), ("t", &count(known))],
                    )
                } else {
                    trf("{p}: {n} games", &[("p", &label), ("n", &count(n))])
                };
                ui.label(text);
                if ui
                    .small_button(tr("Stop"))
                    .on_hover_text(tr(
                        "What is downloaded is kept; Update continues from there",
                    ))
                    .clicked()
                {
                    stop = Some(k);
                }
            });
        }
        if let Some(k) = stop {
            self.jobs[k].work.cancel();
        }
        for (p, name, r) in &self.results {
            let p = p.as_str();
            let line = match r {
                Ok(d) if d.cancelled => trf(
                    "{p}: stopped, {t} games kept in «{name}»",
                    &[("p", &p), ("t", &count(d.total)), ("name", name)],
                ),
                Ok(d) if d.added == 0 => trf(
                    "{p}: nothing new, {t} games in «{name}»",
                    &[("p", &p), ("t", &count(d.total)), ("name", name)],
                ),
                Ok(d) => trf(
                    "{p}: +{n} games, {t} in «{name}»",
                    &[
                        ("p", &p),
                        ("n", &count(d.added)),
                        ("t", &count(d.total)),
                        ("name", name),
                    ],
                ),
                Err(Failure::RateLimited) => trf(
                    "{p}: the site asks to wait a minute; then try again",
                    &[("p", &p)],
                ),
                Err(e) => trf("{p}: failed ({e})", &[("p", &p), ("e", e)]),
            };
            ui.label(line);
        }
        changed
    }
}
