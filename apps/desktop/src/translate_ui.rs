//! Translation in the interface (docs/design/translation-ui.md): the strip above the
//! notation and the marks on comments (`GameTranslation`, one per open game), and what
//! the whole program shares (`Hub`): the "How to translate?" window, the consent, the
//! ChatGPT sign-in, the connection check and the Settings › Translation tab.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use egui::{RichText, Ui};

use crate::board_search::Background;
use crate::chatgpt::{SignIn, SignedIn};
use crate::i18n::{Lang, tr, trf};
use crate::settings::Settings;
use crate::translate::{
    self, Backend, Connection, Event, Failure, GameContext, Job, Plan, Preset, TranslationSettings,
};

/// The model suggested for Ollama (`ollama pull …`).
pub const OLLAMA_MODEL: &str = "qwen3:8b";
/// Above this much foreign text a game waits for "Translate" even in automatic mode.
const BIG: usize = 60_000;
const CHATGPT_SETTINGS: &str = "https://chatgpt.com/#settings";

/// The name of a language in the interface language ("английский").
pub fn lang_name(l: whatlang::Lang) -> &'static str {
    tr(l.eng_name())
}

fn thousands(chars: usize) -> String {
    format!("{}", chars.div_ceil(1000).max(1))
}

// --- Marks in the notation ---------------------------------------------------------------

/// What the notation shows for a comment.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Translated(String),
    Pending,
    Bad,
}

// --- The program-wide part ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Step {
    #[default]
    Choose,
    /// Signing in with ChatGPT in the browser.
    Waiting,
    /// The check sentence is on its way.
    Checking,
    /// What will be sent, and "Start translating".
    Consent,
}

/// What the check found.
struct Checked {
    models: Vec<String>,
    model: String,
    result: Result<String, Failure>,
}

/// The state of the ChatGPT connection as last seen.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ChatGptState {
    #[default]
    Unknown,
    Works(String),
    NoPlan,
    NotEligible,
    Limit,
    SignedOut,
}

/// A message about the last sign-in attempt.
#[derive(Clone, Debug, PartialEq)]
enum SignInNote {
    TimedOut,
    Declined,
    Failed(String),
    NotPrepared(String),
}

#[derive(Default)]
pub struct Hub {
    /// The "How to translate?" window.
    pub setup_open: bool,
    step: Step,
    /// The choice in the window (the settings change only once it works).
    choice: Connection,
    preset: Option<Preset>,
    key: String,
    show_key: bool,
    check: Option<Background<Checked>>,
    check_result: Option<Result<String, Failure>>,
    /// Models of the key connection, from the last check.
    models: Vec<String>,
    sign_in: Option<SignIn>,
    note: Option<SignInNote>,
    copied: Option<f64>,
    pub chatgpt: ChatGptState,
    /// The consent window, standing alone (the connection was ready already).
    consent_open: bool,
    /// The comments of the game asking: how many, how many characters.
    consent_for: (usize, usize),
    /// A game may start translating now (consent given, or set up).
    start: bool,
    clear_confirm: bool,
    /// The settings tab should show the translation tab.
    pub open_settings: bool,
    /// A sign-in was started from the strip: it waits there.
    sign_in_from_strip: bool,
    /// The running sign-in or check belongs to a "Translate" (strip or setup window): when
    /// it works, consent is asked if needed and the translation goes on by itself.
    resume: bool,
    /// The setup or the consent was turned down: the game hides its offer.
    declined: bool,
    /// Bumped when the cache is cleared: open games look at their comments again.
    pub cache_epoch: u32,
    /// The connection was fixed in the settings: a game that failed tries again.
    retry_failed: bool,
}

impl Hub {
    /// The game asks to translate: set up first, ask for consent, or go.
    fn request(&mut self, s: &mut TranslationSettings, n: usize, chars: usize) -> bool {
        self.consent_for = (n, chars);
        if !s.ready() {
            self.open_setup(s);
            false
        } else if !s.allowed() {
            self.consent_open = true;
            false
        } else {
            true
        }
    }

    pub fn open_setup(&mut self, s: &TranslationSettings) {
        if !self.setup_open {
            self.step = Step::Choose;
            self.choice = s.connection;
            self.preset = Some(s.preset);
            self.key = translate::credentials()
                .key(s.preset.id())
                .unwrap_or("")
                .to_owned();
            self.check_result = None;
        }
        self.setup_open = true;
    }

    /// Dev hook (screenshots): opens the setup window.
    pub fn debug_open_setup(&mut self, s: &TranslationSettings, key: bool) {
        self.open_setup(s);
        if key {
            self.choice = Connection::Key;
        }
    }

    fn take_start(&mut self) -> bool {
        std::mem::take(&mut self.start)
    }

    fn take_retry_failed(&mut self) -> bool {
        std::mem::take(&mut self.retry_failed)
    }

    fn take_declined(&mut self) -> bool {
        std::mem::take(&mut self.declined)
    }

    /// A sign-in or check is going on for a "Translate" while its window is closed.
    fn busy_in_strip(&self) -> bool {
        !self.setup_open && self.resume && (self.sign_in.is_some() || self.check.is_some())
    }

    fn signing_in(&self) -> bool {
        self.sign_in.is_some()
    }

    /// Opens the browser for the sign-in (one at a time).
    fn sign_in(&mut self, ctx: &egui::Context, s: &mut TranslationSettings) {
        if self.sign_in.is_some() {
            return;
        }
        if s.host_id.is_empty() {
            s.host_id = crate::chatgpt::new_host_id();
        }
        self.note = None;
        match SignIn::start(ctx, &s.host_id, &s.client_id) {
            Ok(sign_in) => {
                ctx.open_url(egui::OpenUrl::new_tab(&sign_in.url));
                self.sign_in = Some(sign_in);
                self.step = Step::Waiting;
                self.copied = None;
            }
            Err(e) => self.note = Some(SignInNote::NotPrepared(e)),
        }
    }

    fn cancel_sign_in(&mut self) {
        if let Some(s) = self.sign_in.take() {
            s.cancel();
        }
        if self.step == Step::Waiting {
            self.step = Step::Choose;
        }
        self.sign_in_from_strip = false;
    }

    /// Checks the connection the window (or the settings) set up with the sentence.
    fn start_check(&mut self, ctx: &egui::Context, s: &TranslationSettings, now: f64) {
        let target = s.target();
        let preset = s.preset;
        let connection = s.connection;
        let base = s.base();
        let local = s.is_local();
        let wanted = match connection {
            Connection::ChatGpt => s.chatgpt_model.clone(),
            Connection::Key => s.model(),
        };
        let key = translate::credentials().key(preset.id()).map(str::to_owned);
        self.check_result = None;
        self.check = Background::spawn(ctx, now, move |_, cancel| {
            let (models, model) = match connection {
                Connection::ChatGpt => match crate::chatgpt::models() {
                    Ok(m) => {
                        let pick = if m.contains(&wanted) {
                            Some(wanted)
                        } else {
                            crate::chatgpt::default_model(&m)
                        };
                        (m, pick.unwrap_or_default())
                    }
                    Err(e) => {
                        return Some(Checked {
                            models: Vec::new(),
                            model: wanted,
                            result: Err(e),
                        });
                    }
                },
                Connection::Key => {
                    let listed = translate::list_models(&base, key.as_deref(), local);
                    match listed {
                        Ok(m) => {
                            let pick =
                                if !wanted.is_empty() && (m.is_empty() || m.contains(&wanted)) {
                                    wanted
                                } else if preset == Preset::Ollama
                                    && let Some(r) = m.iter().find(|x| x.starts_with(OLLAMA_MODEL))
                                {
                                    r.clone()
                                } else {
                                    m.first().cloned().unwrap_or(wanted)
                                };
                            (m, pick)
                        }
                        // Some services do not list models: try the request anyway.
                        Err(Failure::Service(..)) if !wanted.is_empty() => (Vec::new(), wanted),
                        Err(e) => {
                            return Some(Checked {
                                models: Vec::new(),
                                model: wanted,
                                result: Err(e),
                            });
                        }
                    }
                }
            };
            if model.is_empty() {
                return Some(Checked {
                    models,
                    model,
                    result: Err(Failure::NoModel(String::new())),
                });
            }
            let backend = match connection {
                Connection::ChatGpt => Backend::ChatGpt {
                    model: model.clone(),
                },
                Connection::Key => Backend::Chat {
                    base: base.clone(),
                    key: key.clone(),
                    model: model.clone(),
                    json_mode: matches!(preset, Preset::DeepSeek | Preset::OpenAi | Preset::Gemini),
                    local,
                },
            };
            let result = translate::check(&backend, target, cancel);
            Some(Checked {
                models,
                model,
                result,
            })
        });
    }

