use std::collections::HashMap;

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use trainer::repertoire::{BranchLearningAnalytics, RepertoireId};

use crate::i18n::tr;

const HEADER_HEIGHT: f32 = 40.0;
const ROW_HEIGHT: f32 = 32.0;
const BODY_TEXT_SIZE: f32 = 14.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchSelection {
    pub repertoire_id: RepertoireId,
    pub branch_root: Vec<String>,
}

pub fn show(
    ui: &mut egui::Ui,
    rows: &[BranchLearningAnalytics],
    names: &HashMap<RepertoireId, String>,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    selected: &mut Option<BranchSelection>,
) {
    ui.style_mut().interaction.selectable_labels = false;
    if rows.is_empty() {
        *selected = None;
        ui.label(tr("No branches in this scope"));
        return;
    }
    if selected
        .as_ref()
        .is_none_or(|selection| !rows.iter().any(|row| selection == &selection_for(row)))
    {
        *selected = Some(selection_for(&rows[0]));
    }

    ui.add(
        egui::Label::new(tr(
            "Continuations to review. Shared moves may appear in several variations.",
        ))
        .wrap(),
    );
    ui.add_space(6.0);
    let include_repertoire = include_repertoire_names(rows);
    let mut clicked = None;
    show_table(
        ui,
        rows,
        names,
        labels,
        include_repertoire,
        selected.as_ref(),
        &mut clicked,
    );
    if let Some(selection) = clicked {
        *selected = Some(selection);
    }

    if let Some(selection) = selected.as_ref()
        && let Some(row) = rows.iter().find(|row| selection == &selection_for(row))
    {
        ui.separator();
        show_details(ui, row, names, labels, include_repertoire);
    }
}

fn selection_for(row: &BranchLearningAnalytics) -> BranchSelection {
    BranchSelection {
        repertoire_id: row.repertoire_id.clone(),
        branch_root: row.branch_root.clone(),
    }
}

fn include_repertoire_names(rows: &[BranchLearningAnalytics]) -> bool {
    rows.first().is_some_and(|first| {
        rows.iter()
            .any(|row| row.repertoire_id != first.repertoire_id)
    })
}

