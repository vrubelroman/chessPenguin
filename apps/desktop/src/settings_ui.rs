//! Settings window: engines, board & pieces, sound, interface, about.

use crate::i18n::{Lang, tr, trf};
use egui::{RichText, Sense, Ui, vec2};
use shakmaty::{Color, Piece, Role};

use crate::assets::{BoardThemeId, PieceSet, SoundSet};
use crate::settings::Settings;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Engines,
    Board,
    Sound,
    Interface,
    Translation,
    About,
}

pub enum SettingsAction {
    AddEngine,
    /// Download the official Stockfish (decision 14).
    DownloadEngine,
    CancelDownload,
    /// Delete the downloaded Stockfish from the disk (it was removed from the list).
    RemoveDownloaded,
    TestSound,
    /// The active engine (or its options) changed; a running analysis should restart.
    EngineChanged,
    /// Ask now whether a newer version is out.
    CheckUpdates,
    /// Open the update window.
    OpenUpdate,
}

pub struct SettingsWindow {
    pub open: bool,
    pub tab: Tab,
    pub checking_engine: bool,
    /// The running (or last) Stockfish download, set by the application every frame.
    pub download: Option<crate::engine_download::Stage>,
    /// Engine whose removal (with its downloaded files) waits for confirmation.
    confirm_remove: Option<usize>,
    /// What the update check found, set by the application every frame, and the newer
    /// version if there is one.
    pub update_status: Option<String>,
    pub update_available: Option<String>,
}

impl SettingsWindow {
    pub fn new() -> Self {
        Self {
            open: false,
            tab: Tab::default(),
            checking_engine: false,
            download: None,
            confirm_remove: None,
            update_status: None,
            update_available: None,
        }
    }

