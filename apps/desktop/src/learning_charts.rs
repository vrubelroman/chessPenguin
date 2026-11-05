use std::{collections::BTreeMap, fmt};

use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Visuals,
    pos2, vec2,
};
use trainer::repertoire::{LearningSnapshot, ScheduledBuckets, SequenceHistograms};

use crate::i18n::tr;

const CHART_HEIGHT: f32 = 180.0;
const HISTORY_DATE_HEIGHT: f32 = 18.0;
const SCHEDULED_LABEL_HEIGHT: f32 = 40.0;
const HISTOGRAM_LABEL_HEIGHT: f32 = 28.0;
const MAX_HISTOGRAM_BARS: usize = 12;
const WIDE_SEQUENCE_WIDTH: f32 = 900.0;

#[derive(Clone, Copy)]
enum Marker {
    Circle,
    Square,
    Diamond,
}

type HistorySeries = (Color32, Marker, fn(&LearningSnapshot) -> usize);

impl Marker {
    fn paint(self, painter: &Painter, center: Pos2, color: Color32) {
        match self {
            Self::Circle => {
                painter.circle_filled(center, 2.8, color);
            }
            Self::Square => {
                painter.rect_filled(Rect::from_center_size(center, vec2(5.2, 5.2)), 0.0, color);
            }
            Self::Diamond => {
                painter.add(Shape::convex_polygon(
                    vec![
                        pos2(center.x, center.y - 3.4),
                        pos2(center.x + 3.4, center.y),
                        pos2(center.x, center.y + 3.4),
                        pos2(center.x - 3.4, center.y),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
        }
    }
}

pub fn history(ui: &mut Ui, snapshots: &[LearningSnapshot]) {
    if snapshots.is_empty() {
        return;
    }
    let palette = series_palette(ui.visuals());
    history_legend(ui, &palette);
    ui.add_space(4.0);

    let mut ordered = snapshots.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|snapshot| snapshot.as_of);
    let (Some(first), Some(last)) = (ordered.first(), ordered.last()) else {
        return;
    };
    let start = first.as_of;
    let end = last.as_of;
    let max_value = snapshots.iter().fold(0_usize, |max, snapshot| {
        max.max(snapshot.learning)
            .max(snapshot.learnt)
            .max(snapshot.uncertain)
    });

    let (rect, painter) = chart_area(ui);
    let plot = plot_rect(&painter, rect, max_value, HISTORY_DATE_HEIGHT);
    draw_axes(&painter, plot, max_value, ui.visuals());

    let series: [HistorySeries; 3] = [
        (palette[0], Marker::Circle, |snapshot| snapshot.learning),
        (palette[1], Marker::Square, |snapshot| snapshot.learnt),
        (palette[2], Marker::Diamond, |snapshot| snapshot.uncertain),
    ];
    for (color, marker, value) in series {
        let points = ordered
            .iter()
            .map(|snapshot| {
                pos2(
                    time_x(snapshot.as_of, start, end, plot.left(), plot.right()),
                    value_y(value(snapshot), max_value, plot.top(), plot.bottom()),
                )
            })
            .collect::<Vec<_>>();
        if points.len() >= 2 {
            painter.add(Shape::line(points.clone(), Stroke::new(1.6, color)));
        }
        for point in points {
            marker.paint(&painter, point, color);
        }
    }

    let text_color = ui.visuals().weak_text_color();
    if ordered.len() == 1 {
        painter.text(
            pos2(plot.center().x, rect.bottom()),
            Align2::CENTER_BOTTOM,
            &first.date,
            label_font(),
            text_color,
        );
    } else {
        painter.text(
            pos2(plot.left(), rect.bottom()),
            Align2::LEFT_BOTTOM,
            &first.date,
            label_font(),
            text_color,
        );
        painter.text(
            pos2(plot.right(), rect.bottom()),
            Align2::RIGHT_BOTTOM,
            &last.date,
            label_font(),
            text_color,
        );
    }
}

pub fn scheduled(ui: &mut Ui, buckets: ScheduledBuckets) {
    let rows = [
        (tr("Overdue").to_owned(), buckets.overdue),
        (tr("Today").to_owned(), buckets.today),
        (tr("Next 7 days").to_owned(), buckets.week),
        (tr("Next 30 days").to_owned(), buckets.month),
        (tr("Next 365 days").to_owned(), buckets.year),
        (tr("Later").to_owned(), buckets.later),
    ];
    let color = series_palette(ui.visuals())[0];
    bar_chart(ui, &rows, color, SCHEDULED_LABEL_HEIGHT, 56.0);
}

pub fn sequences(ui: &mut Ui, histograms: &SequenceHistograms) {
    let correct = histogram_rows(&histograms.correct_streak);
    let wrong = histogram_rows(&histograms.wrong_streak);
    let attempts = histogram_rows(&histograms.attempts);
    let palette = series_palette(ui.visuals());

    if ui.available_width() >= WIDE_SEQUENCE_WIDTH {
        ui.columns(3, |columns| {
            sequence_chart(&mut columns[0], "Correct streak", &correct, palette[1]);
            sequence_chart(&mut columns[1], "Wrong streak", &wrong, palette[2]);
            sequence_chart(&mut columns[2], "Attempts", &attempts, palette[0]);
        });
    } else {
        sequence_chart(ui, "Correct streak", &correct, palette[1]);
        sequence_chart(ui, "Wrong streak", &wrong, palette[2]);
        sequence_chart(ui, "Attempts", &attempts, palette[0]);
    }
}

fn history_legend(ui: &mut Ui, palette: &[Color32; 3]) {
    ui.horizontal_wrapped(|ui| {
        for (key, color, marker) in [
            ("Still learning", palette[0], Marker::Circle),
            ("Learnt moves", palette[1], Marker::Square),
            ("Long overdue learnt moves", palette[2], Marker::Diamond),
        ] {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 12.0), Sense::hover());
                let painter = ui.painter().with_clip_rect(rect);
                let y = rect.center().y;
                painter.line_segment(
                    [pos2(rect.left(), y), pos2(rect.right(), y)],
                    Stroke::new(1.6, color),
                );
                marker.paint(&painter, rect.center(), color);
                ui.label(tr(key));
            });
        }
    });
}