    /// Collects finished work: the sign-in and the check. `changed`: settings to save.
    pub fn poll(&mut self, ctx: &egui::Context, settings: &mut Settings) -> bool {
        let s = &mut settings.translation;
        let mut changed = false;
        let now = ctx.input(|i| i.time);
        if let Some(r) = self.sign_in.as_ref().and_then(SignIn::poll) {
            self.sign_in = None;
            match r {
                SignedIn::Ok { account, client_id } => {
                    s.account = account;
                    s.client_id = client_id;
                    s.connection = Connection::ChatGpt;
                    self.choice = Connection::ChatGpt;
                    changed = true;
                    self.step = Step::Checking;
                    self.start_check(ctx, s, now);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                SignedIn::NoPlan { account, client_id } => {
                    s.account = account;
                    s.client_id = client_id;
                    changed = true;
                    self.chatgpt = ChatGptState::NoPlan;
                    self.step = Step::Choose;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                SignedIn::Declined => {
                    self.resume = false;
                    self.note = Some(SignInNote::Declined);
                    self.step = Step::Choose;
                }
                SignedIn::TimedOut => {
                    self.resume = false;
                    self.note = Some(SignInNote::TimedOut);
                    self.step = Step::Choose;
                }
                SignedIn::Failed(e) => {
                    self.resume = false;
                    if e != "cancelled" {
                        self.note = Some(SignInNote::Failed(e));
                    }
                    self.step = Step::Choose;
                }
            }
            self.sign_in_from_strip = false;
        }
        if let Some(r) = self.check.as_ref().and_then(Background::poll) {
            self.check = None;
            let Some(c) = r else { return changed };
            match s.connection {
                Connection::ChatGpt => {
                    self.chatgpt = match &c.result {
                        Ok(_) => ChatGptState::Works(c.model.clone()),
                        Err(Failure::NotEligible(_)) => ChatGptState::NotEligible,
                        Err(Failure::PlanLimit) => ChatGptState::Limit,
                        Err(Failure::SignedOut) => ChatGptState::SignedOut,
                        Err(_) => self.chatgpt.clone(),
                    };
                    if c.result.is_ok() && s.chatgpt_model != c.model {
                        s.chatgpt_model = c.model.clone();
                        changed = true;
                    }
                }
                Connection::Key => {
                    self.models = c.models.clone();
                    if c.result.is_ok() && s.model() != c.model {
                        s.model = c.model.clone();
                        changed = true;
                    }
                }
            }
            let ok = c.result.is_ok();
            self.check_result = Some(c.result);
            let flow = self.resume || (self.setup_open && self.step == Step::Checking);
            self.resume = false;
            if !ok {
                if self.step == Step::Checking {
                    self.step = Step::Choose;
                }
            } else if flow {
                if s.allowed() {
                    // A model on this computer, or allowed before: translating starts.
                    s.auto = true;
                    s.never_offer = false;
                    changed = true;
                    self.setup_open = false;
                    self.start = true;
                } else if self.setup_open {
                    self.step = Step::Consent;
                } else {
                    self.consent_open = true;
                }
            } else if s.allowed() {
                // Fixed in the settings (a new key): a game that failed goes on.
                self.retry_failed = true;
            }
        }
        if self.sign_in.is_some() || self.check.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        changed
    }

    /// The consent was given: remember it and start.
    fn allow(&mut self, s: &mut TranslationSettings) {
        let id = s.service_id();
        if !s.allowed.contains(&id) {
            s.allowed.push(id);
        }
        s.auto = true;
        s.never_offer = false;
        self.consent_open = false;
        self.setup_open = false;
        self.start = true;
    }

    /// The windows: "How to translate?" and the consent. `changed`: settings to save.
    pub fn windows(&mut self, ctx: &egui::Context, settings: &mut Settings) -> bool {
        let mut changed = false;
        if self.setup_open {
            let mut open = true;
            let title = if self.step == Step::Consent {
                trf(
                    "Send comments to {service}?",
                    &[("service", &settings.translation.service_name())],
                )
            } else {
                tr("How to translate?").to_owned()
            };
            egui::Window::new(title)
                .id(egui::Id::new("translation_setup"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(460.0)
                .pivot(egui::Align2::CENTER_CENTER)
                .default_pos(ctx.content_rect().center())
                .show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    changed |= self.setup_ui(ui, &mut settings.translation);
                });
            if !open
                || ctx.input(|i| i.key_pressed(egui::Key::Escape))
                    && !ctx.egui_wants_keyboard_input()
            {
                self.setup_open = false;
                // Closed while signing in or checking: that goes on (`resume`).
                if self.sign_in.is_none() && self.check.is_none() {
                    self.declined = true;
                    self.resume = false;
                    settings.translation.declined += 1;
                    changed = true;
                }
            }
        }
        if self.consent_open {
            let mut close = false;
            egui::Modal::new(egui::Id::new("translation_consent")).show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(
                    RichText::new(trf(
                        "Send comments to {service}?",
                        &[("service", &settings.translation.service_name())],
                    ))
                    .strong()
                    .size(16.0),
                );
                ui.add_space(6.0);
                let (go, cancel) = self.consent_ui(ui, &settings.translation);
                if go {
                    self.allow(&mut settings.translation);
                    changed = true;
                }
                if cancel {
                    close = true;
                }
            });
            if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.consent_open = false;
                self.declined = true;
                self.resume = false;
                settings.translation.declined += 1;
                changed = true;
            }
        }
        changed
    }

    /// The text of the consent and its buttons: (start, cancel).
    fn consent_ui(&mut self, ui: &mut Ui, s: &TranslationSettings) -> (bool, bool) {
        let service = s.service_name();
        let host = match s.connection {
            Connection::ChatGpt => "OpenAI".to_owned(),
            Connection::Key => translate::host_of(&s.base()),
        };
        ui.label(trf(
            "To translate, these will be sent to {service} ({host}):",
            &[("service", &service), ("host", &host)],
        ));
        let (n, chars) = self.consent_for;
        ui.label(trf(
            "• comment texts — {n} in this game, about {k} thousand characters;",
            &[("n", &n), ("k", &thousands(chars))],
        ));
        ui.label(tr(
            "• player names, the event and the opening — for accurate terms.",
        ));
        ui.label(tr(
            "The game's moves are not sent; moves written inside comments are.",
        ));
        let privacy = match s.connection {
            Connection::ChatGpt => Some("https://openai.com/policies/privacy-policy/"),
            Connection::Key => s.preset.privacy(),
        };
        if let Some(url) = privacy {
            ui.hyperlink_to(
                trf(
                    "How {service} stores data: privacy policy ↗",
                    &[("service", &service)],
                ),
                url,
            );
        }
        if s.connection == Connection::ChatGpt {
            ui.label(tr("Translation uses part of your subscription's limit."));
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(tr("From now on, comments in other languages will be translated by themselves when you open a game. To turn it off or withdraw the permission: Settings › Translation.")).weak(),
        );
        ui.add_space(8.0);
        let mut go = false;
        let mut cancel = false;
        let row = egui::vec2(ui.available_width(), 32.0);
        ui.allocate_ui_with_layout(
            row,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let r = crate::widgets::primary(ui, tr("Start translating"));
                if r.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = true;
                }
                if self.setup_open && self.step == Step::Consent {
                    if ui.button(tr("Back")).clicked() {
                        self.step = Step::Choose;
                    }
                } else if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            },
        );
        (go, cancel)
    }

    fn setup_ui(&mut self, ui: &mut Ui, s: &mut TranslationSettings) -> bool {
        let mut changed = false;
        match self.step {
            Step::Consent => {
                let (go, _) = self.consent_ui(ui, s);
                if go {
                    self.allow(s);
                    changed = true;
                }
                return changed;
            }
            Step::Waiting => {
                self.waiting_ui(ui, true);
                return changed;
            }
            // A key is checked with the window as it is (the button waits).
            Step::Checking if s.connection == Connection::Key => {}
            Step::Checking => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(if s.connection == Connection::ChatGpt {
                        tr("Checking the subscription…")
                    } else {
                        tr("Checking…")
                    });
                });
                return changed;
            }
            Step::Choose => {}
        }
        ui.label(tr(
            "Comments are translated by a language model. Choose which:",
        ));
        ui.add_space(6.0);
        ui.radio_value(
            &mut self.choice,
            Connection::ChatGpt,
            tr("I have ChatGPT Plus or Pro"),
        );
        ui.indent("chatgpt_choice", |ui| {
            ui.label(RichText::new(tr("The easiest way: sign in, no key needed.")).weak());
            ui.label(RichText::new(tr("Uses part of your subscription's limit.")).weak());
            self.chatgpt_problem_ui(ui);
        });
        ui.add_space(4.0);
        ui.radio_value(
            &mut self.choice,
            Connection::Key,
            tr("No subscription — an API key"),
        );
        if self.choice == Connection::Key {
            ui.indent("key_choice", |ui| changed |= self.key_steps_ui(ui, s));
        } else {
            ui.indent("key_hint", |ui| {
                ui.label(
                    RichText::new(tr("A service with a key, or a model on this computer.")).weak(),
                );
            });
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(tr("From now on, comments in other languages will be translated by themselves when you open a game. To change: Settings › Translation.")).weak(),
        );
        ui.add_space(8.0);
        let row = egui::vec2(ui.available_width(), 32.0);
        ui.allocate_ui_with_layout(
            row,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let now = ui.input(|i| i.time);
                match self.choice {
                    Connection::ChatGpt => {
                        if crate::widgets::primary(ui, tr("Sign in with ChatGPT")).clicked() {
                            s.connection = Connection::ChatGpt;
                            changed = true;
                            self.resume = true;
                            if s.ready()
                                && translate::credentials().chatgpt.is_some_and(|t| t.plan())
                            {
                                // Signed in already: check and go on.
                                self.step = Step::Checking;
                                self.start_check(ui.ctx(), s, now);
                            } else {
                                self.sign_in(ui.ctx(), s);
                            }
                        }
                    }
                    Connection::Key => {
                        let preset = self.preset.unwrap_or(s.preset);
                        let missing = if preset.needs_key() {
                            self.key.trim().is_empty()
                        } else {
                            preset == Preset::Custom && s.base.trim().is_empty()
                        };
                        let checking = self.check.is_some();
                        let r = ui
                            .add_enabled_ui(!missing && !checking, |ui| {
                                crate::widgets::primary(ui, tr("Check and continue"))
                            })
                            .inner;
                        let r = if missing {
                            r.on_disabled_hover_text(tr("Paste the key first"))
                        } else {
                            r
                        };
                        if r.clicked() {
                            s.connection = Connection::Key;
                            s.preset = preset;
                            if preset.needs_key() {
                                let key = self.key.trim().to_owned();
                                translate::update_credentials(|c| {
                                    c.keys.insert(preset.id().to_owned(), key);
                                });
                            }
                            changed = true;
                            self.step = Step::Checking;
                            self.resume = true;
                            self.start_check(ui.ctx(), s, now);
                        }
                    }
                }
                if ui
                    .add(egui::Button::new(tr("Cancel")).min_size(egui::vec2(0.0, 30.0)))
                    .clicked()
                {
                    self.setup_open = false;
                    self.declined = true;
                    self.resume = false;
                    s.declined += 1;
                    changed = true;
                }
            },
        );
        changed
    }

    /// Why ChatGPT does not work, when known (a free plan, no permission, a sign-in note).
    fn chatgpt_problem_ui(&self, ui: &mut Ui) {
        let warn = ui.visuals().warn_fg_color;
        let text = match (&self.chatgpt, &self.note) {
            (_, Some(SignInNote::TimedOut)) => Some(tr("Sign-in was not finished in 10 minutes.").to_owned()),
            (_, Some(SignInNote::Declined)) => Some(tr("Sign-in was cancelled in the browser.").to_owned()),
            (_, Some(SignInNote::Failed(_))) => Some(tr("⚠ Sign-in could not be finished.").to_owned()),
            (_, Some(SignInNote::NotPrepared(_))) => Some(
                tr("⚠ Sign-in could not be prepared: ChessPenguin cannot receive the browser's answer.").to_owned(),
            ),
            (ChatGptState::NoPlan, _) => Some(tr("⚠ You signed in, but did not allow ChessPenguin to use the subscription's limit. Without it, translation with ChatGPT does not work.").to_owned()),
            (ChatGptState::NotEligible, _) => Some(tr("⚠ This ChatGPT account cannot translate in ChessPenguin: a Plus or Pro subscription is needed.").to_owned()),
            _ => None,
        };
        if let Some(t) = text {
            let r = ui.label(RichText::new(t).color(warn));
            if let Some(SignInNote::Failed(e) | SignInNote::NotPrepared(e)) = &self.note {
                r.on_hover_text(e);
            }
            if self.chatgpt == ChatGptState::NoPlan {
                ui.label(
                    RichText::new(tr("On the OpenAI site, leave the permission to use the subscription turned on"))
                        .weak()
                        .small(),
                );
            }
        }
    }

    /// The service list and how to get a key for it.
    fn key_steps_ui(&mut self, ui: &mut Ui, s: &mut TranslationSettings) -> bool {
        let mut changed = false;
        let mut preset = self.preset.unwrap_or(s.preset);
        ui.horizontal(|ui| {
            ui.label(tr("Service:"));
            egui::ComboBox::from_id_salt("translation_preset_setup")
                .selected_text(preset_label(preset))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for p in Preset::ALL {
                        ui.selectable_value(&mut preset, p, preset_label(p));
                    }
                });
        });
        if Some(preset) != self.preset {
            self.preset = Some(preset);
            self.key = translate::credentials()
                .key(preset.id())
                .unwrap_or("")
                .to_owned();
            self.check_result = None;
        }
        let site = |ui: &mut Ui, name: &str, url: &str, key: &'static str| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let text = trf(key, &[("service", &name)]);
                ui.hyperlink_to(text, url);
            });
        };
        match preset {
            Preset::DeepSeek => {
                site(
                    ui,
                    "DeepSeek",
                    preset.key_page().unwrap_or(""),
                    "1. Open the {service} website ↗ and sign in.",
                );
                ui.label(tr("2. Top up the balance: DeepSeek works with prepayment."));
                ui.label(tr(
                    "3. Press “Create new API key” and copy the key — it is shown only once.",
                ));
                ui.label(tr("4. Paste the key here:"));
            }
            Preset::OpenAi => {
                site(
                    ui,
                    "OpenAI",
                    preset.key_page().unwrap_or(""),
                    "1. Open the {service} website ↗ and sign in.",
                );
                ui.label(tr("2. Top up the balance in the Billing section."));
                ui.label(tr("3. Press “Create new secret key” and copy the key."));
                ui.label(tr("4. Paste the key here:"));
                ui.label(
                    RichText::new(tr(
                        "The API key is paid separately from the ChatGPT subscription.",
                    ))
                    .weak(),
                );
            }
            Preset::Gemini => {
                ui.hyperlink_to(
                    tr("1. Open Google AI Studio ↗ and sign in with your Google account."),
                    preset.key_page().unwrap_or(""),
                );
                ui.label(tr("2. Press “Create API key” and copy the key."));
                ui.label(tr("3. Paste the key here:"));
                ui.label(
                    RichText::new(tr("Free with a Google account, but Google may use the texts to train its models."))
                        .color(ui.visuals().warn_fg_color),
                );
            }
            Preset::Ollama => {
                ui.hyperlink_to(
                    tr("1. Install Ollama from ollama.com ↗ and start it."),
                    "https://ollama.com/download",
                );
                ui.horizontal(|ui| {
                    ui.label(tr("2. Download a model:"));
                    let cmd = format!("ollama pull {OLLAMA_MODEL}");
                    ui.label(RichText::new(&cmd).monospace());
                    if ui.small_button(tr("Copy")).clicked() {
                        ui.ctx().copy_text(cmd);
                    }
                });
                ui.label(tr("3. Press “Check and continue”."));
            }
            Preset::LmStudio => {
                ui.hyperlink_to(
                    tr("1. Install LM Studio from lmstudio.ai ↗."),
                    "https://lmstudio.ai",
                );
                ui.label(tr("2. Download a model in LM Studio."));
                ui.label(tr(
                    "3. Turn on the server: the Developer tab › Start Server.",
                ));
                ui.label(tr("4. Press “Check and continue”."));
            }
            Preset::Custom => {
                ui.horizontal(|ui| {
                    ui.label(tr("Address:"));
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut s.base)
                                .hint_text("https://…/v1")
                                .desired_width(260.0),
                        )
                        .on_hover_text(tr("The address of an OpenAI-compatible service, with /v1"))
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Model:"));
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut s.model).desired_width(200.0))
                        .changed();
                });
            }
        }
        if preset.needs_key() || preset == Preset::Custom {
            ui.horizontal(|ui| {
                ui.label(if preset == Preset::Custom {
                    tr("Key (if needed):")
                } else {
                    tr("Key:")
                });
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.key)
                        .password(!self.show_key)
                        .desired_width(240.0),
                );
                if r.changed() {
                    self.check_result = None;
                }
                ui.toggle_value(&mut self.show_key, "👁");
                if preset == Preset::Custom && r.lost_focus() {
                    let key = self.key.trim().to_owned();
                    translate::update_credentials(|c| {
                        c.keys.insert(Preset::Custom.id().to_owned(), key);
                    });
                }
            });
        }
        if self.check.is_some() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(tr("Checking…"));
            });
        }
        match &self.check_result {
            Some(Ok(t)) => {
                ui.label(trf(
                    "✓ Works: “{sample}” → “{result}”",
                    &[("sample", &translate::sample(s.target())), ("result", t)],
                ));
            }
            Some(Err(e)) if self.check.is_none() => {
                ui.label(RichText::new(failure_text(e, s)).color(ui.visuals().warn_fg_color));
            }
            _ => {}
        }
        changed
    }

    /// Waiting for the browser: what to do there, the link to copy, cancel.
    fn waiting_ui(&mut self, ui: &mut Ui, in_window: bool) {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new());
            ui.label(tr("Waiting for sign-in in the browser…"));
        });
        if in_window {
            ui.add_space(4.0);
            ui.label(tr("A page from OpenAI has opened in the browser. Sign in to your ChatGPT account and allow access for ChessPenguin. Then come back here — everything goes on by itself."));
            ui.add_space(4.0);
        }
        let now = ui.input(|i| i.time);
        ui.horizontal_wrapped(|ui| {
            if ui
                .link(tr("The browser did not open? Copy the link"))
                .clicked()
                && let Some(s) = &self.sign_in
            {
                ui.ctx().copy_text(s.url.clone());
                self.copied = Some(now);
            }
            if !in_window && ui.button(tr("Cancel")).clicked() {
                self.cancel_sign_in();
            }
        });
        if self.copied.is_some_and(|t| now - t < 6.0) {
            ui.label(
                RichText::new(tr("Copied. Paste the link into the browser's address bar.")).weak(),
            );
        }
        if in_window {
            ui.add_space(4.0);
            ui.label(RichText::new(tr("You can close this window: the sign-in goes on.")).weak());
            let row = egui::vec2(ui.available_width(), 32.0);
            ui.allocate_ui_with_layout(
                row,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        self.cancel_sign_in();
                    }
                },
            );
        }
    }

    /// Settings › Translation.
    pub fn settings_tab(&mut self, ui: &mut Ui, settings: &mut Settings) -> bool {
        let mut changed = false;
        let now = ui.input(|i| i.time);
        let s = &mut settings.translation;
        ui.label(
            RichText::new(tr("Comments in other languages are translated by a language model. Moves are not translated. The translation is not written to PGN files.")).weak(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(tr("Translate into:"));
            let current = s.target;
            let label = |t: Option<Lang>| match t {
                None => trf(
                    "As the interface ({lang})",
                    &[("lang", &crate::i18n::current().label())],
                ),
                Some(l) => l.label().to_owned(),
            };
            egui::ComboBox::from_id_salt("translation_target")
                .selected_text(label(current))
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut s.target, None, label(None))
                        .changed();
                    for l in Lang::ALL {
                        changed |= ui
                            .selectable_value(&mut s.target, Some(l), l.label())
                            .changed();
                    }
                });
        });
        ui.add_space(4.0);
        ui.label(tr("When comments are in another language:"));
        ui.horizontal(|ui| {
            let mode = if s.never_offer {
                2
            } else if s.auto {
                0
            } else {
                1
            };
            let mut m = mode;
            ui.radio_value(&mut m, 0, tr("Translate at once"));
            ui.radio_value(&mut m, 1, tr("Ask"));
            ui.radio_value(&mut m, 2, tr("Do nothing"));
            if m != mode {
                s.auto = m == 0;
                s.never_offer = m == 2;
                changed = true;
            }
        });
        ui.add_space(10.0);
        ui.label(RichText::new(tr("How to translate")).strong());
        changed |= ui
            .radio_value(
                &mut s.connection,
                Connection::ChatGpt,
                tr("ChatGPT sign-in"),
            )
            .changed();
        ui.indent("settings_chatgpt", |ui| {
            ui.label(
                RichText::new(tr(
                    "For a Plus or Pro subscription. Uses part of its limit.",
                ))
                .weak(),
            );
            changed |= self.chatgpt_block(ui, s, now);
        });
        changed |= ui
            .radio_value(&mut s.connection, Connection::Key, tr("API key"))
            .changed();
        ui.indent("settings_key", |ui| changed |= self.key_block(ui, s, now));
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Comments may be sent to:"));
            if s.allowed.iter().all(|a| a == "local") {
                ui.label(RichText::new(tr("Nowhere yet.")).weak());
            }
            let mut remove = None;
            for (i, a) in s.allowed.iter().enumerate().filter(|(_, a)| *a != "local") {
                let name = allowed_name(a);
                ui.label(&name);
                if crate::widgets::small_close_button(ui)
                    .on_hover_text(trf(
                        "Stop sending comments to {service}",
                        &[("service", &name)],
                    ))
                    .clicked()
                {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                s.allowed.remove(i);
                changed = true;
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let size = translate::cache_size();
            ui.label(trf(
                "Saved translations: {size}",
                &[("size", &human_size(size))],
            ));
            if self.clear_confirm {
                ui.label(tr("Delete all saved translations?"));
                if ui.button(tr("Cancel")).clicked() {
                    self.clear_confirm = false;
                }
                if ui.button(tr("Delete")).clicked() {
                    translate::clear_cache();
                    self.cache_epoch += 1;
                    self.clear_confirm = false;
                }
            } else if ui
                .add_enabled(size > 0, egui::Button::new(tr("Clear")))
                .clicked()
            {
                self.clear_confirm = true;
            }
        });
        ui.label(
            RichText::new(tr(
                "So that translated games open without the internet and without cost.",
            ))
            .weak(),
        );
        changed
    }

    fn chatgpt_block(&mut self, ui: &mut Ui, s: &mut TranslationSettings, now: f64) -> bool {
        let mut changed = false;
        let active = s.connection == Connection::ChatGpt;
        if self.sign_in.is_some() {
            self.waiting_ui(ui, false);
            return false;
        }
        if self.check.is_some() && active {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(tr("Checking the subscription…"));
            });
            return false;
        }
        let signed_in = translate::credentials().chatgpt.is_some();
        if !signed_in {
            if self.chatgpt == ChatGptState::SignedOut {
                ui.label(
                    RichText::new(tr("⚠ The sign-in is no longer valid."))
                        .color(ui.visuals().warn_fg_color),
                );
            }
            if ui.button(tr("Sign in with ChatGPT")).clicked() {
                s.connection = Connection::ChatGpt;
                changed = true;
                self.sign_in(ui.ctx(), s);
            }
            self.chatgpt_problem_ui(ui);
            return changed;
        }
        ui.horizontal_wrapped(|ui| {
            let who = if s.account.is_empty() {
                "ChatGPT".to_owned()
            } else {
                s.account.clone()
            };
            ui.label(trf("You are signed in: {account}", &[("account", &who)]));
            if ui.link(tr("Sign out")).clicked() {
                crate::chatgpt::sign_out();
                s.account.clear();
                s.allowed.retain(|a| a != "chatgpt");
                self.chatgpt = ChatGptState::Unknown;
                changed = true;
            }
        });
        ui.hyperlink_to(tr("Manage access on the OpenAI site ↗"), CHATGPT_SETTINGS);
        let warn = ui.visuals().warn_fg_color;
        match &self.chatgpt {
            ChatGptState::Works(m) => {
                ui.label(trf("✓ Works, model {model}", &[("model", m)]));
            }
            ChatGptState::NoPlan => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(tr(
                            "⚠ You did not allow ChessPenguin to use the subscription's limit.",
                        ))
                        .color(warn),
                    );
                    if ui.button(tr("Sign in again")).clicked() {
                        self.sign_in(ui.ctx(), s);
                    }
                });
            }
            ChatGptState::NotEligible => {
                ui.label(
                    RichText::new(tr("⚠ This account needs a Plus or Pro subscription."))
                        .color(warn),
                );
            }
            ChatGptState::Limit => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(tr(
                            "⚠ The subscription's limit for ChessPenguin is used up.",
                        ))
                        .color(warn),
                    );
                    ui.hyperlink_to(tr("ChatGPT settings ↗"), CHATGPT_SETTINGS);
                });
            }
            ChatGptState::SignedOut | ChatGptState::Unknown => {
                if active && ui.button(tr("Check")).clicked() {
                    self.start_check(ui.ctx(), s, now);
                }
            }
        }
        changed
    }

    fn key_block(&mut self, ui: &mut Ui, s: &mut TranslationSettings, now: f64) -> bool {
        let mut changed = false;
        let before = s.preset;
        ui.horizontal(|ui| {
            ui.label(tr("Service:"));
            egui::ComboBox::from_id_salt("translation_preset")
                .selected_text(preset_label(s.preset))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for p in Preset::ALL {
                        changed |= ui
                            .selectable_value(&mut s.preset, p, preset_label(p))
                            .changed();
                    }
                });
        });
        if s.preset != before {
            s.base.clear();
            s.model.clear();
            self.models.clear();
            self.check_result = None;
        }
        let checking = self.check.is_some() && s.connection == Connection::Key;
        if s.preset.needs_key() || s.preset == Preset::Custom {
            let mut key = translate::credentials()
                .key(s.preset.id())
                .unwrap_or("")
                .to_owned();
            ui.horizontal(|ui| {
                ui.label(if s.preset == Preset::Custom {
                    tr("Key (if needed):")
                } else {
                    tr("Key:")
                });
                let r = ui.add(
                    egui::TextEdit::singleline(&mut key)
                        .password(!self.show_key)
                        .desired_width(220.0),
                );
                if r.changed() {
                    let id = s.preset.id().to_owned();
                    let k = key.trim().to_owned();
                    translate::update_credentials(|c| {
                        c.keys.insert(id, k);
                    });
                    self.check_result = None;
                }
                ui.toggle_value(&mut self.show_key, "👁");
                if ui
                    .add_enabled(
                        !checking && !key.trim().is_empty(),
                        egui::Button::new(tr("Check")),
                    )
                    .clicked()
                {
                    s.connection = Connection::Key;
                    changed = true;
                    self.start_check(ui.ctx(), s, now);
                }
            });
            if let Some(url) = s.preset.key_page() {
                ui.hyperlink_to(
                    trf(
                        "Get a key on the {service} website ↗",
                        &[("service", &s.preset.name())],
                    ),
                    url,
                );
            }
        } else {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!checking, egui::Button::new(tr("Check")))
                    .clicked()
                {
                    s.connection = Connection::Key;
                    changed = true;
                    self.start_check(ui.ctx(), s, now);
                }
            });
        }
        if s.preset == Preset::Gemini {
            ui.label(
                RichText::new(tr(
                    "Free with a Google account, but Google may use the texts to train its models.",
                ))
                .color(ui.visuals().warn_fg_color),
            );
        }
        let custom = s.preset == Preset::Custom;
        let mut fields = |ui: &mut Ui, s: &mut TranslationSettings, models: &[String]| {
            ui.horizontal(|ui| {
                ui.label(tr("Address:"));
                let mut base = if s.base.is_empty() {
                    s.preset.base().to_owned()
                } else {
                    s.base.clone()
                };
                if ui
                    .add(egui::TextEdit::singleline(&mut base).desired_width(260.0))
                    .on_hover_text(tr("The address of an OpenAI-compatible service, with /v1"))
                    .changed()
                {
                    s.base = base;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("Model:"));
                if models.is_empty() {
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut s.model)
                                .hint_text(s.preset.model())
                                .desired_width(200.0),
                        )
                        .changed();
                } else {
                    let current = s.model();
                    egui::ComboBox::from_id_salt("translation_model")
                        .selected_text(&current)
                        .show_ui(ui, |ui| {
                            for m in models {
                                if ui.selectable_label(*m == current, m).clicked() {
                                    s.model = m.clone();
                                    changed = true;
                                }
                            }
                        });
                }
            });
        };
        let models = self.models.clone();
        if custom || !s.preset.needs_key() {
            fields(ui, s, &models);
        } else {
            egui::CollapsingHeader::new(tr("Advanced"))
                .id_salt("translation_advanced")
                .show(ui, |ui| fields(ui, s, &models));
        }
        if checking {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(tr("Checking…"));
            });
        } else if let Some(r) = &self.check_result
            && s.connection == Connection::Key
        {
            match r {
                Ok(t) => {
                    ui.label(trf(
                        "✓ Works: “{sample}” → “{result}”",
                        &[("sample", &translate::sample(s.target())), ("result", t)],
                    ));
                }
                Err(e) => {
                    ui.label(RichText::new(failure_text(e, s)).color(ui.visuals().warn_fg_color));
                }
            }
        }
        changed
    }
}

