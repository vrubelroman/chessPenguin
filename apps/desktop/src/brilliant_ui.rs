//! The windows of the search for great moves (task 27,
//! docs/design/task-27-brilliant-search-2026-11-27.md): the question before a pass
//! (§4.4) and the summary after it (§4.9).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use chess_import::Database;
use chess_import::chessbase::base::Filter;
use egui::{Key, RichText};
use game_review::{GameAnalysis, Judgement};

use crate::brilliant::{Found, Index, Sides, Skip, Totals};
use crate::i18n::{tr, trf};

/// What a list holds before a pass: the records already analysed, those to analyse with
/// their plies, those that cannot be, and the sides of the filter's player.
pub struct Counted {
    pub reviewed: Vec<u32>,
    pub left: Vec<u32>,
    pub plies_left: Vec<usize>,
    pub skipped: Vec<(u32, Skip)>,
    pub sides: Sides,
}

/// Counts what of `records` is analysed (the analysis cache in `cache`), on this thread.
pub fn count(
    db: &Database,
    records: &[u32],
    filter: &Filter,
    cache: &Path,
    cancel: &AtomicBool,
) -> Counted {
    let store = crate::review::Cache(cache.to_path_buf());
    let mut c = Counted {
        reviewed: Vec::new(),
        left: Vec::new(),
        plies_left: Vec::new(),
        skipped: Vec::new(),
        sides: crate::brilliant::player_sides(db, filter, cancel),
    };
    for &r in records {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(parsed) = db.load_game(r as usize) else {
            c.skipped.push((r, Skip::Unreadable));
            continue;
        };
        if let Some(skip) = crate::brilliant::skip_of(&parsed.game, db.row(r as usize).chess960) {
            c.skipped.push((r, skip));
            continue;
        }
        let a = GameAnalysis::with_store(&parsed.game, Some(&store));
        let by_engine = a.has_data() && a.meta.as_ref().is_some_and(|m| m.nodes > 0);
        if by_engine && (a.stale() || a.missing() == 0) {
            c.reviewed.push(r);
        } else {
            c.left.push(r);
            c.plies_left.push(parsed.game.mainline().len());
        }
    }
    c
}

/// The question before a pass.
pub struct StartDialog {
    pub source: PathBuf,
    pub records: Vec<u32>,
    pub filter: Filter,
    pub about: String,
    rx: Receiver<Counted>,
    cancel: Arc<AtomicBool>,
    pub counted: Option<Counted>,
}

pub enum StartOutcome {
    Start,
    /// Every game is analysed already, or «Summary of the reviewed».
    Summary,
    Cancel,
}

impl StartDialog {
    pub fn new(
        ctx: &egui::Context,
        source: PathBuf,
        db: Arc<Database>,
        records: Vec<u32>,
        filter: Filter,
        about: String,
        cache: PathBuf,
    ) -> StartDialog {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (list, f, c, ctx) = (records.clone(), filter.clone(), cancel.clone(), ctx.clone());
        let _ = std::thread::Builder::new()
            .name("brilliant-count".into())
            .spawn(move || {
                let _ = tx.send(count(&db, &list, &f, &cache, &c));
                ctx.request_repaint();
            });
        StartDialog {
            source,
            records,
            filter,
            about,
            rx,
            cancel,
            counted: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<StartOutcome> {
        if self.counted.is_none()
            && let Ok(c) = self.rx.try_recv()
        {
            // All analysed: no question, the summary at once.
            if c.left.is_empty() {
                self.counted = Some(c);
                return Some(StartOutcome::Summary);
            }
            self.counted = Some(c);
        }
        let mut out = None;
        let name = crate::own_db::base_name(&self.source);
        egui::Window::new(tr("Find great moves"))
            .id(egui::Id::new("brilliant_start"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(
                    RichText::new(format!(
                        "{name} · {} {}",
                        crate::i18n::games(self.records.len()),
                        self.about
                    ))
                    .weak()
                    .size(12.0),
                );
                ui.add_space(8.0);
                // Two lines kept for the counts: the window does not jump when they come.
                ui.allocate_ui(egui::vec2(520.0, 40.0), |ui| match &self.counted {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tr("Counting the games already reviewed…"));
                        });
                    }
                    Some(c) => {
                        let secs = crate::brilliant::estimate_secs(
                            c.plies_left.iter().copied(),
                            crate::review::processes(),
                        );
                        ui.label(trf(
                            "The engine will review {games}: about {time}.",
                            &[
                                ("games", &crate::i18n::games(c.left.len())),
                                ("time", &crate::review::format_duration(secs)),
                            ],
                        ));
                        if !c.reviewed.is_empty() {
                            ui.label(trf(
                                "{k} are already reviewed: their marks come from the saved review.",
                                &[("k", &c.reviewed.len())],
                            ));
                        }
                    }
                });
                ui.add_space(6.0);
                ui.label(trf(
                    "The computer will be busy (up to {p} Stockfish 19 processes). You can keep working and stop at any time; the reviewed games are kept.",
                    &[("p", &crate::review::processes())],
                ));
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = self.counted.is_some();
                    let start = ui.add_enabled(ready, egui::Button::new(tr("Start")));
                    if start.clicked() || (ready && ui.input(|i| i.key_pressed(Key::Enter))) {
                        out = Some(StartOutcome::Start);
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        out = Some(StartOutcome::Cancel);
                    }
                    if let Some(c) = &self.counted
                        && !c.reviewed.is_empty()
                        && ui
                            .button(trf(
                                "Summary of the reviewed ({k})",
                                &[("k", &c.reviewed.len())],
                            ))
                            .clicked()
                    {
                        out = Some(StartOutcome::Summary);
                    }
                });
            });
        if out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(StartOutcome::Cancel);
        }
        out
    }
}