fn sequence_chart(ui: &mut Ui, title: &'static str, rows: &[(String, usize)], color: Color32) {
    ui.add(egui::Label::new(RichText::new(tr(title)).strong()).truncate());
    ui.add_space(4.0);
    bar_chart(ui, rows, color, HISTOGRAM_LABEL_HEIGHT, 38.0);
    ui.add_space(10.0);
}

fn chart_area(ui: &mut Ui) -> (Rect, Painter) {
    let width = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, CHART_HEIGHT), Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);
    (rect, painter)
}

fn plot_rect(painter: &Painter, rect: Rect, max_value: usize, label_height: f32) -> Rect {
    let text_color = Color32::TRANSPARENT;
    let y_label_width = [max_value.to_string(), "0".to_owned()]
        .into_iter()
        .map(|label| {
            painter
                .layout_no_wrap(label, label_font(), text_color)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    let reserved = (y_label_width + 8.0).min((rect.width() - 32.0).max(0.0));
    let right = (rect.right() - 4.0).max(rect.left());
    let left = (rect.left() + reserved).min(right);
    let top = (rect.top() + 6.0).min(rect.bottom());
    let bottom = (rect.bottom() - label_height).max(top).min(rect.bottom());
    Rect::from_min_max(pos2(left, top), pos2(right, bottom))
}

fn draw_axes(painter: &Painter, plot: Rect, max_value: usize, visuals: &Visuals) {
    let grid_color = visuals.weak_text_color().gamma_multiply(0.18);
    for y in [plot.top(), plot.center().y, plot.bottom()] {
        painter.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0, grid_color),
        );
    }
    let axis_color = visuals.weak_text_color().gamma_multiply(0.55);
    painter.line_segment(
        [plot.left_top(), plot.left_bottom()],
        Stroke::new(1.0, axis_color),
    );
    painter.line_segment(
        [plot.left_bottom(), plot.right_bottom()],
        Stroke::new(1.0, axis_color),
    );
    let text_color = visuals.weak_text_color();
    painter.text(
        pos2(plot.left() - 6.0, plot.top()),
        Align2::RIGHT_TOP,
        max_value,
        label_font(),
        text_color,
    );
    painter.text(
        pos2(plot.left() - 6.0, plot.bottom()),
        Align2::RIGHT_BOTTOM,
        0,
        label_font(),
        text_color,
    );
}