fn preset_label(p: Preset) -> String {
    match p {
        Preset::Ollama => tr("Ollama — on this computer").to_owned(),
        Preset::LmStudio => tr("LM Studio — on this computer").to_owned(),
        _ => p.name().to_owned(),
    }
}

fn allowed_name(id: &str) -> String {
    if id == "chatgpt" {
        return "ChatGPT".into();
    }
    if let Some(base) = id.strip_prefix("custom:") {
        return translate::host_of(base);
    }
    Preset::ALL
        .into_iter()
        .find(|p| p.id() == id)
        .map_or_else(|| id.to_owned(), |p| p.name().to_owned())
}

fn human_size(bytes: u64) -> String {
    if bytes == 0 {
        tr("none").to_owned()
    } else if bytes >= 1 << 20 {
        let mb = format!("{:.1}", bytes as f64 / (1u64 << 20) as f64);
        // A decimal comma where the language writes one.
        let mb = if crate::i18n::current() == Lang::En {
            mb
        } else {
            mb.replace('.', ",")
        };
        trf("{n} MB", &[("n", &mb)])
    } else {
        trf("{n} KB", &[("n", &bytes.div_ceil(1024))])
    }
}

/// The plain words for a failure.
pub fn failure_text(e: &Failure, s: &TranslationSettings) -> String {
    let service = s.service_name();
    match e {
        Failure::Key => tr("⚠ The key was not accepted. Perhaps it was copied incompletely or deleted on the site.").to_owned(),
        Failure::Money => trf("⚠ The {service} account has run out of money. Top up the balance on the site.", &[("service", &service)]),
        Failure::RateLimited => tr("⚠ The service is overloaded. What is translated is saved.").to_owned(),
        Failure::PlanLimit => tr("⚠ The ChatGPT subscription's limit for ChessPenguin is used up. When it comes back is shown in the ChatGPT settings.").to_owned(),
        Failure::NotEligible(_) => tr("⚠ This ChatGPT account cannot translate in ChessPenguin: a Plus or Pro subscription is needed.").to_owned(),
        Failure::SignedOut => tr("⚠ The ChatGPT sign-in is no longer valid. Sign in again.").to_owned(),
        Failure::Offline(_) => trf("No connection to {service}. We will translate when the connection is back.", &[("service", &service)]),
        Failure::LocalDown => trf("⚠ {service} does not answer. Start {service} and press “Retry”.", &[("service", &service)]),
        Failure::NoModel(m) if s.preset == Preset::Ollama && s.connection == Connection::Key => {
            if m.is_empty() {
                tr("There are no models in Ollama. Download one with the command below.").to_owned()
            } else {
                trf("⚠ Ollama has no model {model}. Download it with the command: ollama pull {model}", &[("model", m)])
            }
        }
        Failure::NoModel(m) => trf(
            "⚠ {service} has no model {model}. Choose another one in the translation settings.",
            &[("service", &service), ("model", m)],
        ),
        Failure::Service(code, _) => trf("⚠ The service answered with an error ({code}).", &[("code", code)]),
    }
}