    /// Returns actions and whether settings changed (to be saved).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        s: &mut Settings,
        translation: &mut crate::translate_ui::Hub,
    ) -> (Vec<SettingsAction>, bool) {
        let mut actions = Vec::new();
        let mut changed = false;
        let mut open = self.open;
        egui::Window::new(tr("Settings"))
            .open(&mut open)
            .default_size(vec2(560.0, 680.0))
            // Open at the right edge so the board stays visible for live preview.
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(ctx.content_rect().right_top() + vec2(-16.0, 60.0))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Engines, tr("Engines"));
                    ui.selectable_value(&mut self.tab, Tab::Board, tr("Board & pieces"));
                    ui.selectable_value(&mut self.tab, Tab::Sound, tr("Sound"));
                    ui.selectable_value(&mut self.tab, Tab::Interface, tr("Interface"));
                    ui.selectable_value(&mut self.tab, Tab::Translation, tr("Translation"));
                    ui.selectable_value(&mut self.tab, Tab::About, tr("About"));
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Engines => self.engines(ui, s, &mut actions, &mut changed),
                    Tab::Board => board_tab(ui, s, &mut changed),
                    Tab::Translation => changed |= translation.settings_tab(ui, s),
                    Tab::Sound => sound_tab(ui, s, &mut actions, &mut changed),
                    Tab::About => {
                        let (status, available) =
                            (self.update_status.clone(), self.update_available.clone());
                        about_tab(ui, s.language, |ui| {
                            // Updates, right under the version.
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                match (&available, &status) {
                                    (Some(v), _) => {
                                        let label = RichText::new(trf(
                                            "⬆ Update to {v}…",
                                            &[("v", v)],
                                        ))
                                        .color(ui.visuals().hyperlink_color);
                                        if ui.button(label).clicked() {
                                            actions.push(SettingsAction::OpenUpdate);
                                        }
                                    }
                                    (None, Some(st)) => {
                                        ui.label(RichText::new(st).weak());
                                    }
                                    (None, None) => {}
                                }
                                if ui.button(tr("Check now")).clicked() {
                                    actions.push(SettingsAction::CheckUpdates);
                                }
                            });
                            changed |= ui
                                .checkbox(&mut s.check_updates, tr("Check for updates at start"))
                                .on_hover_text(tr(
                                    "ChessPenguin asks its site which version is the latest; the site counts installations that run: only a random number of this installation, the version and the system are sent",
                                ))
                                .changed();
                        });
                    }
                    Tab::Interface => {
                        ui.horizontal(|ui| {
                            ui.label(tr("Language:"));
                            egui::ComboBox::from_id_salt("language")
                                .selected_text(s.language.label())
                                .show_ui(ui, |ui| {
                                    for lang in Lang::ALL {
                                        if ui
                                            .selectable_value(&mut s.language, lang, lang.label())
                                            .changed()
                                        {
                                            crate::i18n::set_lang(lang);
                                            changed = true;
                                        }
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label(tr("Theme:"));
                            changed |= ui
                                .selectable_value(&mut s.dark_mode, true, tr("Dark"))
                                .changed();
                            changed |= ui
                                .selectable_value(&mut s.dark_mode, false, tr("Light"))
                                .changed();
                        });
                    }
                });
            });
        self.open = open;
        (actions, changed)
    }

    fn engines(
        &mut self,
        ui: &mut Ui,
        s: &mut Settings,
        actions: &mut Vec<SettingsAction>,
        changed: &mut bool,
    ) {
        ui.label(
            RichText::new(tr("UCI engines. The active one is used for game analysis. Time, lines, threads and memory are set with ⚙ in the analysis panel.")).weak(),
        );
        ui.add_space(6.0);
        let downloading = self.download.as_ref().is_some_and(|d| {
            matches!(
                d,
                crate::engine_download::Stage::Downloading { .. }
                    | crate::engine_download::Stage::Unpacking
            )
        });
        // No engine yet: the two ways to get one, plainly.
        if s.engines.is_empty() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(tr("No engine yet")).strong());
                ui.label(
                    RichText::new(tr("An engine evaluates positions and suggests moves. Stockfish is the strongest free one."))
                        .weak(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if downloading {
                        self.download_progress(ui, actions);
                    } else if crate::engine_download::available()
                        && ui
                            .button(RichText::new(tr("⬇ Download Stockfish 19")).strong())
                            .on_hover_text(trf(
                                "{mb} MB from the official Stockfish page; installed for your account only",
                                &[("mb", &crate::engine_download::size_mb())],
                            ))
                            .clicked()
                    {
                        actions.push(SettingsAction::DownloadEngine);
                    }
                    if ui.button(tr("Choose engine file…")).clicked() {
                        actions.push(SettingsAction::AddEngine);
                    }
                });
            });
        }
        let mut remove = None;
        for i in 0..s.engines.len() {
            let active = s.active_engine == Some(i);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if ui
                        .radio(active, "")
                        .on_hover_text(tr("Make active"))
                        .clicked()
                        && !active
                    {
                        s.active_engine = Some(i);
                        *changed = true;
                        actions.push(SettingsAction::EngineChanged);
                    }
                    let e = &mut s.engines[i];
                    *changed |= ui
                        .add(egui::TextEdit::singleline(&mut e.name).desired_width(220.0))
                        .changed();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let downloaded =
                            s.engines[i].builtin && crate::engines::bundled_stockfish().is_none();
                        let hint = if downloaded {
                            tr("Remove (and delete the downloaded files)")
                        } else {
                            tr("Remove from list")
                        };
                        if ui.button("🗑").on_hover_text(hint).clicked() {
                            if downloaded && crate::engine_download::installed().is_some() {
                                self.confirm_remove = Some(i);
                            } else {
                                remove = Some(i);
                            }
                        }
                    });
                });
                let e = &s.engines[i];
                if e.builtin {
                    if let Some(p) = crate::engines::bundled_stockfish() {
                        ui.label(
                            RichText::new(tr("Included with ChessPenguin"))
                                .small()
                                .weak(),
                        )
                        .on_hover_text(p.display().to_string());
                    } else if let Some(p) = crate::engine_download::installed() {
                        let mb = std::fs::metadata(&p).map_or(0, |m| m.len() / 1_000_000);
                        ui.label(
                            RichText::new(trf(
                                "Downloaded by ChessPenguin · {mb} MB",
                                &[("mb", &mb)],
                            ))
                            .small()
                            .weak(),
                        )
                        .on_hover_text(p.display().to_string());
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(tr("⚠ File not found"))
                                    .small()
                                    .color(ui.visuals().warn_fg_color),
                            );
                            if downloading {
                                self.download_progress(ui, actions);
                            } else if crate::engine_download::available()
                                && ui.small_button(tr("Download again")).clicked()
                            {
                                actions.push(SettingsAction::DownloadEngine);
                            }
                        });
                    }
                } else {
                    ui.label(RichText::new(e.path.display().to_string()).small().weak());
                }
            });
        }
        if let Some(i) = self.confirm_remove {
            let mut answer = None;
            let modal = egui::Modal::new(egui::Id::new("remove_engine")).show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                let mb = crate::engine_download::installed()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map_or(0, |m| m.len() / 1_000_000);
                ui.label(trf(
                    "Remove Stockfish 19? This frees {mb} MB. You can download it again at any time.",
                    &[("mb", &mb)],
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let cancel = ui.button(tr("Cancel"));
                        cancel.request_focus();
                        if cancel.clicked() {
                            answer = Some(false);
                        }
                        if ui.button(tr("Remove")).clicked() {
                            answer = Some(true);
                        }
                    });
                });
            });
            if modal.should_close() {
                answer = Some(false);
            }
            match answer {
                Some(true) => {
                    remove = Some(i);
                    actions.push(SettingsAction::RemoveDownloaded);
                    self.confirm_remove = None;
                }
                Some(false) => self.confirm_remove = None,
                None => {}
            }
        }
        if let Some(i) = remove {
            s.engines.remove(i);
            s.active_engine = match s.active_engine {
                Some(a) if a == i => (!s.engines.is_empty()).then_some(0),
                Some(a) if a > i => Some(a - 1),
                a => a,
            };
            *changed = true;
            actions.push(SettingsAction::EngineChanged);
        }
        let found = crate::engines::suggestions(s);
        if !found.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(tr("Found on this computer:")).strong());
            for cfg in found {
                ui.horizontal(|ui| {
                    let path = cfg.executable().unwrap_or_default();
                    ui.label(&cfg.name)
                        .on_hover_text(path.display().to_string());
                    if ui.button(tr("Add")).clicked() {
                        s.engines.push(cfg);
                        if s.active_engine.is_none() {
                            s.active_engine = Some(s.engines.len() - 1);
                            actions.push(SettingsAction::EngineChanged);
                        }
                        *changed = true;
                    }
                });
            }
        }
        // ChessPenguin's Stockfish can always be (re)downloaded from here.
        // Not when a Stockfish of the user's is there already (it would be the same
        // engine twice).
        let own = crate::engines::own_stockfish().is_some();
        let has_stockfish = s
            .engines
            .iter()
            .any(|e| e.name.to_lowercase().contains("stockfish") && e.executable().is_some());
        if !s.engines.is_empty() && !own && !has_stockfish && crate::engine_download::available() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if downloading {
                    self.download_progress(ui, actions);
                } else if ui
                    .button(trf(
                        "⬇ Download Stockfish 19 · {mb} MB",
                        &[("mb", &crate::engine_download::size_mb())],
                    ))
                    .on_hover_text(tr(
                        "From the official Stockfish page; installed for your account only",
                    ))
                    .clicked()
                {
                    actions.push(SettingsAction::DownloadEngine);
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.checking_engine,
                    egui::Button::new(tr("+ Choose engine file…")),
                )
                .clicked()
            {
                actions.push(SettingsAction::AddEngine);
            }
            if self.checking_engine {
                ui.spinner();
                ui.label(tr("Checking engine…"));
            }
        });
    }
}

