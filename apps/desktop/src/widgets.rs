//! Small shared widgets.

use crate::i18n::tr;
use egui::{Color32, Sense, Stroke, Ui, vec2};

/// Window close button (the app runs undecorated under tiling compositors).
pub fn close_button(ui: &mut Ui) -> egui::Response {
    x_button(ui, vec2(32.0, 28.0), 6.0).on_hover_text(tr("Close the application"))
}

/// Compact ✕ (e.g. to cancel a running task); the caller adds the tooltip.
pub fn small_close_button(ui: &mut Ui) -> egui::Response {
    x_button(ui, vec2(22.0, 20.0), 4.5)
}

fn x_button(ui: &mut Ui, size: egui::Vec2, d: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, Color32::from_rgb(0xc4, 0x2b, 0x1c));
    }
    let color = if resp.hovered() {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    let c = rect.center();
    let stroke = Stroke::new(1.6, color);
    ui.painter()
        .line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
    ui.painter()
        .line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    resp
}