// --- One game ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum State {
    /// Nothing to translate (or not looked yet).
    #[default]
    None,
    Offer,
    /// Much foreign text: waits for "Translate" even in automatic mode.
    Big,
    Running,
    Done,
    Stopped,
    Failed,
}

/// What the strip asks the game view to do.
pub enum StripAction {
    OpenSettings,
    Toast(String),
}

#[derive(Default)]
pub struct GameTranslation {
    revision: Option<(u64, Lang)>,
    plan: Option<Plan>,
    context: GameContext,
    /// Comment text → what the notation shows.
    pub marks: HashMap<String, Mark>,
    bad: HashSet<String>,
    job: Option<Job>,
    state: State,
    failure: Option<Failure>,
    /// The original text is shown.
    pub original: bool,
    dismissed: bool,
    details: bool,
    /// Characters sent by this game's jobs.
    sent: usize,
    /// Everything shown came from the cache.
    from_cache: bool,
    job_started: f64,
    service: String,
    /// The automatic start was tried for this plan.
    tried: bool,
    /// The service the running job sends to (it stops when that changes).
    job_service: String,
    /// "Translate comments" was chosen: the cache is shown even with "Do nothing".
    forced: bool,
    /// The game was put aside while translating: it goes on when shown again.
    suspended: bool,
    cache_epoch: u32,
}

