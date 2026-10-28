//! Small shared widgets.

use egui::{Color32, Sense, Stroke, Ui, vec2};

/// A dev check (`CP_PANEL_CHECK=1`): a panel whose content is wider than the panel. egui
/// then moves a right panel out (its left part under the central panel, its right part
/// off the window); the content of every panel must fit its least width
/// (docs/design/narrow-windows.md).
pub fn check_panel(name: &str, panel: &egui::Response) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| {
        let on = std::env::var_os("CP_PANEL_CHECK").is_some();
        // In a debug build egui also marks (red) what makes its parent wider.
        #[cfg(debug_assertions)]
        if on {
            panel
                .ctx
                .all_styles_mut(|s| s.debug.show_expand_width = true);
        }
        on
    });
    if !on {
        return;
    }
    let window = panel.ctx.content_rect();
    let over = (panel.rect.right() - window.right()).max(window.left() - panel.rect.left());
    if over > 0.5 {
        eprintln!(
            "CP_PANEL_CHECK: panel {name} is {over:.0} pt wider than its place (window {:.0} pt)",
            window.width()
        );
    }
}

/// A sortable column's header: the title in bold, ▲/▼ when the table is ordered by
/// it (`Some(ascending)`); click to sort.
pub fn sort_header(ui: &mut Ui, label: &str, ascending: Option<bool>) -> egui::Response {
    let arrow = match ascending {
        Some(true) => " ⏶",
        Some(false) => " ⏷",
        None => "",
    };
    ui.add(egui::Button::new(egui::RichText::new(format!("{label}{arrow}")).strong()).frame(false))
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
    primary_wide(ui, label, 0.0)
}

/// `primary()` at least `width` wide (a button whose label changes keeps its place).
pub fn primary_wide(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(ui.visuals().selection.bg_fill)
            .min_size(egui::vec2(width, 30.0)),
    )
}

/// A 30 × 30 button with a painted ✓ (`check`) or ✕: the font has no such glyphs. The
/// main one (`primary`) is filled like `primary()`.
pub fn icon_button(ui: &mut Ui, check: bool, primary: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let visuals = ui.style().interact(&resp);
    let sel = ui.visuals().selection;
    let (fill, color) = if primary {
        (
            if resp.hovered() {
                sel.bg_fill.gamma_multiply(1.15)
            } else {
                sel.bg_fill
            },
            sel.stroke.color,
        )
    } else {
        (visuals.weak_bg_fill, visuals.text_color())
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    if !primary {
        ui.painter()
            .rect_stroke(rect, 4.0, visuals.bg_stroke, egui::StrokeKind::Inside);
    }
    let c = rect.center();
    let stroke = Stroke::new(2.0, color);
    if check {
        ui.painter().add(egui::Shape::line(
            vec![
                c + vec2(-5.0, 0.0),
                c + vec2(-1.5, 4.0),
                c + vec2(5.5, -4.5),
            ],
            stroke,
        ));
    } else {
        let d = 4.5;
        ui.painter()
            .line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
        ui.painter()
            .line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    }
    resp
}
