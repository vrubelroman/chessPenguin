use trainer::repertoire::LearningContextPolicy;

use crate::i18n::tr;

fn hint(ui: &mut egui::Ui, text: &'static str) {
    ui.add(egui::Label::new(egui::RichText::new(tr(text)).weak()).wrap());
}

pub fn show(ui: &mut egui::Ui, policy: &mut LearningContextPolicy) -> bool {
    let before = *policy;
    ui.strong(tr("Moves before the task"));
    hint(ui, "The app shows the moves leading to the task position.");
    ui.add_space(6.0);
    ui.checkbox(
        &mut policy.consider_leadup_moves,
        tr("Ask me to play preceding moves"),
    );
    ui.add_enabled_ui(
        policy.consider_leadup_moves || !policy.use_fixed_piece_speed,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Correct answers in a row"));
                ui.add(
                    egui::DragValue::new(&mut policy.reinforce_minimum_correct).range(0..=u32::MAX),
                );
            });
        },
    );
    hint(
        ui,
        "Ask until this streak is reached. This also sets the maximum acceleration point for adaptive playback.",
    );
    ui.add_space(6.0);
    ui.add_enabled_ui(policy.consider_leadup_moves, |ui| {
        let mut enabled = policy.reinforce_minimum_days.is_some();
        if ui
            .checkbox(&mut enabled, tr("Review ahead of schedule"))
            .changed()
        {
            policy.reinforce_minimum_days = enabled.then_some(7);
        }
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Days before the due date"));
                let mut days = policy.reinforce_minimum_days.unwrap_or(7);
                if ui
                    .add(egui::DragValue::new(&mut days).range(0..=u32::MAX))
                    .changed()
                {
                    policy.reinforce_minimum_days = Some(days);
                }
            });
        });
    });
    hint(ui, "Also ask moves that are already due.");
    ui.add_space(10.0);
    ui.separator();
    ui.strong(tr("Skip a familiar opening"));
    ui.horizontal_wrapped(|ui| {
        ui.label(tr("Skip after this many correct answers in a row"));
        ui.add(
            egui::DragValue::new(&mut policy.dont_show_start_moves_threshold).range(0..=u32::MAX),
        );
    });
    hint(
        ui,
        "Each of your opening moves must reach this streak. 0 means always show the opening.",
    );
    ui.add_space(10.0);
    ui.separator();
    ui.strong(tr("Playback speed"));
    let mut adaptive = !policy.use_fixed_piece_speed;
    if ui
        .checkbox(&mut adaptive, tr("Speed up familiar moves"))
        .changed()
    {
        policy.use_fixed_piece_speed = !adaptive;
    }
    hint(
        ui,
        "Base animation speed comes from board settings. Familiar moves play up to twice as fast.",
    );
    if adaptive {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Base speed multiplier"));
            ui.add(egui::DragValue::new(&mut policy.move_speed_adjustment)
                .range(-2.0..=2.0).speed(0.05).fixed_decimals(2).suffix(" ×"))
                .on_hover_text(tr("Range −2…2. Adaptive speed = adjustment × (1 + learned fraction), limited to 0.25×–4×: 1.5 gives 3× for a fully learned move; 0 or less gives 0.25×."));
        });
        hint(ui, "0 or less uses the minimum speed (0.25×).");
    }
    ui.add_space(6.0);
    hint(ui, "Learning counters do not use these playback settings.");
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
    fn context_options_preserve_values_until_changed_and_enable_dependencies() {
        let ctx = egui::Context::default();
        let mut policy = LearningContextPolicy::default();
        let (shapes, changed, _) = render(&ctx, 640.0, 1000.0, 440.0, 0.0, Vec::new(), &mut policy);
        assert!(!changed);
        assert_eq!(policy, LearningContextPolicy::default());
        let leadup = text_position(&shapes, "Ask me to play preceding moves").unwrap();
        let (_, changed, _) = render(&ctx, 640.0, 1000.0, 440.0, 1.0, click(leadup), &mut policy);
        assert!(changed);
        assert!(policy.consider_leadup_moves);
        let (shapes, changed, _) = render(&ctx, 640.0, 1000.0, 440.0, 2.0, Vec::new(), &mut policy);
        assert!(!changed);
        let days = text_position(&shapes, "Review ahead of schedule").unwrap();
        let (_, changed, _) = render(&ctx, 640.0, 1000.0, 440.0, 3.0, click(days), &mut policy);
        assert!(changed);
        assert_eq!(policy.reinforce_minimum_days, Some(7));
        let speed = text_position(&shapes, "Speed up familiar moves").unwrap();
        let (_, changed, _) = render(&ctx, 640.0, 1000.0, 440.0, 4.0, click(speed), &mut policy);
        assert!(changed);
        assert!(!policy.use_fixed_piece_speed);
        assert_eq!(policy.move_speed_adjustment, 1.0);
    }
}