impl GameTranslation {
    /// The marks to show (none in the original mode).
    pub fn shown(&self) -> Option<&HashMap<String, Mark>> {
        (!self.original && !self.marks.is_empty()).then_some(&self.marks)
    }

    pub fn has_translation(&self) -> bool {
        self.marks
            .values()
            .any(|m| matches!(m, Mark::Translated(_)))
    }

    /// The game has comments still to translate and nothing is going on.
    pub fn can_translate(&self) -> bool {
        self.job.is_none()
            && self.plan.as_ref().is_some_and(|p| {
                p.texts
                    .iter()
                    .any(|t| !matches!(self.marks.get(t), Some(Mark::Translated(_))))
            })
    }

    pub fn toggle(&mut self) {
        if self.has_translation() {
            self.original = !self.original;
        }
    }

    /// Stops the work of a game that is not shown any more (it goes on when shown again).
    pub fn suspend(&mut self) {
        if self.job.take().is_some() && self.state == State::Running {
            self.marks.retain(|_, m| *m != Mark::Pending);
            self.state = State::Stopped;
            self.suspended = true;
        }
    }

    fn translated(&self) -> usize {
        self.marks
            .values()
            .filter(|m| matches!(m, Mark::Translated(_)))
            .count()
    }

    fn total(&self) -> usize {
        self.plan.as_ref().map_or(0, |p| p.texts.len())
    }

