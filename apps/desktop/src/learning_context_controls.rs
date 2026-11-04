use trainer::repertoire::LearningContextPolicy;

use crate::i18n::tr;

pub fn show(ui: &mut egui::Ui, policy: &mut LearningContextPolicy) -> bool {
    let before = *policy;
    egui::CollapsingHeader::new(tr("Context and move speed"))
        .id_salt("learning_context_controls")
        .default_open(std::env::var_os("CP_REPERTOIRE_CONTEXT_OPEN").is_some())
        .show_unindented(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    let label = |ui: &mut egui::Ui, text: &'static str| {
                        ui.add(egui::Label::new(tr(text)).wrap());
                    };
                    let column_width = ((ui.available_width() - 12.0) / 2.0).max(1.0);
                    egui::Grid::new("learning_context_controls_grid")
                        .num_columns(2)
                        .max_col_width(column_width)
                        .spacing(egui::vec2(12.0, 6.0))
                        .show(ui, |ui| {
                            label(ui, "Reinforce minimum correct");
                            ui.add(
                                egui::DragValue::new(&mut policy.reinforce_minimum_correct)
                                    .range(0..=u32::MAX),
                            );
                            ui.end_row();

                            let mut enabled = policy.reinforce_minimum_days.is_some();
                            if ui
                                .checkbox(&mut enabled, tr("Reinforce minimum days"))
                                .changed()
                            {
                                policy.reinforce_minimum_days = enabled.then_some(7);
                            }
                            let mut days = policy.reinforce_minimum_days.unwrap_or(7);
                            if ui
                                .add_enabled(
                                    enabled,
                                    egui::DragValue::new(&mut days).range(0..=u32::MAX),
                                )
                                .changed()
                            {
                                policy.reinforce_minimum_days = Some(days);
                            }
                            ui.end_row();

                            label(ui, "Don't show start moves threshold");
                            ui.add(
                                egui::DragValue::new(
                                    &mut policy.dont_show_start_moves_threshold,
                                )
                                .range(0..=u32::MAX),
                            );
                            ui.end_row();

                            ui.checkbox(
                                &mut policy.use_fixed_piece_speed,
                                tr("Use fixed piece movement speed"),
                            );
                            ui.end_row();

                            label(ui, "Move speed adjustment");
                            ui.add_enabled(
                                !policy.use_fixed_piece_speed,
                                egui::DragValue::new(&mut policy.move_speed_adjustment)
                                    .range(-2.0..=2.0)
                                    .speed(0.05)
                                    .fixed_decimals(1),
                            )
                            .on_hover_text(tr("Range −2…2. Adaptive speed = adjustment × (1 + learned fraction), limited to 0.25×–4×: 1.5 gives 3× for a fully learned move; 0 or less gives 0.25×."));
                            ui.end_row();

                            ui.checkbox(
                                &mut policy.consider_leadup_moves,
                                tr("Consider leadup moves when training branch"),
                            );
                            ui.end_row();
                        });
                });
            });
        });
    *policy != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        ctx: &egui::Context,
        width: f32,
        height: f32,
        container_width: f32,
        time: f64,
        events: Vec<egui::Event>,
        policy: &mut LearningContextPolicy,
    ) -> (Vec<egui::epaint::ClippedShape>, bool, egui::Rect) {
        let mut changed = false;
        let mut container =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(container_width, height));
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let available = ui.available_rect_before_wrap();
                    container = egui::Rect::from_min_size(
                        available.min,
                        egui::vec2(container_width, available.height()),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(container), |ui| {
                        ui.set_clip_rect(container);
                        changed = show(ui, policy);
                    });
                });
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, changed, container)
    }

    fn text_rect(shapes: &[egui::epaint::ClippedShape], key: &'static str) -> Option<egui::Rect> {
        let translations = crate::i18n::all_languages(key);
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text)
                if translations
                    .iter()
                    .any(|candidate| *candidate == text.galley.text()) =>
            {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    }

    fn text_position(
        shapes: &[egui::epaint::ClippedShape],
        key: &'static str,
    ) -> Option<egui::Pos2> {
        text_rect(shapes, key).map(|rect| rect.min + egui::vec2(8.0, 6.0))
    }

    fn click(position: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn controls_fit_embedded_side_widths_and_report_only_real_changes() {
        for (width, height) in [(1400.0, 900.0), (640.0, 480.0)] {
            for container_width in [320.0, 360.0, 420.0] {
                let ctx = egui::Context::default();
                let mut policy = LearningContextPolicy::default();
                let (collapsed, changed, _) = render(
                    &ctx,
                    width,
                    height,
                    container_width,
                    0.0,
                    Vec::new(),
                    &mut policy,
                );
                assert!(!changed);
                assert!(text_position(&collapsed, "Context and move speed").is_some());
                assert!(text_position(&collapsed, "Reinforce minimum correct").is_none());

                let header = text_position(&collapsed, "Context and move speed").unwrap();
                let (_, changed, _) = render(
                    &ctx,
                    width,
                    height,
                    container_width,
                    1.0,
                    click(header),
                    &mut policy,
                );
                assert!(!changed, "expanding the header must not save settings");
                assert_eq!(policy, LearningContextPolicy::default());
                let (expanded, changed, container) = render(
                    &ctx,
                    width,
                    height,
                    container_width,
                    2.0,
                    Vec::new(),
                    &mut policy,
                );
                assert!(!changed);
                for key in [
                    "Reinforce minimum correct",
                    "Reinforce minimum days",
                    "Don't show start moves threshold",
                    "Use fixed piece movement speed",
                    "Move speed adjustment",
                    "Consider leadup moves when training branch",
                ] {
                    assert!(
                        text_position(&expanded, key).is_some(),
                        "missing {key} in {container_width} at {width}x{height}; {container:?}; visible: {:?}",
                        expanded
                            .iter()
                            .filter_map(|clipped| match &clipped.shape {
                                egui::Shape::Text(text) => Some(text.galley.text()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    );
                }
                for clipped in &expanded {
                    if let egui::Shape::Text(text) = &clipped.shape {
                        let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                        assert!(
                            rect.min.x >= container.min.x
                                && rect.min.y >= container.min.y
                                && rect.max.x <= container.max.x + 0.01
                                && rect.max.y <= container.max.y + 0.01,
                            "text outside {container:?}: {rect:?} {}",
                            text.galley.text()
                        );
                    }
                }
                let indent = text_rect(&expanded, "Reinforce minimum correct")
                    .unwrap()
                    .left()
                    - container.left();
                assert!((indent - 16.0).abs() < 0.01, "body indent: {indent}");

                let fixed = text_position(&expanded, "Use fixed piece movement speed").unwrap();
                let (_, changed, _) = render(
                    &ctx,
                    width,
                    height,
                    container_width,
                    3.0,
                    click(fixed),
                    &mut policy,
                );
                assert!(changed);
                assert!(!policy.use_fixed_piece_speed);
                let (_, changed, _) = render(
                    &ctx,
                    width,
                    height,
                    container_width,
                    4.0,
                    Vec::new(),
                    &mut policy,
                );
                assert!(!changed, "an unchanged frame must not save settings");

                let days = text_position(&expanded, "Reinforce minimum days").unwrap();
                let (_, changed, _) = render(
                    &ctx,
                    width,
                    height,
                    container_width,
                    5.0,
                    click(days),
                    &mut policy,
                );
                assert!(changed);
                assert_eq!(policy.reinforce_minimum_days, Some(7));
            }
        }
    }
}
