//! Settings window: engines, board & pieces, sound, interface.

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
}

pub enum SettingsAction {
    AddEngine,
    TestSound,
    /// The active engine (or its options) changed; a running analysis should restart.
    EngineChanged,
}

pub struct SettingsWindow {
    pub open: bool,
    pub tab: Tab,
    pub checking_engine: bool,
}

impl SettingsWindow {
    pub fn new() -> Self {
        Self {
            open: false,
            tab: Tab::default(),
            checking_engine: false,
        }
    }

    /// Returns actions and whether settings changed (to be saved).
    pub fn show(&mut self, ctx: &egui::Context, s: &mut Settings) -> (Vec<SettingsAction>, bool) {
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
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Engines => self.engines(ui, s, &mut actions, &mut changed),
                    Tab::Board => board_tab(ui, s, &mut changed),
                    Tab::Sound => sound_tab(ui, s, &mut actions, &mut changed),
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
                        if ui
                            .button("🗑")
                            .on_hover_text(tr("Remove from list"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                });
                let e = &s.engines[i];
                let location = if e.builtin {
                    match crate::engines::bundled_stockfish() {
                        Some(p) => trf(
                            "Shipped with ChessPenguin: {path}",
                            &[("path", &p.display())],
                        ),
                        None => tr("Shipped with ChessPenguin: not part of this build").into(),
                    }
                } else {
                    e.path.display().to_string()
                };
                ui.label(RichText::new(location).small().weak());
            });
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
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.checking_engine,
                    egui::Button::new(tr("+ Add engine…")),
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
    *changed |= ui
        .checkbox(&mut s.show_coords, tr("Show coordinates"))
        .changed();
    *changed |= ui
        .checkbox(&mut s.show_best_arrow, tr("Show engine best-move arrows"))
        .changed();
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