    fn foreign_chars(&self) -> usize {
        self.plan
            .as_ref()
            .map_or(0, |p| p.texts.iter().map(|t| t.len()).sum())
    }

    /// Takes what the cache has for the plan; true when that is everything.
    fn take_cached(&mut self, target: Lang) -> bool {
        let Some(plan) = &self.plan else { return false };
        let mut all = true;
        for t in &plan.texts {
            if matches!(self.marks.get(t), Some(Mark::Translated(_))) {
                continue;
            }
            match translate::cached(target, t) {
                Some(tr) => {
                    self.marks.insert(t.clone(), Mark::Translated(tr));
                }
                None => all = false,
            }
        }
        all
    }

    /// Looks at the game again when it, the target language or the cache changed.
    fn refresh(&mut self, game: &chess_core::Game, s: &TranslationSettings, epoch: u32) {
        let target = s.target();
        let key = (game.revision(), target);
        if self.revision == Some(key) && self.cache_epoch == epoch {
            return;
        }
        let reset = self.revision.is_some_and(|(_, l)| l != target)
            || self.plan.is_none()
            || self.cache_epoch != epoch;
        self.revision = Some(key);
        self.cache_epoch = epoch;
        if reset {
            self.job = None;
            self.marks.clear();
            self.bad.clear();
            self.state = State::None;
            self.failure = None;
            self.tried = false;
        }
        let texts = translate::comments(game);
        let before: HashSet<String> = self
            .plan
            .as_ref()
            .map(|p| p.texts.iter().cloned().collect())
            .unwrap_or_default();
        self.plan = translate::plan(&texts, target);
        self.context = GameContext::of(game);
        let Some(plan) = &self.plan else {
            self.state = State::None;
            return;
        };
        let wanted: HashSet<&String> = plan.texts.iter().collect();
        self.marks.retain(|t, _| wanted.contains(t));
        // "Do nothing": not even the cache, until "Translate comments".
        let all = if s.never_offer && !self.forced {
            false
        } else {
            self.take_cached(target)
        };
        if all {
            if self.job.is_none() {
                self.state = State::Done;
                self.from_cache = self.sent == 0;
            }
        } else if matches!(self.state, State::None) {
            self.state = State::Offer;
        } else if self.state == State::Done
            && self
                .plan
                .as_ref()
                .is_some_and(|p| p.texts.iter().any(|t| !before.contains(t)))
        {
            // New comments to translate (the game changed); failed ones are not sent again.
            self.state = State::Stopped;
            self.tried = false;
        }
    }