fn san_path(
    row: &BranchLearningAnalytics,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
) -> String {
    let labels = labels.get(&(row.repertoire_id.clone(), row.branch_root.clone()));
    row.branch_root
        .iter()
        .enumerate()
        .map(|(index, uci)| {
            let label = labels.and_then(|labels| labels.get(index)).unwrap_or(uci);
            crate::san::show(label).into_owned()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn branch_text(
    row: &BranchLearningAnalytics,
    names: &HashMap<RepertoireId, String>,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    include_repertoire: bool,
) -> String {
    let path = san_path(row, labels);
    if !include_repertoire {
        return if path.is_empty() {
            "—".to_owned()
        } else {
            path
        };
    }
    let name = names
        .get(&row.repertoire_id)
        .map_or_else(|| row.repertoire_id.as_str(), String::as_str);
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {path}")
    }
}

#[allow(clippy::too_many_arguments)]
fn show_table(
    ui: &mut Ui,
    rows: &[BranchLearningAnalytics],
    names: &HashMap<RepertoireId, String>,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    include_repertoire: bool,
    selected: Option<&BranchSelection>,
    clicked: &mut Option<BranchSelection>,
) {
    let numeric_width = (ui.available_width() - 3.0 * ui.spacing().item_spacing.x) * 0.4 / 3.0;
    TableBuilder::new(ui)
        .id_salt("learning_branches")
        .striped(true)
        .sense(Sense::click())
        .max_scroll_height(192.0)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().clip(true))
        .column(Column::exact(numeric_width).clip(true))
        .column(Column::exact(numeric_width).clip(true))
        .column(Column::exact(numeric_width).clip(true))
        .header(HEADER_HEIGHT, |mut header| {
            for (index, key) in ["Variation start", "Learnt", "Due now", "Accuracy"]
                .into_iter()
                .enumerate()
            {
                header.col(|ui| {
                    header_cell(ui, tr(key), index > 0);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut table_row| {
                let row = &rows[table_row.index()];
                let selection = selection_for(row);
                table_row.set_selected(selected == Some(&selection));
                table_row.col(|ui| {
                    let text = branch_text(row, names, labels, include_repertoire);
                    text_cell(ui, &text, false).on_hover_text(text);
                });
                for value in [
                    format!("{} / {}", row.summary.learnt, row.summary.total),
                    row.summary.due.to_string(),
                    format_percent(row.summary.accuracy_percent),
                ] {
                    table_row.col(|ui| {
                        text_cell(ui, &value, true);
                    });
                }
                if table_row.response().clicked() {
                    *clicked = Some(selection);
                }
            });
        });
}

fn show_details(
    ui: &mut Ui,
    row: &BranchLearningAnalytics,
    names: &HashMap<RepertoireId, String>,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    include_repertoire: bool,
) {
    let branch = branch_text(row, names, labels, include_repertoire);
    ui.add(egui::Label::new(RichText::new(&branch).strong().size(BODY_TEXT_SIZE)).wrap())
        .on_hover_text(branch);
    ui.add_space(4.0);
    let details = [
        (
            "Learnt moves",
            format!("{} / {}", row.summary.learnt, row.summary.total),
        ),
        ("Due now", row.summary.due.to_string()),
        ("Not practised yet", row.summary.unseen.to_string()),
        ("Accuracy", format_percent(row.summary.accuracy_percent)),
    ];
    show_details_rows(ui, &details);
}

fn show_details_rows(ui: &mut Ui, details: &[(&'static str, String)]) {
    let label_width = (ui.available_width() * 0.36).clamp(110.0, 160.0);
    TableBuilder::new(ui)
        .id_salt("learning_branches_details")
        .vscroll(false)
        .cell_layout(Layout::left_to_right(Align::Min))
        .column(Column::exact(label_width).clip(true))
        .column(Column::remainder().at_least(80.0).clip(true))
        .body(|body| {
            body.rows(36.0, details.len(), |mut row| {
                let (label, value) = &details[row.index()];
                row.col(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(tr(label)).weak().size(BODY_TEXT_SIZE))
                            .wrap(),
                    );
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(RichText::new(value).size(BODY_TEXT_SIZE)).wrap());
                });
            });
        });
}

fn header_cell(ui: &mut Ui, text: &str, _numeric: bool) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).strong().size(BODY_TEXT_SIZE)).wrap())
}

fn text_cell(ui: &mut Ui, text: &str, _numeric: bool) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).size(BODY_TEXT_SIZE)).truncate())
}

fn format_percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0} %"))
}

#[cfg(test)]
mod tests {
    use trainer::repertoire::{LearningAggregate, Repertoire};

    use super::*;

    const BASE: i64 = 1_800_000_000;

    struct Fixture {
        rows: Vec<BranchLearningAnalytics>,
        names: HashMap<RepertoireId, String>,
        labels: HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    }

