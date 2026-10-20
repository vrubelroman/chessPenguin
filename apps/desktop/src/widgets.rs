//! Small shared widgets.

use egui::{Color32, Sense, Stroke, Ui, vec2};

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

/// The quiet "ℹ n" of import notes: minor remarks, so no colour and no frame.
pub fn notes_button(ui: &mut egui::Ui, notes: usize) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(format!("ℹ {notes}")).weak().small()).frame(false))
        .on_hover_text(crate::i18n::tr("Import notes"))
}

/// "⬆ Update 0.8.0" (or the download's progress, "⟳ Restart for 0.8.0"): opens the
/// update window.
pub fn update_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.button(egui::RichText::new(label).color(ui.visuals().hyperlink_color))
        .on_hover_text(crate::i18n::tr("A new version of ChessPenguin is out"))
}

/// The main button of a window: filled, a little taller.
pub fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(ui.visuals().selection.bg_fill)
            .min_size(egui::vec2(0.0, 30.0)),
    )
}