    /// Sends what is not translated yet (or `only` these). Never without the consent.
    fn start(&mut self, ctx: &egui::Context, s: &TranslationSettings, only: Option<Vec<String>>) {
        if !s.ready() || !s.allowed() {
            return;
        }
        let target = s.target();
        self.take_cached(target);
        let Some(plan) = &self.plan else { return };
        let Some(backend) = Backend::from(s) else {
            return;
        };
        let texts: Vec<String> = match only {
            Some(t) => t,
            None => plan
                .texts
                .iter()
                .filter(|t| !matches!(self.marks.get(*t), Some(Mark::Translated(_))))
                .cloned()
                .collect(),
        };
        if texts.is_empty() {
            self.state = State::Done;
            return;
        }
        for t in &texts {
            self.marks.insert(t.clone(), Mark::Pending);
            self.bad.remove(t);
        }
        self.sent += texts.iter().map(|t| t.len()).sum::<usize>();
        self.from_cache = false;
        self.failure = None;
        self.details = false;
        self.service = s.service_name();
        self.job_service = s.service_id();
        self.suspended = false;
        self.job_started = ctx.input(|i| i.time);
        self.job = Some(Job::start(
            ctx,
            backend,
            target,
            format!(
                "{} {}",
                s.service_id(),
                match s.connection {
                    Connection::ChatGpt => s.chatgpt_model.clone(),
                    Connection::Key => s.model(),
                }
            ),
            self.context.clone(),
            texts,
        ));
        self.state = State::Running;
    }

    fn poll(&mut self, hub: &mut Hub) {
        let Some(job) = &self.job else { return };
        let mut finished = false;
        for e in job.poll() {
            match e {
                Event::Done(pairs) => {
                    for (o, t) in pairs {
                        self.marks.insert(o, Mark::Translated(t));
                    }
                }
                Event::Bad(list) => {
                    for o in list {
                        self.marks.insert(o.clone(), Mark::Bad);
                        self.bad.insert(o);
                    }
                }
                Event::Failed(f) => {
                    match f {
                        Failure::PlanLimit => hub.chatgpt = ChatGptState::Limit,
                        Failure::SignedOut => hub.chatgpt = ChatGptState::SignedOut,
                        Failure::NotEligible(_) => hub.chatgpt = ChatGptState::NotEligible,
                        _ => {}
                    }
                    self.failure = Some(f);
                }
                Event::Finished => finished = true,
            }
        }
        if finished {
            self.job = None;
            // What is still pending was not reached (stopped or failed).
            self.marks.retain(|_, m| *m != Mark::Pending);
            self.state = if self.failure.is_some() {
                State::Failed
            } else {
                State::Done
            };
        }
    }