fn bar_chart(
    ui: &mut Ui,
    rows: &[(String, usize)],
    color: Color32,
    label_height: f32,
    max_bar_width: f32,
) {
    let max_value = rows.iter().map(|(_, count)| *count).max().unwrap_or(0);
    let (rect, painter) = chart_area(ui);
    let plot = plot_rect(&painter, rect, max_value, label_height);
    draw_axes(&painter, plot, max_value, ui.visuals());
    if rows.is_empty() {
        return;
    }

    let slot_width = (plot.width() / rows.len() as f32).max(0.0);
    let text_color = ui.visuals().weak_text_color();
    for (index, (label, count)) in rows.iter().enumerate() {
        let center = plot.left() + (index as f32 + 0.5) * slot_width;
        if *count > 0 && max_value > 0 {
            let width = (slot_width - 6.0).max(0.0).min(max_bar_width);
            if width > 0.0 {
                let top = value_y(*count, max_value, plot.top(), plot.bottom());
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(center - width / 2.0, top),
                        pos2(center + width / 2.0, plot.bottom()),
                    ),
                    0.0,
                    color,
                );
            }
        }
        let galley = painter.layout(label.clone(), label_font(), text_color, slot_width.max(1.0));
        painter.galley(
            pos2(
                center - galley.size().x / 2.0,
                rect.bottom() - galley.size().y,
            ),
            galley,
            text_color,
        );
    }
}

fn value_y(value: usize, max_value: usize, top: f32, bottom: f32) -> f32 {
    if max_value == 0 {
        return bottom;
    }
    let fraction = (value as f64 / max_value as f64).clamp(0.0, 1.0) as f32;
    bottom - (bottom - top) * fraction
}

fn time_x(at: i64, start: i64, end: i64, left: f32, right: f32) -> f32 {
    if start == end {
        return (left + right) / 2.0;
    }
    let numerator = i128::from(at) - i128::from(start);
    let denominator = i128::from(end) - i128::from(start);
    let fraction = (numerator as f64 / denominator as f64).clamp(0.0, 1.0) as f32;
    left + (right - left) * fraction
}

fn histogram_rows<K: Ord + fmt::Display>(histogram: &BTreeMap<K, usize>) -> Vec<(String, usize)> {
    if histogram.len() <= MAX_HISTOGRAM_BARS {
        return histogram
            .iter()
            .map(|(value, count)| (value.to_string(), *count))
            .collect();
    }

    let mut rows = histogram
        .iter()
        .take(MAX_HISTOGRAM_BARS - 1)
        .map(|(value, count)| (value.to_string(), *count))
        .collect::<Vec<_>>();
    if let Some((value, _)) = histogram.iter().nth(MAX_HISTOGRAM_BARS - 1) {
        let count = histogram
            .iter()
            .skip(MAX_HISTOGRAM_BARS - 1)
            .fold(0_usize, |sum, (_, count)| sum.saturating_add(*count));
        rows.push((format!("{value}+"), count));
    }
    rows
}

fn series_palette(visuals: &Visuals) -> [Color32; 3] {
    if visuals.dark_mode {
        [
            Color32::from_rgb(112, 184, 232),
            Color32::from_rgb(126, 200, 150),
            Color32::from_rgb(232, 174, 100),
        ]
    } else {
        [
            Color32::from_rgb(35, 105, 155),
            Color32::from_rgb(45, 125, 80),
            Color32::from_rgb(170, 95, 35),
        ]
    }
}

fn label_font() -> FontId {
    FontId::proportional(12.0)
}

#[cfg(test)]
mod tests {
    use trainer::repertoire::{LearningSide, LocalDatePolicy, StatsScope};

    use super::*;

    fn snapshot(
        at: i64,
        date: &str,
        learning: usize,
        learnt: usize,
        uncertain: usize,
    ) -> LearningSnapshot {
        LearningSnapshot {
            date: date.to_owned(),
            scope: StatsScope::new(Vec::new(), LearningSide::White, at),
            as_of: at,
            timezone: LocalDatePolicy::UTC,
            total: learning + learnt,
            unseen: 0,
            learning,
            learnt,
            uncertain,
            due: 0,
        }
    }