impl Drop for StartDialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A row of the summary's list: a !! move of a game.
pub struct BrilliantRow {
    pub record: u32,
    pub found: Found,
    /// «White — Black · result · event · date».
    pub game: String,
}

/// The summary of a pass (§4.9), counted anew from the index for the pass's list.
pub struct Summary {
    pub source: PathBuf,
    pub about: String,
    pub totals: Totals,
    /// The filter's player and the games counted for each colour.
    pub player: Option<(String, usize, usize)>,
    /// Stopped: analysed of all.
    pub stopped: Option<(usize, usize)>,
    /// Not analysed (for «Continue»).
    pub rest: Vec<u32>,
    pub skipped: Vec<(u32, Skip)>,
    pub rows: Vec<BrilliantRow>,
    pub selected: Option<usize>,
}

pub enum SummaryAction {
    /// The game of a row chosen in the table behind.
    Select(u32),
    /// Open the game at the position after the move.
    Open {
        record: u32,
        ply: u16,
    },
    Continue(Vec<u32>),
    /// Sort the table by the «!!» column.
    Sort,
    Close,
}

impl Summary {
    /// The summary of `records` (a pass's list, in its order) from `index`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: &Path,
        db: &Database,
        index: &Index,
        records: &[u32],
        sides: &Sides,
        filter: &Filter,
        about: String,
        stopped: Option<(usize, usize)>,
        skipped: Vec<(u32, Skip)>,
    ) -> Summary {
        let totals = crate::brilliant::totals(index, records, sides);
        let rest = records
            .iter()
            .copied()
            .filter(|r| index.get(*r).is_none())
            .collect();
        let player = sides.player.as_ref().map(|(w, b)| {
            let count = |v: &Vec<u32>| {
                records
                    .iter()
                    .filter(|r| v.binary_search(r).is_ok())
                    .count()
            };
            (filter.player.text.trim().to_owned(), count(w), count(b))
        });
        let rows = totals
            .brilliant
            .iter()
            .map(|(r, f)| {
                let row = db.row(*r as usize);
                let mut parts = vec![format!("{} — {}", row.white, row.black)];
                parts.push(row.result.to_string());
                if !row.event.trim().is_empty() && row.event != "?" {
                    parts.push(row.event.to_string());
                }
                let date = crate::library::display_date(&row.date);
                if !date.is_empty() {
                    parts.push(date);
                }
                BrilliantRow {
                    record: *r,
                    found: f.clone(),
                    game: parts.join(" · "),
                }
            })
            .collect();
        Summary {
            source: source.to_path_buf(),
            about,
            totals,
            player,
            stopped,
            rest,
            skipped,
            rows,
            selected: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<SummaryAction> {
        let mut out = None;
        let mut open = true;
        let t = &self.totals;
        let title = match self.stopped {
            Some((i, n)) => trf(
                "Search stopped · reviewed: {i} of {n}",
                &[("i", &i), ("n", &n)],
            ),
            None => trf(
                "Summary · reviewed: {games}",
                &[("games", &crate::i18n::games(t.games))],
            ),
        };
        let width = (ctx.content_rect().width() - 40.0).min(600.0);
        egui::Window::new(title)
            .id(egui::Id::new("brilliant_summary"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(width)
            .max_height(ctx.content_rect().height() - 80.0)
            .show(ctx, |ui| {
                ui.set_width(width);
                let weak = |ui: &mut egui::Ui, text: String| {
                    ui.add(egui::Label::new(RichText::new(text).weak().size(12.0)).truncate());
                };
                weak(
                    ui,
                    format!("{} · {}", crate::own_db::base_name(&self.source), self.about),
                );
                if let Some((name, w, b)) = &self.player {
                    weak(
                        ui,
                        trf(
                            "Only the moves of «{name}» count: as White {w}, as Black {b}",
                            &[("name", name), ("w", w), ("b", b)],
                        ),
                    );
                }
                if !self.skipped.is_empty() {
                    let reasons: Vec<&str> = {
                        let mut r: Vec<&str> = self.skipped.iter().map(|(_, s)| s.reason()).collect();
                        r.sort_unstable();
                        r.dedup();
                        r
                    };
                    weak(
                        ui,
                        trf(
                            "Not reviewed: {k} ({reason})",
                            &[("k", &self.skipped.len()), ("reason", &reasons.join(", "))],
                        ),
                    );
                }
                if t.stale > 0 {
                    weak(
                        ui,
                        trf(
                            "{k} of them were reviewed with the previous engine settings",
                            &[("k", &t.stale)],
                        ),
                    );
                }
                if !t.player_missing.is_empty() {
                    let numbers: Vec<String> =
                        t.player_missing.iter().map(|r| (r + 1).to_string()).collect();
                    ui.add(
                        egui::Label::new(
                            RichText::new(trf(
                                "In {k} games the player was not found by name: their moves did not count",
                                &[("k", &t.player_missing.len())],
                            ))
                            .weak()
                            .size(12.0),
                        )
                        .truncate(),
                    )
                    .on_hover_text(numbers.join(", "));
                }
                if self.stopped.is_some() && !self.rest.is_empty() {
                    weak(
                        ui,
                        trf(
                            "{k} games are not reviewed: «Continue» reviews only them",
                            &[("k", &self.rest.len())],
                        ),
                    );
                }
                ui.add_space(8.0);
                let dark = ui.visuals().dark_mode;
                egui::Grid::new("brilliant_totals")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        let c = &t.counts;
                        for (j, n) in [
                            (Judgement::Brilliant, c.brilliant),
                            (Judgement::Great, c.great),
                            (Judgement::Inaccuracy, c.inaccuracies),
                            (Judgement::Mistake, c.mistakes),
                            (Judgement::Blunder, c.blunders),
                        ] {
                            let color = if n == 0 {
                                ui.visuals().weak_text_color()
                            } else {
                                crate::review::judgement_text_color(j, dark)
                            };
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(n.to_string()).strong().size(15.0).color(color));
                            });
                            ui.label(
                                RichText::new(crate::review::judgement_plural(j))
                                    .size(15.0)
                                    .color(color),
                            );
                            ui.end_row();
                        }
                        let shown = |v: Option<String>| v.unwrap_or_else(|| "—".to_owned());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(shown(t.acpl.map(|a| format!("{}", a.round() as i64))))
                                    .strong()
                                    .size(15.0),
                            );
                        });
                        ui.label(RichText::new(tr("Average centipawn loss")).size(15.0));
                        ui.end_row();
                        let color = t
                            .accuracy
                            .map(|a| crate::review::accuracy_color(a, dark))
                            .unwrap_or(ui.visuals().weak_text_color());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(shown(t.accuracy.map(|a| format!("{}%", a.round() as i64))))
                                    .strong()
                                    .size(15.0)
                                    .color(color),
                            );
                        });
                        let label = ui.label(RichText::new(tr("Average accuracy")).size(15.0));
                        if let Some((lo, hi)) = t.accuracy_range {
                            label.on_hover_text(trf(
                                "Average over {k}: from {min}% to {max}%",
                                &[
                                    ("k", &crate::i18n::games(t.sides)),
                                    ("min", &(lo.round() as i64)),
                                    ("max", &(hi.round() as i64)),
                                ],
                            ));
                        }
                        ui.end_row();
                    });
                ui.separator();
                if self.rows.is_empty() {
                    if t.counts.great > 0 {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(tr(
                                    "No brilliant moves. Games with ! — sort by the «!!» column",
                                ))
                                .weak(),
                            );
                            if ui.link(tr("Sort")).clicked() {
                                out = Some(SummaryAction::Sort);
                            }
                        });
                    } else {
                        ui.label(RichText::new(tr("No brilliant or great moves")).weak());
                    }
                } else {
                    ui.label(
                        RichText::new(trf("Games with !! ({n})", &[("n", &self.rows.len())]))
                            .strong()
                            .size(13.0),
                    );
                    let teal = crate::review::judgement_text_color(Judgement::Brilliant, dark);
                    let labels: Vec<String> = self.rows.iter().map(|r| r.found.label()).collect();
                    let move_w = labels
                        .iter()
                        .map(|l| {
                            ui.fonts_mut(|f| {
                                f.layout_no_wrap(
                                    l.clone(),
                                    egui::TextStyle::Body.resolve(ui.style()),
                                    egui::Color32::WHITE,
                                )
                                .size()
                                .x
                            })
                        })
                        .fold(0.0, f32::max)
                        + 12.0;
                    let rows = if ctx.content_rect().height() < 700.0 { 3 } else { 6 };
                    egui::ScrollArea::vertical()
                        .max_height(24.0 * rows as f32)
                        .auto_shrink([false, true])
                        .show_rows(ui, 24.0, self.rows.len(), |ui, range| {
                            for i in range {
                                let row = &self.rows[i];
                                let selected = self.selected == Some(i);
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 24.0),
                                    egui::Sense::click(),
                                );
                                if selected {
                                    ui.painter().rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
                                } else if resp.hovered() {
                                    ui.painter()
                                        .rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
                                }
                                let mut child = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(rect.shrink2(egui::vec2(4.0, 0.0)))
                                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                );
                                let color = if selected { ui.visuals().selection.stroke.color } else { teal };
                                child.add_sized(
                                    egui::vec2(move_w, 24.0),
                                    egui::Label::new(RichText::new(&labels[i]).strong().color(color))
                                        .selectable(false),
                                );
                                child.add(egui::Label::new(&row.game).truncate().selectable(false));
                                if resp.clicked() {
                                    self.selected = Some(i);
                                    out = Some(SummaryAction::Select(row.record));
                                }
                                if resp.double_clicked() {
                                    out = Some(SummaryAction::Open {
                                        record: row.record,
                                        ply: row.found.ply,
                                    });
                                }
                            }
                        });
                    ui.label(
                        RichText::new(tr(
                            "Click: choose the row in the table; double click: open at this move",
                        ))
                        .weak()
                        .size(12.0),
                    );
                    // ↑ ↓ through the list, Enter opens.
                    let (up, down, enter) = ui.input(|i| {
                        (
                            i.key_pressed(Key::ArrowUp),
                            i.key_pressed(Key::ArrowDown),
                            i.key_pressed(Key::Enter),
                        )
                    });
                    if up || down {
                        let last = self.rows.len() - 1;
                        let next = match (self.selected, down) {
                            (None, _) => 0,
                            (Some(s), true) => (s + 1).min(last),
                            (Some(s), false) => s.saturating_sub(1),
                        };
                        self.selected = Some(next);
                        out = Some(SummaryAction::Select(self.rows[next].record));
                    }
                    if enter && let Some(s) = self.selected {
                        out = Some(SummaryAction::Open {
                            record: self.rows[s].record,
                            ply: self.rows[s].found.ply,
                        });
                    }
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Close")).clicked() {
                        out = Some(SummaryAction::Close);
                    }
                    if !self.rows.is_empty() {
                        let pick = self.selected.map(|s| &self.rows[s]);
                        if ui
                            .add_enabled(pick.is_some(), egui::Button::new(tr("Open game")))
                            .clicked()
                            && let Some(r) = pick
                        {
                            out = Some(SummaryAction::Open {
                                record: r.record,
                                ply: r.found.ply,
                            });
                        }
                    }
                    if self.stopped.is_some()
                        && !self.rest.is_empty()
                        && ui.button(tr("Continue")).clicked()
                    {
                        out = Some(SummaryAction::Continue(self.rest.clone()));
                    }
                });
            });
        if !open || (out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape))) {
            out = Some(SummaryAction::Close);
        }
        out
    }
}