impl SettingsWindow {
    /// The download's progress in place of its button, with a cancel button.
    fn download_progress(&self, ui: &mut Ui, actions: &mut Vec<SettingsAction>) {
        match &self.download {
            Some(crate::engine_download::Stage::Downloading { done, total }) => {
                let frac = if *total > 0 {
                    *done as f32 / *total as f32
                } else {
                    0.0
                };
                ui.add(egui::ProgressBar::new(frac).desired_width(200.0).text(trf(
                    "{done} of {total} MB",
                    &[
                        ("done", &(done / 1_000_000)),
                        ("total", &(total / 1_000_000)),
                    ],
                )));
            }
            _ => {
                ui.spinner();
                ui.label(tr("Checking and installing…"));
            }
        }
        if crate::widgets::small_close_button(ui)
            .on_hover_text(tr("Cancel download"))
            .clicked()
        {
            actions.push(SettingsAction::CancelDownload);
        }
    }
}

fn board_tab(ui: &mut Ui, s: &mut Settings, changed: &mut bool) {
    ui.label(RichText::new(tr("Piece set")).strong());
    ui.horizontal_wrapped(|ui| {
        for set in PieceSet::ALL {
            let selected = s.piece_set == set;
            let (rect, resp) = ui.allocate_exact_size(vec2(110.0, 64.0), Sense::click());
            let v = ui.visuals();
            let fill = if selected {
                v.selection.bg_fill
            } else if resp.hovered() {
                v.widgets.hovered.weak_bg_fill
            } else {
                v.faint_bg_color
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            let size = 44.0;
            let board = s.board_theme.theme();
            for (k, role) in [Role::Knight, Role::Queen].into_iter().enumerate() {
                let color = if k == 0 { Color::White } else { Color::Black };
                let r = egui::Rect::from_min_size(
                    rect.left_top() + vec2(8.0 + k as f32 * 48.0, 2.0),
                    vec2(size, size),
                );
                // Preview on the current board colours so dark sets stay readable.
                let square = if k == 0 { board.light } else { board.dark };
                ui.painter().rect_filled(r, 3.0, square);
                set.paint(ui, Piece { color, role }, r);
            }
            ui.painter().text(
                rect.center_bottom() - vec2(0.0, 4.0),
                egui::Align2::CENTER_BOTTOM,
                set.name(),
                egui::FontId::proportional(12.0),
                v.text_color(),
            );
            if resp.clicked() {
                s.piece_set = set;
                *changed = true;
            }
        }
    });
    ui.add_space(8.0);
    notation_section(ui, s, changed);
    ui.add_space(8.0);
    ui.label(RichText::new(tr("Board")).strong());
    ui.horizontal_wrapped(|ui| {
        for id in BoardThemeId::ALL {
            let t = id.theme();
            let selected = s.board_theme == id;
            let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::click());
            let sq = rect.width() / 2.0;
            if let Some(uri) = t.texture {
                // A quarter of the full-board image = 2×2 squares, like the flat swatches.
                egui::Image::new(uri)
                    .uv(egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.75),
                        egui::pos2(0.25, 1.0),
                    ))
                    .paint_at(ui, rect);
            }
            for (k, c) in [t.light, t.dark, t.dark, t.light]
                .into_iter()
                .enumerate()
                .filter(|_| t.texture.is_none())
            {
                let r = egui::Rect::from_min_size(
                    rect.min + vec2((k % 2) as f32 * sq, (k / 2) as f32 * sq),
                    vec2(sq, sq),
                );
                ui.painter().rect_filled(r, 0.0, c);
            }
            if selected {
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(3.0, ui.visuals().selection.bg_fill),
                    egui::StrokeKind::Outside,
                );
            }
            if resp.on_hover_text(id.label()).clicked() {
                s.board_theme = id;
                *changed = true;
            }
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(tr("Piece animation"));
        for a in crate::settings::Animation::ALL {
            *changed |= ui
                .selectable_value(&mut s.animation, a, a.label())
                .changed();
        }
    });
    *changed |= ui
        .checkbox(&mut s.show_coords, tr("Show coordinates"))
        .changed();
    *changed |= ui
        .checkbox(&mut s.show_best_arrow, tr("Show engine best-move arrows"))
        .changed();
}

