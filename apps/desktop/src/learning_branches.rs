use std::collections::HashMap;

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use trainer::repertoire::{BranchLearningAnalytics, RepertoireId};

use crate::i18n::tr;

const WIDE_WIDTH: f32 = 900.0;
const HEADER_HEIGHT: f32 = 40.0;
const ROW_HEIGHT: f32 = 24.0;
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

    let wide = ui.available_width() >= WIDE_WIDTH;
    let include_repertoire = include_repertoire_names(rows, names);
    let mut clicked = None;
    if wide {
        show_wide(
            ui,
            rows,
            names,
            labels,
            include_repertoire,
            selected.as_ref(),
            &mut clicked,
        );
    } else {
        show_narrow(
            ui,
            rows,
            names,
            labels,
            include_repertoire,
            selected.as_ref(),
            &mut clicked,
        );
    }
    if let Some(selection) = clicked {
        *selected = Some(selection);
    }

    if !wide
        && let Some(selection) = selected.as_ref()
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

fn include_repertoire_names(
    rows: &[BranchLearningAnalytics],
    names: &HashMap<RepertoireId, String>,
) -> bool {
    names.len() > 1
        || rows.first().is_some_and(|first| {
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
fn show_wide(
    ui: &mut Ui,
    rows: &[BranchLearningAnalytics],
    names: &HashMap<RepertoireId, String>,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    include_repertoire: bool,
    selected: Option<&BranchSelection>,
    clicked: &mut Option<BranchSelection>,
) {
    let headers = [
        "Branch",
        "Total",
        "Unseen",
        "Learning",
        "Learnt",
        "Uncertain",
        "Due",
        "Accuracy",
        "Learnt %",
        "Last trained",
    ];
    let mut table = TableBuilder::new(ui)
        .id_salt("learning_branches_wide")
        .striped(true)
        .sense(Sense::click())
        .max_scroll_height(360.0)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(120.0).clip(true));
    for width in [30.0, 66.0, 70.0, 52.0, 72.0, 46.0, 68.0, 64.0, 136.0] {
        table = table.column(Column::exact(width).clip(true));
    }
    table
        .header(HEADER_HEIGHT, |mut header| {
            for (index, key) in headers.into_iter().enumerate() {
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
                let summary = &row.summary;
                let values = [
                    summary.total.to_string(),
                    summary.unseen.to_string(),
                    summary.learning.to_string(),
                    summary.learnt.to_string(),
                    summary.uncertain.to_string(),
                    summary.due.to_string(),
                    format_percent(summary.accuracy_percent),
                    format_percent(summary.learnt_percent),
                    format_time(row.last_independent_at),
                ];
                for value in values {
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

#[allow(clippy::too_many_arguments)]
fn show_narrow(
    ui: &mut Ui,
    rows: &[BranchLearningAnalytics],
    names: &HashMap<RepertoireId, String>,
    labels: &HashMap<(RepertoireId, Vec<String>), Vec<String>>,
    include_repertoire: bool,
    selected: Option<&BranchSelection>,
    clicked: &mut Option<BranchSelection>,
) {
    TableBuilder::new(ui)
        .id_salt("learning_branches_narrow")
        .striped(true)
        .sense(Sense::click())
        .max_scroll_height(192.0)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(88.0).clip(true))
        .column(Column::exact(62.0).clip(true))
        .column(Column::exact(52.0).clip(true))
        .column(Column::exact(76.0).clip(true))
        .header(HEADER_HEIGHT, |mut header| {
            for (index, key) in ["Branch", "Total", "Due", "Learnt %"]
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
                    row.summary.total.to_string(),
                    row.summary.due.to_string(),
                    format_percent(row.summary.learnt_percent),
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
        ("Unseen", row.summary.unseen.to_string()),
        ("Learning", row.summary.learning.to_string()),
        ("Learnt", row.summary.learnt.to_string()),
        ("Uncertain", row.summary.uncertain.to_string()),
        ("Due", row.summary.due.to_string()),
        ("Accuracy", format_percent(row.summary.accuracy_percent)),
        ("Last trained", format_time(row.last_independent_at)),
    ];
    egui::Grid::new("learning_branches_details")
        .num_columns(2)
        .spacing([12.0, 5.0])
        .show(ui, |ui| {
            for (label, value) in details {
                ui.add(
                    egui::Label::new(RichText::new(tr(label)).weak().size(BODY_TEXT_SIZE)).wrap(),
                );
                ui.add(egui::Label::new(RichText::new(value).size(BODY_TEXT_SIZE)).wrap());
                ui.end_row();
            }
        });
}

fn header_cell(ui: &mut Ui, text: &str, numeric: bool) -> egui::Response {
    let text = RichText::new(text).strong().size(BODY_TEXT_SIZE);
    if numeric {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(text).wrap())
        })
        .inner
    } else {
        ui.add(egui::Label::new(text).wrap())
    }
}

fn text_cell(ui: &mut Ui, text: &str, numeric: bool) -> egui::Response {
    let text = RichText::new(text).size(BODY_TEXT_SIZE);
    if numeric {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(text).truncate())
        })
        .inner
    } else {
        ui.add(egui::Label::new(text).truncate())
    }
}

fn format_percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0} %"))
}

fn format_time(at: Option<i64>) -> String {
    at.and_then(|at| chrono::DateTime::from_timestamp(at, 0))
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_owned())
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

        let names = HashMap::from([(fixture.rows[0].repertoire_id.clone(), "First".to_owned())]);
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
    fn wide_and_narrow_layouts_stay_in_bounds_in_light_and_dark() {
        let fixture = fixture();
        for visuals in [egui::Visuals::light(), egui::Visuals::dark()] {
            for (width, height) in [(1200.0, 800.0), (640.0, 480.0)] {
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
                    if width < WIDE_WIDTH {
                        for key in [
                            "Unseen",
                            "Learning",
                            "Learnt",
                            "Uncertain",
                            "Due",
                            "Accuracy",
                            "Last trained",
                        ] {
                            assert!(
                                has_translated_text(&shapes, key),
                                "missing narrow detail {key}"
                            );
                        }
                    } else {
                        for key in [
                            "Branch",
                            "Total",
                            "Unseen",
                            "Learning",
                            "Learnt",
                            "Uncertain",
                            "Due",
                            "Accuracy",
                            "Learnt %",
                            "Last trained",
                        ] {
                            assert!(
                                has_translated_text(&shapes, key),
                                "missing wide header {key}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn narrow_details_are_visible_and_click_selects_the_branch() {
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
        for key in [
            "Unseen",
            "Learning",
            "Learnt",
            "Uncertain",
            "Due",
            "Accuracy",
            "Last trained",
        ] {
            assert!(has_translated_text(&shapes, key), "missing detail {key}");
        }
    }

    #[test]
    fn null_accuracy_and_last_training_render_as_dashes() {
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
        assert!(dashes >= 3, "expected null metrics as dashes: {dashes}");
        assert_eq!(format_percent(None), "—");
        assert_eq!(format_time(None), "—");
        assert_eq!(format_time(Some(i64::MAX)), "—");
        assert_eq!(format_percent(Some(75.0)), "75 %");
        assert!(format_time(Some(BASE)).contains('-'));
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