    fn fixture() -> Fixture {
        let first = Repertoire::default().id;
        let second = Repertoire::default().id;
        let branch_root = vec!["e2e4".to_owned(), "e7e5".to_owned()];
        let summary = LearningAggregate {
            total: 8,
            unseen: 2,
            learning: 2,
            learnt: 4,
            uncertain: 1,
            learnt_confident: 3,
            due: 3,
            learnt_percent: Some(50.0),
            accuracy_correct: 6,
            accuracy_wrong: 2,
            accuracy_attempts: 8,
            accuracy_percent: Some(75.0),
            ..LearningAggregate::default()
        };
        let rows = vec![
            BranchLearningAnalytics {
                repertoire_id: first.clone(),
                branch_root: branch_root.clone(),
                summary: summary.clone(),
                last_independent_at: Some(BASE),
            },
            BranchLearningAnalytics {
                repertoire_id: second.clone(),
                branch_root: branch_root.clone(),
                summary,
                last_independent_at: Some(BASE),
            },
        ];
        let names = HashMap::from([
            (first.clone(), "First".to_owned()),
            (second.clone(), "Second".to_owned()),
        ]);
        let labels = HashMap::from([
            (
                (first, branch_root.clone()),
                vec!["1.e4".to_owned(), "1...e5".to_owned()],
            ),
            ((second, branch_root), vec!["1.e4".to_owned()]),
        ]);
        Fixture {
            rows,
            names,
            labels,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        ctx: &egui::Context,
        width: f32,
        height: f32,
        time: f64,
        events: Vec<egui::Event>,
        rows: &[BranchLearningAnalytics],
        names: &HashMap<RepertoireId, String>,
        labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
        selected: &mut Option<BranchSelection>,
    ) -> Vec<egui::epaint::ClippedShape> {
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
                    show(ui, rows, names, labels, selected);
                });
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        shapes
    }

    fn has_text(shapes: &[egui::epaint::ClippedShape], expected: &str) -> bool {
        shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => text.galley.text().replace('\n', " ") == expected,
            _ => false,
        })
    }

    fn has_translated_text(shapes: &[egui::epaint::ClippedShape], key: &'static str) -> bool {
        crate::i18n::all_languages(key)
            .into_iter()
            .any(|translation| has_text(shapes, translation))
    }

    fn text_position(shapes: &[egui::epaint::ClippedShape], expected: &str) -> Option<egui::Pos2> {
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().replace('\n', " ") == expected => {
                Some(text.pos + egui::vec2(8.0, 6.0))
            }
            _ => None,
        })
    }

    fn text_occurrences(shapes: &[egui::epaint::ClippedShape], expected: &str) -> usize {
        shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => text.galley.text().replace('\n', " ") == expected,
                _ => false,
            })
            .count()
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

    fn assert_bounds(shapes: &[egui::epaint::ClippedShape], width: f32, height: f32) {
        for clipped in shapes {
            let bounds = clipped.shape.visual_bounding_rect();
            if bounds.is_finite() && bounds.is_positive() {
                assert!(
                    bounds.min.x >= -0.5
                        && bounds.min.y >= -0.5
                        && bounds.max.x <= width + 0.5
                        && bounds.max.y <= height + 0.5,
                    "shape outside {width}x{height}: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn summarized_shared_rows_are_not_deduplicated_and_labels_fall_back_to_uci() {
        let fixture = fixture();
        let ctx = egui::Context::default();
        let mut selected = None;
        let shapes = render(
            &ctx,
            1200.0,
            800.0,
            0.0,
            Vec::new(),
            &fixture.rows,
            &fixture.names,
            &fixture.labels,
            &mut selected,
        );
        assert_eq!(selected, Some(selection_for(&fixture.rows[0])));
        assert!(has_text(&shapes, "First · 1.e4 1...e5"));
        assert!(has_text(&shapes, "Second · 1.e4 e7e5"));

        let names = fixture.names.clone();
        let shapes = render(
            &ctx,
            1200.0,
            800.0,
            1.0,
            Vec::new(),
            &fixture.rows[..1],
            &names,
            &fixture.labels,
            &mut selected,
        );
        assert!(has_text(&shapes, "1.e4 1...e5"));
        assert!(!has_text(&shapes, "First · 1.e4 1...e5"));
    }

    #[test]
    fn unified_layout_stays_in_bounds_in_light_and_dark() {
        let fixture = fixture();
        for visuals in [egui::Visuals::light(), egui::Visuals::dark()] {
            for (width, height) in [(1400.0, 900.0), (960.0, 600.0), (640.0, 480.0)] {
                let ctx = egui::Context::default();
                ctx.set_visuals(visuals.clone());
                let mut selected = None;
                for pass in 0..2 {
                    let shapes = render(
                        &ctx,
                        width,
                        height,
                        pass as f64,
                        Vec::new(),
                        &fixture.rows,
                        &fixture.names,
                        &fixture.labels,
                        &mut selected,
                    );
                    assert!(!shapes.is_empty());
                    assert_bounds(&shapes, width, height);
                    for key in [
                        "Variation start",
                        "Learnt",
                        "Due now",
                        "Accuracy",
                        "Learnt moves",
                        "Not practised yet",
                    ] {
                        assert!(
                            has_translated_text(&shapes, key),
                            "missing header/detail {key}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn headers_fit_accepted_matrix_in_russian_and_german() {
        let cases = [
            ["Начало варианта", "Изучено", "Повторить", "Точность"],
            ["Variantenanfang", "Gelernt", "Wiederholen", "Genauigkeit"],
        ];
        for visuals in [egui::Visuals::light(), egui::Visuals::dark()] {
            for (width, height) in [(960.0, 600.0), (1400.0, 900.0)] {
                for headers in &cases {
                    let ctx = egui::Context::default();
                    ctx.set_visuals(visuals.clone());
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let numeric_width =
                                (ui.available_width() - 3.0 * ui.spacing().item_spacing.x) * 0.4
                                    / 3.0;
                            let mut table = TableBuilder::new(ui)
                                .id_salt("learning_branches_wide_header_test")
                                .cell_layout(Layout::left_to_right(Align::Center))
                                .column(Column::remainder().at_least(120.0).clip(true));
                            for _ in 0..3 {
                                table = table.column(Column::exact(numeric_width).clip(true));
                            }
                            table.header(HEADER_HEIGHT, |mut header| {
                                for (index, key) in headers.iter().enumerate() {
                                    header.col(|ui| {
                                        header_cell(ui, tr(key), index > 0);
                                    });
                                }
                            });
                        },
                    );
                    let shapes = output.shapes.clone();
                    output.drop_without_applying_deltas();
                    assert_bounds(&shapes, width, height);
                    for expected in headers {
                        let (rect, clip, lines) = shapes
                            .iter()
                            .find_map(|clipped| match &clipped.shape {
                                egui::Shape::Text(text)
                                    if text.galley.text().replace('\n', " ") == *expected =>
                                {
                                    Some((
                                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                                        clipped.clip_rect,
                                        text.galley.text().lines().count(),
                                    ))
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| {
                                panic!("missing full header {expected} at {width}x{height}")
                            });
                        assert!(lines <= 2, "header wraps too much: {expected}");
                        assert!(
                            rect.min.x >= clip.min.x - 0.1
                                && rect.min.y >= clip.min.y - 0.1
                                && rect.max.x <= clip.max.x + 0.1
                                && rect.max.y <= clip.max.y + 0.1,
                            "clipped header {expected} at {width}x{height}: {rect:?} {clip:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn details_are_visible_and_click_selects_the_branch() {
        let fixture = fixture();
        let ctx = egui::Context::default();
        let mut selected = None;
        let shapes = render(
            &ctx,
            640.0,
            480.0,
            0.0,
            Vec::new(),
            &fixture.rows,
            &fixture.names,
            &fixture.labels,
            &mut selected,
        );
        assert_eq!(selected, Some(selection_for(&fixture.rows[0])));
        let second = text_position(&shapes, "Second · 1.e4 e7e5")
            .expect("second branch row should be visible");
        let shapes = render(
            &ctx,
            640.0,
            480.0,
            1.0,
            click(second),
            &fixture.rows,
            &fixture.names,
            &fixture.labels,
            &mut selected,
        );
        assert_eq!(selected, Some(selection_for(&fixture.rows[1])));
        assert!(text_occurrences(&shapes, "Second · 1.e4 e7e5") >= 2);
        for key in ["Learnt moves", "Due now", "Not practised yet", "Accuracy"] {
            assert!(has_translated_text(&shapes, key), "missing detail {key}");
        }
    }

    #[test]
    fn details_keep_long_labels_readable_at_all_widths() {
        let cases: Vec<Vec<(&str, String)>> = vec![
            vec![
                ("Новый", "2".to_owned()),
                ("Изучается", "1".to_owned()),
                ("Изучено", "4".to_owned()),
                ("Не подтверждено", "1".to_owned()),
                ("К повторению", "3".to_owned()),
                ("Точность", "75 %".to_owned()),
                ("Последняя тренировка", "2026-11-04 08:26".to_owned()),
            ],
            vec![
                ("Neu", "2".to_owned()),
                ("Wird gelernt", "1".to_owned()),
                ("Gelernt", "4".to_owned()),
                ("Nicht bestätigt", "1".to_owned()),
                ("Fällig", "3".to_owned()),
                ("Genauigkeit", "75 %".to_owned()),
                ("Zuletzt trainiert", "2026-11-04 08:26".to_owned()),
            ],
        ];
        for visuals in [egui::Visuals::light(), egui::Visuals::dark()] {
            for width in [320.0, 360.0, 420.0, 640.0] {
                for details in &cases {
                    let ctx = egui::Context::default();
                    ctx.set_visuals(visuals.clone());
                    let mut expected_label_width = 0.0;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 700.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            egui::CentralPanel::default().show(ui, |ui| {
                                expected_label_width =
                                    (ui.available_width() * 0.36).clamp(110.0, 160.0);
                                show_details_rows(ui, details);
                            });
                        },
                    );
                    let shapes = output.shapes.clone();
                    output.drop_without_applying_deltas();
                    assert_bounds(&shapes, width, 700.0);
                    for (label, _) in details {
                        let (clip, lines) = shapes
                            .iter()
                            .find_map(|clipped| match &clipped.shape {
                                egui::Shape::Text(text)
                                    if text.galley.text().replace('\n', " ") == *label =>
                                {
                                    Some((clipped.clip_rect, text.galley.text().lines().count()))
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| panic!("missing full label {label} at {width}"));
                        assert!(
                            (clip.width() - expected_label_width).abs() < 0.1,
                            "label column at {width}: {clip:?}, expected {expected_label_width}: {label}"
                        );
                        assert!(lines <= 2, "label wraps too much at {width}: {label}");
                    }
                    assert!(has_text(&shapes, "2026-11-04 08:26"));
                    assert!(has_text(&shapes, "75 %"));
                }
            }
        }
    }

    #[test]
    fn null_accuracy_renders_as_dashes_in_table_and_details() {
        let fixture = fixture();
        let mut row = fixture.rows[0].clone();
        row.summary.accuracy_percent = None;
        row.summary.learnt_percent = None;
        row.last_independent_at = None;
        let names = HashMap::from([(row.repertoire_id.clone(), "Only".to_owned())]);
        let labels = HashMap::new();
        let ctx = egui::Context::default();
        let mut selected = None;
        let shapes = render(
            &ctx,
            640.0,
            480.0,
            0.0,
            Vec::new(),
            std::slice::from_ref(&row),
            &names,
            &labels,
            &mut selected,
        );
        let dashes = text_occurrences(&shapes, "—");
        assert!(
            dashes >= 2,
            "expected null accuracy in table and details: {dashes}"
        );
        assert_eq!(format_percent(None), "—");
        assert_eq!(format_percent(Some(75.0)), "75 %");
    }

    #[test]
    fn long_selected_notation_wraps_in_details_and_row_retains_wide_column() {
        let mut fixture = fixture();
        fixture.rows.truncate(1);
        fixture.rows[0].branch_root = (0..40).map(|i| format!("move{i}")).collect();
        let notation: Vec<_> = (1..=40).map(|i| format!("{i}.Nf3")).collect();
        fixture.labels.insert(
            (
                fixture.rows[0].repertoire_id.clone(),
                fixture.rows[0].branch_root.clone(),
            ),
            notation,
        );
        let expected = san_path(&fixture.rows[0], &fixture.labels);
        for width in [960.0, 1400.0] {
            let ctx = egui::Context::default();
            let mut selected = None;
            let shapes = render(
                &ctx,
                width,
                900.0,
                0.0,
                Vec::new(),
                &fixture.rows,
                &fixture.names,
                &fixture.labels,
                &mut selected,
            );
            assert_bounds(&shapes, width, 900.0);
            let texts: Vec<_> = shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == expected => {
                        Some((clipped, text))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(texts.len(), 2, "table row and full selected notation");
            assert!(texts[0].0.clip_rect.width() >= width * 0.5);
            assert_eq!(
                texts[0].1.galley.rows.len(),
                1,
                "table notation stays on one line"
            );
            assert!(
                texts[1].1.galley.rows.len() > 1,
                "full selected notation wraps"
            );
            assert!(has_text(&shapes, "4 / 8"));
        }
    }

    #[test]
    fn empty_scope_clears_selection() {
        let fixture = fixture();
        let ctx = egui::Context::default();
        let mut selected = Some(selection_for(&fixture.rows[0]));
        let shapes = render(
            &ctx,
            640.0,
            480.0,
            0.0,
            Vec::new(),
            &[],
            &fixture.names,
            &fixture.labels,
            &mut selected,
        );
        assert_eq!(selected, None);
        assert!(has_translated_text(&shapes, "No branches in this scope"));
    }
}