    fn input(width: f32, height: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, height))),
            time: Some(0.0),
            ..Default::default()
        }
    }

    #[test]
    fn value_scaling_has_stable_endpoints() {
        assert_eq!(value_y(0, 20, 10.0, 150.0), 150.0);
        assert_eq!(value_y(20, 20, 10.0, 150.0), 10.0);
        assert_eq!(value_y(10, 20, 10.0, 150.0), 80.0);
    }

    #[test]
    fn zero_scaling_stays_on_the_baseline() {
        assert_eq!(value_y(0, 0, 10.0, 150.0), 150.0);
    }

    #[test]
    fn one_time_point_is_centered() {
        assert_eq!(time_x(42, 42, 42, 20.0, 220.0), 120.0);
    }

    #[test]
    fn time_scaling_handles_the_full_timestamp_range() {
        assert_eq!(time_x(i64::MIN, i64::MIN, i64::MAX, 20.0, 220.0), 20.0);
        assert_eq!(time_x(i64::MAX, i64::MIN, i64::MAX, 20.0, 220.0), 220.0);
        let middle = time_x(0, i64::MIN, i64::MAX, 20.0, 220.0);
        assert!((middle - 120.0).abs() < 0.001);
    }

    #[test]
    fn histogram_tail_preserves_zero_and_total() {
        let histogram = (0_u64..25).map(|value| (value, 1_usize)).collect();
        let rows = histogram_rows(&histogram);
        assert_eq!(rows.len(), MAX_HISTOGRAM_BARS);
        assert_eq!(rows[0], ("0".to_owned(), 1));
        assert_eq!(rows.last(), Some(&("11+".to_owned(), 14)));
        assert_eq!(rows.iter().map(|(_, count)| count).sum::<usize>(), 25);
    }

    #[test]
    fn one_snapshot_paints_a_visible_marker() {
        let snapshots = [snapshot(42, "2026-11-04", 2, 3, 1)];
        let ctx = egui::Context::default();
        let output = ctx.run_ui(input(640.0, 480.0), |ui| history(ui, &snapshots));
        assert!(
            output
                .shapes
                .iter()
                .any(|clipped| matches!(&clipped.shape, Shape::Circle(_)))
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn charts_render_wide_narrow_light_and_dark_without_horizontal_overflow() {
        let snapshots = vec![
            snapshot(100, "2026-11-02", 8, 2, 0),
            snapshot(200, "2026-11-03", 5, 5, 1),
            snapshot(300, "2026-11-04", 3, 9, 2),
        ];
        let scheduled_buckets = ScheduledBuckets {
            overdue: 4,
            today: 2,
            week: 7,
            month: 3,
            year: 5,
            later: 1,
        };
        let histograms = SequenceHistograms {
            correct_streak: (0_u32..18).map(|key| (key, 2)).collect(),
            wrong_streak: BTreeMap::from([(0, 8), (1, 3), (2, 1)]),
            attempts: (0_u64..16).map(|key| (key, 1)).collect(),
        };

        for visuals in [Visuals::light(), Visuals::dark()] {
            for (width, height) in [(1400.0, 900.0), (640.0, 480.0)] {
                let ctx = egui::Context::default();
                ctx.set_visuals(visuals.clone());
                for _ in 0..2 {
                    let output = ctx.run_ui(input(width, height), |ui| {
                        history(ui, &snapshots);
                        ui.separator();
                        scheduled(ui, scheduled_buckets);
                        ui.separator();
                        sequences(ui, &histograms);
                    });
                    assert!(!output.shapes.is_empty());
                    for clipped in &output.shapes {
                        let bounds = clipped
                            .shape
                            .visual_bounding_rect()
                            .intersect(clipped.clip_rect);
                        if bounds.is_finite() && bounds.is_positive() {
                            assert!(
                                bounds.min.x >= -0.5 && bounds.max.x <= width + 0.5,
                                "horizontal overflow at {width}x{height}: {bounds:?}"
                            );
                        }
                    }
                    output.drop_without_applying_deltas();
                }
            }
        }
    }

    #[test]
    fn empty_and_all_zero_inputs_render() {
        let zero_histograms = SequenceHistograms {
            correct_streak: BTreeMap::from([(0, 0)]),
            wrong_streak: BTreeMap::from([(0, 0)]),
            attempts: BTreeMap::from([(0_u64, 0)]),
        };
        let ctx = egui::Context::default();
        let output = ctx.run_ui(input(640.0, 480.0), |ui| {
            history(ui, &[]);
            scheduled(ui, ScheduledBuckets::default());
            sequences(ui, &SequenceHistograms::default());
            sequences(ui, &zero_histograms);
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
    }
}