    /// Per frame: looks at the game, collects results, starts automatically.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        game: &chess_core::Game,
        settings: &Settings,
        hub: &mut Hub,
    ) {
        let s = &settings.translation;
        self.refresh(game, s, hub.cache_epoch);
        self.poll(hub);
        if hub.take_declined() {
            self.dismissed = true;
        }
        // Permission withdrawn, signed out or another service chosen: stop sending.
        if self.job.is_some() && (!s.allowed() || !s.ready() || s.service_id() != self.job_service)
        {
            self.job = None;
            self.marks.retain(|_, m| *m != Mark::Pending);
            self.state = State::Stopped;
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        // Taken every frame, so a request never waits for another game.
        let start = hub.take_start();
        let retry = hub.take_retry_failed();
        if self.plan.is_none() {
            return;
        }
        if start
            && matches!(
                self.state,
                State::Offer | State::Big | State::Stopped | State::Failed
            )
            || retry && self.state == State::Failed
        {
            self.dismissed = false;
            self.start(ctx, s, None);
            return;
        }
        let automatic = s.auto && !s.never_offer && s.ready() && s.allowed();
        if self.suspended && automatic {
            self.start(ctx, s, None);
        } else if !self.tried
            && matches!(self.state, State::Offer | State::Stopped)
            && s.auto
            && !s.never_offer
        {
            self.tried = true;
            if self.foreign_chars() > BIG {
                self.state = State::Big;
            } else if automatic {
                self.start(ctx, s, None);
            }
        }
    }

    /// The "Translate" button (strip, menu, Ctrl+Shift+T): set up and ask first if needed.
    pub fn translate(&mut self, ctx: &egui::Context, settings: &mut Settings, hub: &mut Hub) {
        self.translate_only(ctx, settings, hub, None);
    }

    fn translate_only(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        hub: &mut Hub,
        only: Option<Vec<String>>,
    ) {
        self.forced = true;
        self.dismissed = false;
        let n = self.total();
        let chars = self.foreign_chars();
        if hub.request(&mut settings.translation, n, chars) {
            self.start(ctx, &settings.translation, only);
        } else {
            hub.resume = true;
        }
    }

    /// The strip above the notation.
    pub fn strip(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        hub: &mut Hub,
    ) -> Option<StripAction> {
        if self.plan.is_none() && self.marks.is_empty() {
            return None;
        }
        let s = &settings.translation;
        let hidden = match self.state {
            State::None => true,
            State::Offer | State::Big => self.dismissed || (s.never_offer && !self.forced),
            _ => false,
        };
        let ready = s.ready() && s.allowed();
        let service_name = s.service_name();
        let local = s.is_local();
        let waiting_sign_in = hub.busy_in_strip() || (hub.signing_in() && hub.sign_in_from_strip);
        if hidden && !waiting_sign_in {
            return None;
        }
        let mut action = None;
        let warn = ui.visuals().warn_fg_color;
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // The text and its buttons wrap on the left; the toggle keeps its place on
                // the right, or goes to a line of its own in a narrow panel.
                let toggle = self.has_translation() && !waiting_sign_in;
                let narrow = ui.available_width() < 360.0;
                let toggle_w = if toggle && !narrow { 170.0 } else { 0.0 };
                let row_h = ui.spacing().interact_size.y;
                let spinner = egui::Spinner::new().size(ui.text_style_height(&egui::TextStyle::Body));
                ui.horizontal_top(|ui| {
                let left = (ui.available_width() - toggle_w).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(left, 0.0),
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                    ui.set_min_height(row_h);
                    // Buttons and links stay whole; only the texts wrap.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    let text = |ui: &mut Ui, t: RichText| ui.add(egui::Label::new(t).wrap());
                    if waiting_sign_in {
                        ui.add(spinner);
                        if hub.signing_in() {
                            text(ui, tr("Waiting for sign-in in the browser…").into());
                            if ui.button(tr("Cancel")).clicked() {
                                hub.cancel_sign_in();
                            }
                        } else {
                            text(ui, tr("Checking…").into());
                        }
                        return;
                    }
                    let lang = self.plan.as_ref().map_or("", |p| lang_name(p.source));
                    let (n, total) = (self.translated(), self.total());
                    match self.state {
                        State::Offer | State::Big => {
                            if self.has_translation() {
                                text(ui, trf("Translated: {n} of {total}", &[("n", &n), ("total", &total)]).into());
                            } else if self.state == State::Big {
                                text(ui, trf(
                                    "Many comments in another language ({lang}), about {k} thousand characters",
                                    &[("lang", &lang), ("k", &thousands(self.foreign_chars()))],
                                ).into());
                            } else {
                                text(ui, trf("Comments in another language ({lang})", &[("lang", &lang)]).into());
                            }
                            let hint = if ready {
                                trf("Translate with {service}", &[("service", &service_name)])
                            } else {
                                tr("Choose how to translate").to_owned()
                            };
                            if ui.button(tr("Translate")).on_hover_text(hint).clicked() {
                                self.translate(ui.ctx(), settings, hub);
                            }
                            if settings.translation.declined >= 2
                                && ui.link(tr("Don't offer")).on_hover_text(tr("Never offer to translate comments. You can turn it back on in Settings › Translation.")).clicked()
                            {
                                settings.translation.never_offer = true;
                                self.dismissed = true;
                                action = Some(StripAction::Toast(tr("Translation will not be offered again. To turn it back on: Settings › Translation.").to_owned()));
                            }
                            if crate::widgets::small_close_button(ui).on_hover_text(tr("Hide for this game")).clicked() {
                                self.dismissed = true;
                                settings.translation.declined += 1;
                            }
                        }
                        State::Running => {
                            ui.add(spinner);
                            let waiting = translate::WAITING.load(Ordering::Relaxed);
                            let now = ui.input(|i| i.time);
                            if waiting > 0 {
                                text(ui, trf("The service asks to wait… {s} s", &[("s", &waiting)]).into());
                            } else if n == 0 && local && now - self.job_started > 8.0 {
                                text(ui, tr("The model is loading into memory — this can take up to a minute…").into());
                            } else {
                                text(ui, trf("Translating… {n} of {total}", &[("n", &n), ("total", &total)]).into());
                            }
                            if ui.button(tr("Stop")).clicked() {
                                self.job = None;
                                self.marks.retain(|_, m| *m != Mark::Pending);
                                self.state = State::Stopped;
                            }
                        }
                        State::Done => {
                            if self.bad.is_empty() {
                                let tip = if self.from_cache {
                                    tr("Saved translation shown — nothing was sent. The translation is not written to the PGN file.").to_owned()
                                } else {
                                    trf(
                                        "Translated: {n} of {total}. Sent now: about {k} thousand characters. The translation is not written to the PGN file.",
                                        &[("n", &n), ("total", &total), ("k", &thousands(self.sent))],
                                    )
                                };
                                let service = if self.service.is_empty() { service_name.clone() } else { self.service.clone() };
                                text(ui, trf("Machine translation ({service})", &[("service", &service)]).into()).on_hover_text(tip);
                            } else {
                                text(ui, RichText::new(format!("⚠ {}", trf(
                                        "Translated: {n} of {total}, failed: {bad}",
                                        &[("n", &n), ("total", &total), ("bad", &self.bad.len())],
                                    )))
                                    .color(warn));
                                if ui.button(tr("Retry")).clicked() {
                                    let only: Vec<String> = self.bad.iter().cloned().collect();
                                    self.translate_only(ui.ctx(), settings, hub, Some(only));
                                }
                            }
                        }
                        State::Stopped => {
                            text(ui, trf("Translated: {n} of {total}", &[("n", &n), ("total", &total)]).into());
                            if ui.button(tr("Continue")).clicked() {
                                self.translate(ui.ctx(), settings, hub);
                            }
                        }
                        State::Failed => {
                            let f = self.failure.clone().unwrap_or(Failure::Service(0, String::new()));
                            let calm = matches!(f, Failure::Offline(_));
                            let shown = failure_text(&f, &settings.translation);
                            if calm {
                                text(ui, RichText::new(shown));
                            } else {
                                text(ui, RichText::new(shown).color(warn));
                            }
                            match &f {
                                Failure::Key | Failure::NoModel(_) if !(settings.translation.preset == Preset::Ollama) => {
                                    if ui.button(tr("Translation settings")).clicked() {
                                        action = Some(StripAction::OpenSettings);
                                    }
                                }
                                Failure::NoModel(m) => {
                                    if ui.button(tr("Copy the command")).clicked() {
                                        let m = if m.is_empty() { OLLAMA_MODEL } else { m };
                                        ui.ctx().copy_text(format!("ollama pull {m}"));
                                    }
                                }
                                Failure::Money => {
                                    if let Some(url) = settings.translation.preset.key_page() {
                                        ui.hyperlink_to(tr("Open the site ↗"), url);
                                    }
                                }
                                Failure::PlanLimit => {
                                    ui.hyperlink_to(tr("ChatGPT settings ↗"), CHATGPT_SETTINGS);
                                    if ui.button(tr("Another way")).clicked() {
                                        hub.choice = Connection::Key;
                                        hub.open_setup(&settings.translation);
                                        hub.choice = Connection::Key;
                                    }
                                }
                                Failure::NotEligible(_) => {
                                    if ui.button(tr("Use an API key")).clicked() {
                                        hub.open_setup(&settings.translation);
                                        hub.choice = Connection::Key;
                                    }
                                }
                                Failure::SignedOut if ui.button(tr("Sign in again")).clicked() => {
                                    hub.sign_in_from_strip = true;
                                    hub.resume = true;
                                    hub.sign_in(ui.ctx(), &mut settings.translation);
                                }
                                _ => {}
                            }
                            if ui.button(tr("Retry")).clicked() {
                                self.translate(ui.ctx(), settings, hub);
                            }
                            if !calm && ui.link(tr("Details")).clicked() {
                                self.details = !self.details;
                            }
                            if crate::widgets::small_close_button(ui).on_hover_text(tr("Hide for this game")).clicked() {
                                self.state = if self.has_translation() { State::Done } else { State::Offer };
                                self.dismissed = true;
                            }
                        }
                        State::None => {}
                    }
                    },
                );
                if toggle && !narrow {
                    self.toggle_ui(ui);
                }
                });
                if toggle && narrow {
                    self.toggle_ui(ui);
                }
                if self.details
                    && let Some(f) = &self.failure
                {
                    let text = f.to_string();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&text).monospace().small());
                        if ui.small_button(tr("Copy")).clicked() {
                            ui.ctx().copy_text(text.clone());
                        }
                    });
                }
                if self.state == State::Running && self.total() > 40 {
                    let frac = self.translated() as f32 / self.total().max(1) as f32;
                    ui.add(egui::ProgressBar::new(frac).desired_height(3.0));
                }
            });
        action
    }

    fn toggle_ui(&mut self, ui: &mut Ui) {
        if !self.has_translation() {
            return;
        }
        let row = egui::vec2(ui.available_width(), 24.0);
        ui.allocate_ui_with_layout(
            row,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // Right to left: "Translation" first.
                if ui
                    .selectable_label(!self.original, tr("Translation"))
                    .clicked()
                {
                    self.original = false;
                }
                if ui.selectable_label(self.original, tr("Original")).clicked() {
                    self.original = true;
                }
            },
        );
    }
}
