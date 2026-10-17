//! Small shared widgets.

use crate::i18n::tr;
use egui::{Color32, Sense, Stroke, Ui, vec2};

/// Window close button (the app runs undecorated under tiling compositors).
pub fn close_button(ui: &mut Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 28.0), Sense::click());
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
    let d = 6.0;
    let stroke = Stroke::new(1.6, color);
    ui.painter()
        .line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
    ui.painter()
        .line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    resp.on_hover_text(tr("Close the application"))
}