/// "Move notation": Latin letters, figurines, the interface language's letters, with a
/// sample for each and a line of a game as it will look.
fn notation_section(ui: &mut Ui, s: &mut Settings, changed: &mut bool) {
    use crate::san::{MoveNotation, local_letters, show_in};
    let lang = crate::i18n::current();
    ui.label(RichText::new(tr("Move notation")).strong());
    // In English the language's letters are the Latin ones: that option is not shown.
    let local = local_letters(lang);
    let shown = if s.move_notation == MoveNotation::Local && local.is_none() {
        MoveNotation::Latin
    } else {
        s.move_notation
    };
    let sample = |m: MoveNotation| {
        ["Nf3", "Qxd8+", "e8=Q"]
            .map(|x| show_in(x, m, lang).into_owned())
            .join("  ")
    };
    let mut options = vec![
        (
            MoveNotation::Latin,
            tr("Letters (K Q R B N)").to_owned(),
            tr("International notation, as in PGN files and engine output"),
        ),
        (
            MoveNotation::Figurine,
            tr("Figurines").to_owned(),
            tr("Piece symbols, as on Lichess. They look the same in every language."),
        ),
    ];
    if let Some(l) = local {
        options.push((
            MoveNotation::Local,
            format!("{} ({})", tr("Interface language letters"), l.join(" ")),
            tr("Follows the interface language"),
        ));
    }
    egui::Grid::new("move_notation")
        .num_columns(2)
        .spacing(vec2(24.0, 4.0))
        .show(ui, |ui| {
            for (mode, label, tip) in options {
                let r = ui.radio(shown == mode, label).on_hover_text(tip);
                let r2 = ui
                    .add(
                        egui::Label::new(RichText::new(sample(mode)).size(15.0))
                            .sense(Sense::click()),
                    )
                    .on_hover_text(tip);
                if (r.clicked() || r2.clicked()) && s.move_notation != mode {
                    s.move_notation = mode;
                    *changed = true;
                }
                ui.end_row();
            }
        });
    // A line of a game, the current move highlighted as in the notation.
    let line = "1.e4 e5 2.Nf3 Nc6 3.Bb5 a6 4.Ba4 Nf6 5.O-O Be7 6.Re1 b5";
    let mode = shown;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for word in line.split(' ') {
                let text = RichText::new(show_in(word, mode, lang).into_owned()).size(16.0);
                if word == "3.Bb5" {
                    let sel = ui.visuals().selection;
                    let galley = egui::Label::new(text.color(sel.stroke.color).strong());
                    egui::Frame::new()
                        .fill(sel.bg_fill)
                        .corner_radius(2.0)
                        .inner_margin(egui::Margin::symmetric(3, 0))
                        .show(ui, |ui| ui.add(galley));
                } else {
                    ui.label(text);
                }
            }
        });
    });
    ui.label(
        RichText::new(tr(
            "Only how moves look on screen. PGN files and copied games always use K Q R B N.",
        ))
        .weak(),
    );
}

const SITE: &str = "https://chess.vrubel.xyz";
const REPO: &str = "https://github.com/vrubelroman/chessPenguin";
const EMAIL: &str = "vrubelroman@gmail.com";

/// Version, links to the website and the sources, support contacts.
fn about_tab(ui: &mut Ui, lang: Lang, updates: impl FnOnce(&mut Ui)) {
    let version = env!("CARGO_PKG_VERSION");
    // The website page in the interface language.
    let site = match lang {
        Lang::En => format!("{SITE}/"),
        Lang::Ru => format!("{SITE}/ru/"),
        Lang::Fr => format!("{SITE}/fr/"),
        Lang::De => format!("{SITE}/de/"),
        Lang::Es => format!("{SITE}/es/"),
        Lang::It => format!("{SITE}/it/"),
        Lang::Pt => format!("{SITE}/pt/"),
        Lang::Uz => format!("{SITE}/uz/"),
        Lang::Kk => format!("{SITE}/kk/"),
    };
    ui.add_space(6.0);
    ui.label(RichText::new("ChessPenguin").size(22.0).strong());
    ui.label(format!("{} {version}", tr("Version")));
    updates(ui);
    ui.label(
        RichText::new(tr(
            "Free chess database software for Windows, macOS and Linux.",
        ))
        .weak(),
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.hyperlink_to(tr("Website"), &site);
        ui.label("·");
        ui.hyperlink_to(tr("What's new"), format!("{REPO}/releases/tag/v{version}"));
        ui.label("·");
        ui.hyperlink_to(tr("Source code"), REPO);
    });
    ui.add_space(14.0);
    ui.label(RichText::new(tr("Found a problem or have an idea? Write to us:")).strong());
    ui.horizontal_wrapped(|ui| {
        // Copied rather than opened: a mailto link does nothing useful without a mail app.
        let copied_at = egui::Id::new("about_email_copied");
        let now = ui.input(|i| i.time);
        let copied = ui
            .data(|d| d.get_temp::<f64>(copied_at))
            .is_some_and(|t| now - t < 2.0);
        let link = ui
            .link(EMAIL)
            .on_hover_text(tr("Click to copy the address"));
        if link.clicked() {
            ui.ctx().copy_text(EMAIL.to_owned());
            ui.data_mut(|d| d.insert_temp(copied_at, now));
        }
        if copied {
            ui.label(RichText::new(tr("Copied")).weak());
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        ui.label("·");
        ui.hyperlink_to("Telegram @vrubelr", "https://t.me/vrubelr");
    });
    ui.add_space(14.0);
    ui.label(
        RichText::new(tr("Free software under GPL-3.0-or-later. Piece sets, boards and sounds come from Lichess under their own free licences."))
            .weak()
            .small(),
    );
}

fn sound_tab(ui: &mut Ui, s: &mut Settings, actions: &mut Vec<SettingsAction>, changed: &mut bool) {
    *changed |= ui
        .checkbox(&mut s.sound_enabled, tr("Move sounds"))
        .changed();
    ui.add_enabled_ui(s.sound_enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Set:"));
            for set in SoundSet::ALL {
                if ui
                    .selectable_value(&mut s.sound_set, set, set.label())
                    .changed()
                {
                    *changed = true;
                    actions.push(SettingsAction::TestSound);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("Volume:"));
            let r = ui.add(egui::Slider::new(&mut s.volume, 0.0..=1.0).show_value(false));
            *changed |= r.changed();
            if r.drag_stopped() {
                actions.push(SettingsAction::TestSound);
            }
            if ui.button(tr("▶ Test")).clicked() {
                actions.push(SettingsAction::TestSound);
            }
        });
    });
}
